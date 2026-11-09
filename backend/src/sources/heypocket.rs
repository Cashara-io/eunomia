//! heypocket source -- meeting recordings from heypocketai.com (connector
//! kind `pocketai`, API key, bearer auth). Poll + on-demand only (the public
//! API has no webhooks).
//!
//! Sync: `GET /public/recordings?start_date=` (paged) for recordings since
//! the last one seen, then `GET /public/recordings/{id}` for each one's
//! transcript and summaries -- the list endpoint carries only metadata.
//!
//! Each recording is kept twice: verbatim in `pocket_recording` (everything
//! Pocket returned, plus the full speaker-labelled transcript -- see
//! [`stored`]), and in the cache as one `heypocket.recording` (summary,
//! action items, speakers, tags) plus `heypocket.transcript_chunk`s small
//! enough to embed and extract entities from.
//!
//! The tool helpers (`summary`, `list_recordings`, `search_recordings`) read
//! `cache_record` directly.

use surrealdb::types::SurrealValue;
use async_trait::async_trait;
use chrono::{Duration, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use surrealdb::types::RecordId;

use crate::connectors::clients::PocketAIClient;
use crate::error::AppResult;
use crate::pool::OrgDb;
use crate::rid::RecordIdExt;
use crate::store;
use crate::sources::base::{datetime_to_chrono, envelope, items, owner_key_str, rfc3339, s, Conn, Source, SourceCtx, SyncResult, MAX_PAGES};

/// Transcript chunk size: well under the embedding input limit, and a
/// reasonable amount of text for one entity-extraction call.
const CHUNK_CHARS: usize = 6000;

pub struct HeyPocketSource;

#[async_trait]
impl Source for HeyPocketSource {
    fn key(&self) -> &'static str {
        "heypocket"
    }

    fn provider(&self) -> &'static str {
        "pocketai"
    }

    fn label(&self) -> &'static str {
        "heypocket"
    }

    fn record_types(&self) -> &'static [&'static str] {
        &["heypocket.recording", "heypocket.transcript_chunk"]
    }

    async fn check(&self, conn: &Conn) -> AppResult<()> {
        PocketAIClient::new(&conn.credentials, &conn.config)?.ping().await
    }

    async fn fetch(&self, conn: &Conn, cursor: Option<String>) -> AppResult<SyncResult> {
        let client = PocketAIClient::new(&conn.credentials, &conn.config)?;
        // `start_date` is a calendar date; the cursor is the newest
        // recording's timestamp, so re-read that whole day (dedupe is free).
        let start = cursor
            .as_deref()
            .map(|c| c.chars().take(10).collect::<String>())
            .unwrap_or_else(|| (Utc::now() - Duration::days(30)).date_naive().to_string());

        let mut data = Vec::new();
        for page in 1..=MAX_PAGES {
            let res = client
                .recordings(&[("start_date", start.clone()), ("limit", "100".to_string()), ("page", page.to_string())])
                .await?;
            data.extend(items(&res, "/data"));
            if !res.pointer("/pagination/has_more").and_then(Value::as_bool).unwrap_or(false) {
                break;
            }
        }

        // The list endpoint has no transcript/summary: fold each recording's
        // full detail over its list entry, verbatim.
        for r in data.iter_mut() {
            let Some(rid) = id_of(r).map(String::from) else { continue };
            let detail = client.recording(&rid).await?.get("data").cloned().unwrap_or(json!({}));
            if let (Some(obj), Some(detail)) = (r.as_object_mut(), detail.as_object()) {
                obj.extend(detail.clone());
            }
        }

        let newest = data
            .iter()
            .filter_map(|r| r.get("recording_at").or_else(|| r.get("created_at")).and_then(|v| v.as_str()))
            .max()
            .map(String::from)
            .or(cursor)
            .unwrap_or(start);

        Ok(SyncResult { records: data, cursor: Some(newest), ..SyncResult::default() })
    }

    fn map(&self, raw: &Value) -> Option<Value> {
        self.map_many(raw).into_iter().next()
    }

    fn map_many(&self, raw: &Value) -> Vec<Value> {
        let Some(rid) = id_of(raw) else { return Vec::new() };
        let r = Recording::from(raw);
        let title = s(raw, "/title");
        let at = rfc3339(raw.get("recording_at").or_else(|| raw.get("created_at")).and_then(|v| v.as_str()).unwrap_or(""));
        let url = raw.get("url").or_else(|| raw.get("share_url")).and_then(|v| v.as_str()).unwrap_or("");

        let mut body = vec![r.summary.clone()];
        if !r.action_items.is_empty() {
            body.push(format!("Action items:\n{}", r.action_items.iter().map(|a| format!("- {a}")).collect::<Vec<_>>().join("\n")));
        }
        if !r.speakers.is_empty() {
            body.push(format!("Speakers: {}", r.speakers.join(", ")));
        }
        if !r.tags.is_empty() {
            body.push(format!("Tags: {}", r.tags.join(", ")));
        }
        let body = body.into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join("\n\n");

        let mut main = envelope(
            "heypocket",
            "heypocket.recording",
            rid,
            title,
            if body.is_empty() { title } else { &body },
            at.clone(),
            url,
            json!({"duration_seconds": raw.get("duration").cloned().unwrap_or(json!(0)), "tags": r.tags, "speakers": r.speakers}),
        );
        main["deleted"] = json!(raw.get("deleted").and_then(|v| v.as_bool()).unwrap_or(false));

        let chunks = chunk(&r.transcript);
        let n = chunks.len();
        let mut out = vec![main];
        for (i, text) in chunks.into_iter().enumerate() {
            let mut env = envelope(
                "heypocket",
                "heypocket.transcript_chunk",
                &format!("{rid}:{i}"),
                &format!("{title} (transcript {}/{n})", i + 1),
                &text,
                at.clone(),
                url,
                json!({"recording_id": rid, "part": i}),
            );
            env["links"] = json!([{"target": format!("heypocket:heypocket.recording:{rid}"), "rel": "part_of"}]);
            out.push(env);
        }
        out
    }

    async fn persist(&self, db: &OrgDb, owner: &RecordId, raw: &Value, conn: Option<&Conn>) -> AppResult<()> {
        let Some(rid) = id_of(raw) else { return Ok(()) };
        let r = Recording::from(raw);
        let recorded_at = raw
            .get("recording_at")
            .or_else(|| raw.get("created_at"))
            .and_then(|v| v.as_str())
            .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
            .map(|d| surrealdb::types::Datetime::from(d.with_timezone(&Utc)));
        let parts = chunk(&r.transcript).len();
        crate::tx::with_retry(|| async {
            store::app::SOURCES_POCKET_RECORDING_PUT
                .on(db)
                .bind(("id", record_id(owner, rid)))
                .bind(("owner", owner.clone()))
                .bind(("rid", rid.to_string()))
                .bind(("title", s(raw, "/title").to_string()))
                .bind(("recorded_at", recorded_at))
                .bind(("duration", raw.get("duration").and_then(Value::as_f64).unwrap_or(0.0)))
                .bind(("tags", r.tags.clone()))
                .bind(("speakers", r.speakers.clone()))
                .bind(("summary", r.summary.clone()))
                .bind(("action_items", r.action_items.clone()))
                .bind(("transcript", r.transcript.join("\n")))
                .bind(("raw", raw.clone()))
                .bind(("parts", parts as i64))
                .await
        })
        .await?;
        if let Some(conn) = conn.filter(|c| store_audio(&c.config)) {
            store_recording_audio(db, owner, rid, conn).await;
        }
        Ok(())
    }
}

/// Largest recording audio kept (#117). Longer recordings keep their transcript only.
// ponytail: fixed cap; an env var (like EUNOMIA_DOCUMENTS_MAX_BYTES) if someone records longer
pub const MAX_AUDIO_BYTES: usize = 200 << 20;

/// Whether a Pocket connection keeps recording audio (`config.store_audio`, off unless set).
pub fn store_audio(config: &Value) -> bool {
    config.get("store_audio").and_then(Value::as_bool).unwrap_or(false)
}

/// Where a recording's audio goes in the org's `documents` bucket.
pub fn audio_path(owner: &RecordId, recording_id: &str, media_type: &str) -> String {
    use crate::documents::safe_segment;
    let ext = match media_type {
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/wav" | "audio/x-wav" | "audio/wave" => "wav",
        "audio/ogg" | "audio/opus" => "ogg",
        "audio/webm" => "webm",
        "audio/flac" | "audio/x-flac" => "flac",
        "audio/aac" => "aac",
        _ => "m4a",
    };
    format!("/pocket/{}/{}.{ext}", safe_segment(&owner_key_str(owner)), safe_segment(recording_id))
}

/// A recording's stored audio: where it is and what it is. `path` is NONE when there is none.
#[derive(Debug, Deserialize, SurrealValue)]
pub struct StoredAudio {
    #[serde(default)]
    #[surreal(default)]
    pub audio_path: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    pub audio_media_type: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    pub audio_bytes: Option<i64>,
}

/// The stored audio of `owner`'s recording `recording_id`, if any was downloaded.
pub async fn stored_audio(db: &OrgDb, owner: &RecordId, recording_id: &str) -> AppResult<Option<StoredAudio>> {
    let mut res = store::app::SOURCES_POCKET_AUDIO_GET.on(db).bind(("id", record_id(owner, recording_id))).await?;
    let row: Option<StoredAudio> = res.take(0)?;
    Ok(row.filter(|r| r.audio_path.is_some()))
}

/// Downloads and keeps one recording's audio, once. Never fails the sync: audio Pocket does not have
/// yet (the device uploads it later) is retried on a later sync while the recording is still inside
/// the sync window; anything else (too large, provider error, no bucket) is logged and skipped.
async fn store_recording_audio(db: &OrgDb, owner: &RecordId, rid: &str, conn: &Conn) {
    static NO_BUCKET_WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    match stored_audio(db, owner, rid).await {
        // audio never changes: downloaded once
        Ok(Some(_)) => return,
        Ok(None) => {}
        Err(e) => {
            tracing::warn!(recording = rid, error = %e.message, "pocket audio: stored row not read");
            return;
        }
    }
    let client = match PocketAIClient::new(&conn.credentials, &conn.config) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(recording = rid, error = %e.message, "pocket audio: no client");
            return;
        }
    };
    let (bytes, media_type) = match client.audio(rid, MAX_AUDIO_BYTES).await {
        Ok(Some(found)) => found,
        // not uploaded from the device yet: a later sync tries again
        Ok(None) => return,
        Err(e) => {
            tracing::warn!(recording = rid, error = %e.message, "pocket audio: not downloaded");
            return;
        }
    };
    let media_type = if media_type.starts_with("audio/") || media_type.starts_with("video/") { media_type } else { "audio/mp4".to_string() };
    let path = audio_path(owner, rid, &media_type);
    let size = bytes.len() as i64;
    if let Err(e) = crate::documents::put_object(db, &path, bytes).await {
        if !NO_BUCKET_WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            tracing::warn!(error = %e.message, "pocket audio: not stored (is EUNOMIA_DOCUMENTS_BACKEND set?); further failures are logged at debug");
        } else {
            tracing::debug!(recording = rid, error = %e.message, "pocket audio: not stored");
        }
        return;
    }
    let set = crate::tx::with_retry(|| async {
        store::app::SOURCES_POCKET_AUDIO_SET
            .on(db)
            .bind(("id", record_id(owner, rid)))
            .bind(("path", path.clone()))
            .bind(("media_type", media_type.clone()))
            .bind(("size", size))
            .await?
            .check()
    })
    .await;
    if let Err(e) = set {
        tracing::warn!(recording = rid, error = %e, "pocket audio: stored but not recorded; removing the file");
        let _ = crate::documents::delete_object(db, &path).await;
    }
}

fn id_of(raw: &Value) -> Option<&str> {
    raw.get("id").or_else(|| raw.get("recording_id")).and_then(|v| v.as_str())
}

fn record_id(owner: &RecordId, recording_id: &str) -> RecordId {
    RecordId::from_table_key("pocket_recording", format!("{}:{recording_id}", owner_key_str(owner)))
}

/// The parts of a Pocket recording worth reading, out of its raw JSON.
struct Recording {
    tags: Vec<String>,
    speakers: Vec<String>,
    summary: String,
    action_items: Vec<String>,
    /// One `"Speaker: text"` line per transcript segment.
    transcript: Vec<String>,
}

impl From<&Value> for Recording {
    fn from(raw: &Value) -> Self {
        let tags = items(raw, "/tags").iter().filter_map(|t| t.get("name").and_then(|n| n.as_str()).map(String::from)).collect();

        // `speakers` maps a segment's `speaker` id to `{name, speakerId}`.
        let speaker_name = |seg: &Value| {
            let id = s(seg, "/speaker");
            [s(seg, "/speakerName"), raw.pointer(&format!("/speakers/{id}/name")).and_then(|v| v.as_str()).unwrap_or(""), id]
                .into_iter()
                .find(|n| !n.is_empty())
                .unwrap_or("Unknown")
                .to_string()
        };
        let segments = items(raw, "/transcript/segments");
        let transcript: Vec<String> = segments
            .iter()
            .filter(|seg| !s(seg, "/text").trim().is_empty())
            .map(|seg| format!("{}: {}", speaker_name(seg), s(seg, "/text").trim()))
            .collect();
        let mut speakers: Vec<String> = segments.iter().filter(|seg| !s(seg, "/text").trim().is_empty()).map(speaker_name).collect();
        let mut seen = std::collections::HashSet::new();
        speakers.retain(|n| seen.insert(n.clone()));

        // `summarizations` maps a summarization id to its versions; the
        // current one (`v2`) carries markdown and action items.
        let mut summary = Vec::new();
        let mut action_items = Vec::new();
        for sm in raw.get("summarizations").and_then(|v| v.as_object()).into_iter().flat_map(|m| m.values()) {
            let markdown = s(sm, "/v2/summary/markdown").trim();
            if !markdown.is_empty() {
                summary.push(markdown.to_string());
            }
            for item in items(sm, "/v2/actionItems/items") {
                let text = item.as_str().or_else(|| ["/title", "/text", "/label"].iter().map(|p| s(&item, p)).find(|t| !t.is_empty()));
                if let Some(t) = text.map(str::trim).filter(|t| !t.is_empty()) {
                    action_items.push(t.to_string());
                }
            }
        }

        Recording { tags, speakers, summary: summary.join("\n\n"), action_items, transcript }
    }
}

/// Transcript lines grouped into chunks of at most ~[`CHUNK_CHARS`] (a
/// single longer line is split on character boundaries).
fn chunk(lines: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for line in lines {
        let chars: Vec<char> = line.chars().collect();
        for piece in chars.chunks(CHUNK_CHARS) {
            let piece: String = piece.iter().collect();
            if !cur.is_empty() && cur.chars().count() + piece.chars().count() + 1 > CHUNK_CHARS {
                out.push(std::mem::take(&mut cur));
            }
            if !cur.is_empty() {
                cur.push('\n');
            }
            cur.push_str(&piece);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Everything stored for one recording: title, summary, action items, tags,
/// speakers and the full transcript (not the verbatim `raw`, which can be
/// large; it stays in the table). `None` if it isn't stored.
pub async fn stored(db: &OrgDb, owner: &RecordId, recording_id: &str) -> AppResult<Option<Value>> {
    let mut res = store::app::SOURCES_POCKET_RECORDING_GET.on(db).bind(("id", record_id(owner, recording_id))).await?;
    Ok(res.take::<Option<Value>>(0)?)
}

#[derive(Debug, Deserialize, SurrealValue)]
pub(crate) struct CachedRecording {
    #[serde(default)]
    #[surreal(default)]
    title: String,
    #[serde(default)]
    #[surreal(default)]
    occurred_at: Option<surrealdb::types::Datetime>,
    #[serde(default)]
    #[surreal(default)]
    url: String,
    #[serde(default)]
    #[surreal(default)]
    payload: Value,
}

async fn cached_recordings(ctx: &SourceCtx<'_>, days: i64) -> AppResult<Vec<CachedRecording>> {
    let since = Utc::now() - Duration::days(days);
    let mut res = store::app::SOURCES_HEYPOCKET_RECENT
        .on(ctx.db)
        .bind(("owner", ctx.owner.clone()))
        .bind(("since", surrealdb::types::Datetime::from(since)))
        .await?;
    Ok(res.take(0)?)
}

fn iso(dt: &Option<surrealdb::types::Datetime>) -> Option<String> {
    dt.as_ref().and_then(datetime_to_chrono).map(|d| d.to_rfc3339())
}

fn duration_minutes(payload: &Value) -> f64 {
    let secs = payload.get("duration_seconds").and_then(|v| v.as_f64()).unwrap_or(0.0);
    (secs / 60.0 * 10.0).round() / 10.0
}

fn tags_of(payload: &Value) -> Vec<String> {
    payload
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|t| t.as_str().map(String::from)).collect())
        .unwrap_or_default()
}

/// Recording count/duration/tag breakdown over the last `days`, computed from
/// cached recordings (not a live API call) with no invented metrics.
pub(crate) fn compute_summary(recs: &[CachedRecording]) -> Value {
    let mut tag_counts: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    for r in recs {
        for tag in tags_of(&r.payload) {
            *tag_counts.entry(tag).or_insert(0) += 1;
        }
    }
    let mut tag_breakdown: Vec<Value> = tag_counts.into_iter().map(|(k, v)| json!({"tag": k, "count": v})).collect();
    tag_breakdown.sort_by(|a, b| b["count"].as_i64().unwrap_or(0).cmp(&a["count"].as_i64().unwrap_or(0)));

    let total_minutes: f64 = recs.iter().map(|r| r.payload.get("duration_seconds").and_then(|v| v.as_f64()).unwrap_or(0.0)).sum::<f64>() / 60.0;

    json!({
        "recordings_count": recs.len(),
        "total_duration_minutes": (total_minutes * 10.0).round() / 10.0,
        "tag_breakdown": tag_breakdown,
        "recent_recordings": recs.iter().take(10).map(|r| json!({
            "title": r.title,
            "duration_minutes": duration_minutes(&r.payload),
            "recorded_at": iso(&r.occurred_at),
            "tags": tags_of(&r.payload),
        })).collect::<Vec<_>>(),
    })
}

pub async fn summary(ctx: &SourceCtx<'_>, days: i64) -> AppResult<Value> {
    Ok(compute_summary(&cached_recordings(ctx, days).await?))
}

pub async fn list_recordings(ctx: &SourceCtx<'_>, days: i64, tag: Option<&str>, limit: i64) -> AppResult<Value> {
    let recs = cached_recordings(ctx, days).await?;
    let limit = limit.clamp(0, 200) as usize;
    let out: Vec<Value> = recs
        .iter()
        .filter(|r| tag.map(|t| tags_of(&r.payload).iter().any(|x| x == t)).unwrap_or(true))
        .take(limit)
        .map(|r| json!({
            "title": r.title,
            "duration_minutes": duration_minutes(&r.payload),
            "recorded_at": iso(&r.occurred_at),
            "tags": tags_of(&r.payload),
            "url": if r.url.is_empty() { Value::Null } else { json!(r.url) },
        }))
        .collect();
    Ok(json!(out))
}

/// A plain case-insensitive substring match over cached title/body_text.
pub async fn search_recordings(ctx: &SourceCtx<'_>, query: &str) -> AppResult<Value> {
    let needle = query.to_lowercase();
    let mut res = store::app::SOURCES_HEYPOCKET_SEARCH
        .on(ctx.db)
        .bind(("owner", ctx.owner.clone()))
        .bind(("q", needle))
        .await?;
    let recs: Vec<CachedRecording> = res.take(0)?;
    Ok(json!({
        "results": recs.iter().map(|r| json!({
            "title": r.title,
            "recorded_at": iso(&r.occurred_at),
            "tags": tags_of(&r.payload),
            "url": if r.url.is_empty() { Value::Null } else { json!(r.url) },
        })).collect::<Vec<_>>()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(title: &str, mins: f64, tags: Vec<&str>) -> CachedRecording {
        CachedRecording {
            title: title.to_string(),
            occurred_at: None,
            url: String::new(),
            payload: json!({"duration_seconds": mins * 60.0, "tags": tags}),
        }
    }

    #[test]
    fn compute_summary_tallies_tags_and_duration() {
        let recs = vec![rec("Standup", 10.0, vec!["work"]), rec("1:1", 30.0, vec!["work", "1:1"])];
        let summary = compute_summary(&recs);
        assert_eq!(summary["recordings_count"], 2);
        assert_eq!(summary["total_duration_minutes"], 40.0);
        assert_eq!(summary["tag_breakdown"][0]["tag"], "work");
        assert_eq!(summary["tag_breakdown"][0]["count"], 2);
    }

    #[test]
    fn compute_summary_empty_input_has_zeroed_fields() {
        let summary = compute_summary(&[]);
        assert_eq!(summary["recordings_count"], 0);
        assert_eq!(summary["total_duration_minutes"], 0.0);
        assert_eq!(summary["tag_breakdown"], json!([]));
    }

    use crate::cache::search::Envelope;
    use crate::sources::mock::{assert_fetch_fails, route, serve};

    #[tokio::test]
    async fn fetch_pages_since_the_cursor_day_and_folds_in_each_detail() {
        let list = |id: &str, at: &str, more: bool| {
            json!({"success": true, "data": [{"id": id, "title": "Weekly standup", "duration": 900, "recording_at": at,
                    "tags": [{"id": "t1", "name": "work"}]}],
                   "pagination": {"has_more": more}})
        };
        let detail = |id: &str| {
            json!({"success": true, "data": {
                "id": id, "language": "en", "speakers": {"s1": {"name": "Ada", "speakerId": "s1"}},
                "transcript": {"segments": [{"speaker": "s1", "start": 0, "end": 900, "text": "Morning all."},
                                            {"speaker": "s2", "speakerName": "Grace", "start": 900, "end": 1800, "text": "Importer is ready."}]},
                "summarizations": {"sm1": {"v2": {"summary": {"markdown": "Agreed to ship the importer on Friday."},
                                                  "actionItems": {"items": [{"title": "Ship the importer"}]}}}},
            }})
        };
        let mock = serve(vec![
            route("GET", "/public/recordings", list("rec_1", "2024-05-02T09:00:00Z", true)).query_has("page=1"),
            route("GET", "/public/recordings", list("rec_2", "2024-05-03T09:00:00Z", false)).query_has("page=2"),
            route("GET", "/public/recordings/rec_1", detail("rec_1")),
            route("GET", "/public/recordings/rec_2", detail("rec_2")),
        ])
        .await;

        let res = HeyPocketSource
            .fetch(&mock.conn(json!({"api_key": "pk_test"})), Some("2024-05-01T08:00:00Z".into()))
            .await
            .unwrap();
        assert_eq!(res.cursor.as_deref(), Some("2024-05-03T09:00:00Z"));
        assert_eq!(res.records.len(), 2);
        assert!(mock.requests().iter().all(|r| r.header("authorization") == "Bearer pk_test"));
        assert!(mock.requests().iter().filter(|r| r.path == "/public/recordings").all(|r| r.query.contains("start_date=2024-05-01") && r.query.contains("limit=100")));
        // The list entry and the detail are both kept, verbatim.
        assert_eq!(res.records[0]["duration"], 900);
        assert_eq!(res.records[0]["language"], "en");

        let envs: Vec<Envelope> = HeyPocketSource.map_many(&res.records[0]).into_iter().map(|v| serde_json::from_value(v).unwrap()).collect();
        assert_eq!(envs.len(), 2, "the recording + one transcript chunk");
        assert_eq!(envs[0].title, "Weekly standup");
        assert_eq!(
            envs[0].body_text,
            "Agreed to ship the importer on Friday.\n\nAction items:\n- Ship the importer\n\nSpeakers: Ada, Grace\n\nTags: work"
        );
        assert_eq!(envs[0].payload["tags"], json!(["work"]));
        assert_eq!(envs[1].id, "heypocket:heypocket.transcript_chunk:rec_1:0");
        assert_eq!(envs[1].body_text, "Ada: Morning all.\nGrace: Importer is ready.");
        assert_eq!(envs[1].links[0].target, "heypocket:heypocket.recording:rec_1");
    }

    #[test]
    fn long_transcripts_are_chunked_within_the_limit_without_losing_text() {
        let lines: Vec<String> = (0..40).map(|i| format!("Ada: line {i} {}", "x".repeat(400))).chain(["Grace: ".to_string() + &"y".repeat(15000)]).collect();
        let chunks = chunk(&lines);
        assert!(chunks.iter().all(|c| c.chars().count() <= CHUNK_CHARS));
        assert_eq!(chunks.join("\n").replace('\n', ""), lines.concat());
        assert!(chunk(&[]).is_empty());
    }

    #[tokio::test]
    async fn fetch_errors_are_visible() {
        let creds = json!({"api_key": "bad"});
        assert_fetch_fails(&HeyPocketSource, 401, "/public/recordings", creds.clone(), "HTTP 401").await;
        assert_fetch_fails(&HeyPocketSource, 429, "/public/recordings", creds.clone(), "HTTP 429").await;
        assert_fetch_fails(&HeyPocketSource, 503, "/public/recordings", creds, "HTTP 503").await;
    }
}

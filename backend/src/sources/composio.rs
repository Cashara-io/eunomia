//! One consumer account, several explicitly selected app accounts, independent checkpoints.
use super::base::{Conn, Source, SyncProgress, SyncResult, envelope, s};
use crate::rid::RecordIdExt;
use crate::{
    connectors::composio::{
        mcp::Failure,
        tools::{self, ReadClient, Selection},
    },
    error::{AppError, AppResult},
};
use async_trait::async_trait;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Mutex;

pub struct ComposioSource;
#[derive(Default, Deserialize, Serialize)]
#[serde(default)]
struct Checkpoint {
    page: String,
    since: String,
    until: String,
    pending: Vec<Value>,
    chats: Vec<String>,
    chat_offset: usize,
    message_offset: usize,
    gmail: Option<String>,
    calendar_sync: String,
}
#[async_trait]
impl Source for ComposioSource {
    fn key(&self) -> &'static str {
        "composio"
    }
    fn label(&self) -> &'static str {
        "Composio"
    }
    fn record_types(&self) -> &'static [&'static str] {
        &[
            "composio.file",
            "composio.message",
            "composio.event",
            "composio.note",
            "composio.delivery",
        ]
    }
    async fn check(&self, conn: &Conn) -> AppResult<()> {
        ReadClient::new(s(&conn.credentials, "/access_token"))
            .await?
            .mcp
            .tools()
            .await
            .map(|_| ())
            .map_err(tools::failure)
    }
    async fn fetch(&self, conn: &Conn, cursor: Option<String>) -> AppResult<SyncResult> {
        let selections: Vec<Selection> =
            serde_json::from_value(conn.config.get("apps").cloned().unwrap_or(json!([])))
                .map_err(|_| AppError::bad_request("Choose apps in the Composio card."))?;
        let mut checkpoints: std::collections::BTreeMap<String, Checkpoint> = match cursor {
            Some(c) => serde_json::from_str(&c)
                .map_err(|_| AppError::internal("Invalid Composio checkpoint."))?,
            None => Default::default(),
        };
        let mut client = ReadClient::new(s(&conn.credentials, "/access_token")).await?;
        let mut result = SyncResult::default();
        for selection in &selections {
            selection.validate()?;
            let cp = checkpoints.entry(selection.key()).or_default();
            let batch = if selection.toolkit == "gmail" {
                let reader = GmailBridge {
                    client: Mutex::new(&mut client),
                    selection,
                    next_read: Mutex::new(tokio::time::Instant::now()),
                };
                let batch = super::gmail::fetch_with(&reader, &reader, cp.gmail.clone()).await;
                match batch {
                    Ok(mut batch) => {
                        cp.gmail = batch.cursor.take();
                        batch.records = batch
                            .records
                            .iter()
                            .filter_map(|r| super::gmail::GmailSource.map(r))
                            .map(|r| wrap(selection, "message", r))
                            .collect();
                        Ok(batch)
                    }
                    Err(e) => Err(e),
                }
            } else {
                fetch_page(&mut client, selection, cp).await
            };
            match batch {
                Ok(batch) => {
                    result.records.extend(batch.records);
                    result.warnings.extend(batch.warnings);
                    if !matches!(batch.progress, SyncProgress::Complete)
                        && !matches!(
                            result.progress,
                            SyncProgress::Partial {
                                reason: super::base::SyncPause::RateLimited,
                                ..
                            }
                        )
                    {
                        result.progress = batch.progress;
                    }
                }
                Err(error) if crate::connectors::clients::retryable(&error) => {
                    result.progress = SyncProgress::interrupted(&error);
                    break;
                }
                Err(error) => return Err(error),
            }
        }
        checkpoints.retain(|key, _| selections.iter().any(|s| &s.key() == key));
        let cursor = serde_json::to_string(&checkpoints)
            .map_err(|_| AppError::internal("Could not save Composio progress."))?;
        if cursor.len() > 2 * 1024 * 1024 {
            return Err(AppError::internal(
                "Composio progress exceeded its limit. The previous checkpoint is kept.",
            ));
        }
        result.cursor = Some(cursor);
        Ok(result)
    }
    fn map(&self, raw: &Value) -> Option<Value> {
        let external = raw["external"].as_str()?;
        let mut env = envelope(
            "composio",
            s(raw, "/type"),
            external,
            s(raw, "/title"),
            s(raw, "/body"),
            super::base::rfc3339(s(raw, "/date")),
            s(raw, "/url"),
            json!({"app":raw["app"],"account":raw["account"],"document_id":raw["document_id"],"frontmatter":raw["frontmatter"]}),
        );
        env["links"] = raw.get("links").cloned().unwrap_or(json!([]));
        env["deleted"] = json!(raw["deleted"] == true);
        Some(env)
    }
}
fn wrap(sel: &Selection, kind: &str, env: Value) -> Value {
    json!({"external":format!("{}:{}:{}",sel.toolkit,sel.account,s(&env,"/external_id")),"type":format!("composio.{kind}"),"app":sel.toolkit,"account":sel.account,"title":env["title"],"body":env["body_text"],"date":env["occurred_at"],"url":env["url"],"deleted":env["deleted"]})
}
fn record(
    sel: &Selection,
    kind: &str,
    id: &str,
    title: &str,
    body: &str,
    date: &str,
    url: &str,
) -> Value {
    wrap(
        sel,
        kind,
        envelope(
            "composio",
            kind,
            id,
            title,
            body,
            Some(date.into()),
            url,
            json!({}),
        ),
    )
}
fn protocol() -> AppError {
    tools::failure(Failure::Protocol)
}

async fn fetch_page(
    client: &mut ReadClient,
    sel: &Selection,
    cp: &mut Checkpoint,
) -> AppResult<SyncResult> {
    let mut batch = SyncResult::default();
    match sel.toolkit.as_str() {
        "googledrive" => {
            if cp.until.is_empty() {
                cp.until = chrono::Utc::now().to_rfc3339();
            }
            if cp.pending.is_empty() {
                let mut q = format!("trashed = false and modifiedTime < '{}'", cp.until);
                if !cp.since.is_empty() {
                    q.push_str(&format!(" and modifiedTime >= '{}'", cp.since));
                }
                let mut args = json!({"q":q,"pageSize":25,"orderBy":"modifiedTime","fields":"nextPageToken,incompleteSearch,files(id,name,mimeType,modifiedTime,webViewLink)"});
                if !sel.folder.is_empty() {
                    args["folder_id"] = json!(sel.folder);
                }
                if !cp.page.is_empty() {
                    args["pageToken"] = json!(cp.page);
                }
                let page = client.execute(sel, "GOOGLEDRIVE_FIND_FILE", args).await?;
                if page["incompleteSearch"] == true {
                    return Err(protocol());
                }
                cp.pending = page["files"].as_array().ok_or_else(protocol)?.clone();
                cp.pending.reverse();
                cp.page = s(&page, "/nextPageToken").into();
            }
            for _ in 0..25 {
                let Some(file) = cp.pending.last().cloned() else {
                    break;
                };
                let mime = s(&file, "/mimeType");
                let export = match mime {
                    "application/vnd.google-apps.document" => Some("text/plain"),
                    "application/vnd.google-apps.spreadsheet" => Some("text/csv"),
                    _ => None,
                };
                if export.is_none()
                    && crate::documents::media_type(s(&file, "/name"), Some(mime)).is_none()
                {
                    cp.pending.pop();
                    batch.warnings.push("Drive skipped an unsupported file type. Text, Markdown, JSON, CSV, PDF, Docs and Sheets are supported.".into());
                    continue;
                }
                let data = match export {
                    Some(mime) => {
                        client
                            .execute(
                                sel,
                                "GOOGLEDRIVE_EXPORT_GOOGLE_WORKSPACE_FILE",
                                json!({"fileId":file["id"],"mimeType":mime}),
                            )
                            .await
                    }
                    None => {
                        client
                            .execute(
                                sel,
                                "GOOGLEDRIVE_DOWNLOAD_FILE",
                                json!({"fileId":file["id"]}),
                            )
                            .await
                    }
                };
                let data = match data {
                    Ok(v) => v,
                    Err(e) if crate::connectors::clients::retryable(&e) => {
                        batch.progress = SyncProgress::interrupted(&e);
                        break;
                    }
                    Err(e) => return Err(e),
                };
                let link = if export.is_some() {
                    s(&data, "/file/s3url")
                } else {
                    s(&data, "/downloaded_file_content/s3url")
                };
                let mut bytes = match download(link).await {
                    Ok(b) => b,
                    Err(e) => {
                        batch.progress = SyncProgress::interrupted(&e);
                        break;
                    }
                };
                client.mcp.scrub_file(&mut bytes);
                let media = export.unwrap_or(mime);
                let text = crate::documents::extract_text(media, bytes.clone())
                    .await
                    .map_err(|_| protocol())?;
                let mut raw = record(
                    sel,
                    "file",
                    s(&file, "/id"),
                    s(&file, "/name"),
                    &text,
                    s(&file, "/modifiedTime"),
                    s(&file, "/webViewLink"),
                );
                let name = match export {
                    Some("text/csv") => format!("{}.csv", s(&file, "/name")),
                    Some(_) => format!("{}.txt", s(&file, "/name")),
                    None => s(&file, "/name").into(),
                };
                raw["document"] = json!({"filename":name,"media_type":media,"base64":base64::engine::general_purpose::STANDARD.encode(bytes)});
                if mime == "text/markdown" {
                    note_relations(sel, &mut raw, &text, s(&file, "/name"));
                }
                batch.records.push(raw);
                cp.pending.pop();
            }
            if cp.pending.is_empty() && cp.page.is_empty() {
                cp.since = std::mem::take(&mut cp.until);
            } else if matches!(batch.progress, SyncProgress::Complete) {
                batch.progress = SyncProgress::remaining();
            }
        }
        "googlecalendar" => {
            let mut args = json!({"calendarId":"primary","maxResults":100,"showDeleted":true});
            if !cp.calendar_sync.is_empty() {
                args["syncToken"] = json!(cp.calendar_sync);
            }
            if !cp.page.is_empty() {
                args["pageToken"] = json!(cp.page);
            }
            let page = match client
                .execute(sel, "GOOGLECALENDAR_EVENTS_LIST", args)
                .await
            {
                Err(e) if e.reason == Some("connector_removed") => {
                    cp.calendar_sync.clear();
                    cp.page.clear();
                    batch.progress = SyncProgress::remaining();
                    return Ok(batch);
                }
                other => other?,
            };
            for event in page["items"].as_array().ok_or_else(protocol)? {
                let mapped = super::google_calendar::GoogleCalendarSource
                    .map(event)
                    .ok_or_else(protocol)?;
                batch.records.push(wrap(sel, "event", mapped));
            }
            cp.page = s(&page, "/nextPageToken").into();
            if cp.page.is_empty() {
                cp.calendar_sync = s(&page, "/nextSyncToken").into();
            } else {
                batch.progress = SyncProgress::remaining();
            }
        }
        "github" => {
            let (owner, repo) = sel.repo.split_once('/').ok_or_else(protocol)?;
            if cp.pending.is_empty() {
                let tree=client.execute(sel,"GITHUB_GET_A_TREE",json!({"owner":owner,"repo":repo,"tree_sha":sel.reference,"recursive":"true"})).await?;
                if tree["truncated"] != false {
                    return Err(AppError::bad_request(
                        "The Obsidian repository tree is too large. Choose a smaller vault repository.",
                    ));
                }
                cp.until = s(&tree, "/sha").into();
                cp.pending = tree["tree"]
                    .as_array()
                    .ok_or_else(protocol)?
                    .iter()
                    .filter(|f| {
                        s(f, "/type") == "blob"
                            && s(f, "/path").ends_with(".md")
                            && (sel.folder.is_empty()
                                || s(f, "/path")
                                    .starts_with(&format!("{}/", sel.folder.trim_end_matches('/'))))
                    })
                    .cloned()
                    .collect();
                cp.pending.reverse();
            }
            for _ in 0..25 {
                let Some(file) = cp.pending.last() else { break };
                let path = s(file, "/path").to_string();
                let output = client
                    .execute(
                        sel,
                        "GITHUB_GET_REPOSITORY_CONTENT",
                        json!({"owner":owner,"repo":repo,"ref":cp.until,"path":path}),
                    )
                    .await;
                let output = match output {
                    Ok(v) => v,
                    Err(e) if crate::connectors::clients::retryable(&e) => {
                        batch.progress = SyncProgress::interrupted(&e);
                        break;
                    }
                    Err(e) => return Err(e),
                };
                let content = &output["content"];
                if s(content, "/encoding") != "base64" {
                    return Err(protocol());
                }
                let bytes =
                    crate::documents::tool_bytes(None, Some(s(content, "/content").into()))?;
                let text = String::from_utf8(bytes).map_err(|_| protocol())?;
                let mut raw = record(
                    sel,
                    "note",
                    &format!("{}:{path}", sel.repo),
                    &path,
                    &text,
                    "",
                    s(content, "/html_url"),
                );
                note_relations(sel, &mut raw, &text, &path);
                batch.records.push(raw);
                cp.pending.pop();
            }
            if !cp.pending.is_empty() && matches!(batch.progress, SyncProgress::Complete) {
                batch.progress = SyncProgress::remaining();
            }
        }
        "timelinesai" => {
            if cp.chats.is_empty() {
                let page=client.execute(sel,"TIMELINESAI_GET_CHATS",json!({"limit":50,"offset":cp.chat_offset,"sort_field":"createdAt","sort_direction":"asc"})).await?;
                let chats = page["results"].as_array().ok_or_else(protocol)?;
                cp.chats = chats.iter().map(|v| s(v, "/id").to_string()).collect();
                cp.chats.reverse();
                if page["has_more_pages"] == true {
                    cp.chat_offset += chats.len();
                } else {
                    cp.chat_offset = 0;
                }
            }
            if let Some(chat) = cp.chats.last() {
                let page = client
                    .execute(
                        sel,
                        "TIMELINESAI_GET_CHAT_MESSAGES",
                        json!({"chat_id":chat,"limit":100,"offset":cp.message_offset}),
                    )
                    .await?;
                let messages = page["messages"].as_array().ok_or_else(protocol)?;
                for m in messages {
                    batch.records.push(record(
                        sel,
                        "message",
                        &format!("{chat}:{}", s(m, "/id")),
                        &format!("WhatsApp chat {chat}"),
                        s(m, "/text"),
                        s(m, "/timestamp"),
                        "",
                    ));
                }
                if messages.len() == 100 || !s(&page, "/next_cursor").is_empty() {
                    cp.message_offset += messages.len();
                } else {
                    cp.chats.pop();
                    cp.message_offset = 0;
                }
            }
            if !cp.chats.is_empty() || cp.chat_offset > 0 {
                batch.progress = SyncProgress::remaining();
            }
        }
        "whatsapp" => {
            let mut args = json!({"phone_number_id":sel.phone_number_id,"limit":100,"fields":"id,message_id,events{delivery_status,timestamp,webhook_update_state}"});
            if !cp.page.is_empty() {
                args["after"] = json!(cp.page);
            }
            let page = client
                .execute(sel, "WHATSAPP_GET_MESSAGE_HISTORY", args)
                .await?;
            for m in page["data"].as_array().ok_or_else(protocol)? {
                batch.records.push(record(
                    sel,
                    "delivery",
                    s(m, "/id"),
                    "WhatsApp Business delivery history",
                    &m.to_string(),
                    "",
                    "",
                ));
            }
            cp.page = if page.pointer("/paging/next").is_some() {
                s(&page, "/paging/cursors/after").into()
            } else {
                String::new()
            };
            if !cp.page.is_empty() {
                batch.progress = SyncProgress::remaining();
            }
        }
        _ => return Err(protocol()),
    }
    Ok(batch)
}

/// Signed Composio file URLs are fetched without the OAuth bearer or MCP session. No redirects.
async fn download(raw: &str) -> AppResult<Vec<u8>> {
    let url = url::Url::parse(raw).map_err(|_| protocol())?;
    let host = url.host_str().unwrap_or("");
    let mock = crate::connectors::composio::test_origin()
        .is_some_and(|o| url.origin().ascii_serialization() == o);
    if !mock
        && (url.scheme() != "https"
            || !(host.ends_with(".s3.amazonaws.com")
                || host.ends_with(".amazonaws.com")
                || host == "s3.amazonaws.com"
                || host.ends_with(".composio.dev")))
    {
        return Err(protocol());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(protocol());
    }
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(60));
    if !mock {
        let addresses: Vec<_> = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            tokio::net::lookup_host((host, url.port_or_known_default().unwrap_or(443))),
        )
        .await
        .map_err(|_| protocol())?
        .map_err(|_| protocol())?
        .collect();
        if addresses.is_empty()
            || addresses
                .iter()
                .any(|a| crate::oauth::cimd::ip_blocked(a.ip()))
        {
            return Err(protocol());
        }
        builder = builder.resolve_to_addrs(host, &addresses);
    }
    let http = builder.build().map_err(|_| protocol())?;
    let mut response = http
        .get(url)
        .send()
        .await
        .map_err(|_| tools::failure(Failure::Upstream))?;
    if !response.status().is_success() {
        return Err(tools::failure(Failure::Upstream));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| tools::failure(Failure::Upstream))?
    {
        if bytes.len() + chunk.len() > 16 * 1024 * 1024 {
            return Err(protocol());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

struct GmailBridge<'a> {
    client: Mutex<&'a mut ReadClient>,
    selection: &'a Selection,
    next_read: Mutex<tokio::time::Instant>,
}
#[async_trait]
impl super::gmail::GmailReader for GmailBridge<'_> {
    async fn get(&self, path: &str, query: &[(&str, String)]) -> AppResult<Value> {
        let mut args = json!({"user_id":"me"});
        let slug = match path {
            "/users/me/profile" => "GMAIL_GET_PROFILE",
            "/users/me/messages" => {
                args["ids_only"] = json!(true);
                "GMAIL_FETCH_EMAILS"
            }
            "/users/me/history" => "GMAIL_LIST_HISTORY",
            p if p.starts_with("/users/me/messages/") => {
                args["message_id"] = json!(p.trim_start_matches("/users/me/messages/"));
                "GMAIL_FETCH_MESSAGE_BY_MESSAGE_ID"
            }
            _ => return Err(protocol()),
        };
        for (key, val) in query {
            let key = match *key {
                "q" => "query",
                "maxResults" => "max_results",
                "pageToken" => "page_token",
                "startHistoryId" => "start_history_id",
                k => k,
            };
            args[key] = if key == "max_results" {
                json!(val.parse::<u64>().map_err(|_| protocol())?)
            } else {
                json!(val)
            };
        }
        if slug == "GMAIL_FETCH_MESSAGE_BY_MESSAGE_ID" {
            let mut next = self.next_read.lock().await;
            tokio::time::sleep_until(*next).await;
            *next = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
        }
        let mut data = self
            .client
            .lock()
            .await
            .execute(self.selection, slug, args)
            .await?;
        if slug == "GMAIL_FETCH_EMAILS" {
            for m in data["messages"].as_array_mut().ok_or_else(protocol)? {
                if m.get("id").is_none() {
                    m["id"] = m["messageId"].clone();
                }
            }
        }
        if slug == "GMAIL_FETCH_MESSAGE_BY_MESSAGE_ID" {
            data["id"] = data["messageId"].clone();
            data["snippet"] = data["messageText"].clone();
            let date = s(&data, "/messageTimestamp");
            let ms = chrono::DateTime::parse_from_rfc3339(date)
                .map(|d| d.timestamp_millis())
                .or_else(|_| date.parse::<i64>())
                .map_err(|_| protocol())?;
            data["internalDate"] = json!(ms.to_string());
            if data.pointer("/payload/headers").is_none() {
                data["payload"] = json!({"headers":[{"name":"Subject","value":data["subject"]},{"name":"From","value":data["sender"]},{"name":"To","value":data["to"]}]});
            }
        }
        Ok(data)
    }
}

fn note_relations(sel: &Selection, raw: &mut Value, text: &str, path: &str) {
    let front = text
        .strip_prefix("---\n")
        .and_then(|s| s.split_once("\n---"))
        .map(|(f, _)| f)
        .unwrap_or("");
    // Keep arbitrary front matter intact; only explicit wikilinks become relations.
    raw["frontmatter"] = json!(front);
    let re =
        regex::Regex::new(r"\[\[([^\]|#]+)(?:[^\]]*)\]\]").expect("static wikilink expression");
    let mut links:Vec<_>=re.captures_iter(text).map(|c|{
        let target=c[1].trim();
        let parent=path.rsplit_once('/').map(|(p,_)|format!("{p}/")).unwrap_or_default();
        let target=if target.contains('/') {target.to_string()}else{format!("{parent}{target}")};
        let target=if target.ends_with(".md"){target}else{format!("{target}.md")};
        let id=if sel.toolkit=="github"{format!("{}:{target}",sel.repo)}else{target};
        json!({"target":format!("composio:composio.note:{}:{}:{id}",sel.toolkit,sel.account),"rel":"wikilink"})
    }).collect();
    let mut field = "";
    for line in front.lines() {
        let value =
            if let Some((key, value)) = line.split_once(':').filter(|_| !line.starts_with(' ')) {
                field = key.trim();
                value.trim()
            } else if line.trim_start().starts_with("- ") {
                line.trim().trim_start_matches("- ")
            } else {
                continue;
            };
        if !matches!(field, "parent" | "related" | "links" | "tags") || value.contains("[[") {
            continue;
        }
        for target in value
            .trim_matches(['[', ']'])
            .split(',')
            .map(|s| s.trim().trim_matches(['\'', '"']))
            .filter(|s| !s.is_empty())
        {
            let id = if field == "tags" {
                format!("tag:{target}")
            } else {
                let path = if target.ends_with(".md") {
                    target.to_string()
                } else {
                    format!("{target}.md")
                };
                if sel.toolkit == "github" {
                    format!("{}:{path}", sel.repo)
                } else {
                    path
                }
            };
            links.push(json!({"target":format!("composio:composio.note:{}:{}:{id}",sel.toolkit,sel.account),"rel":format!("frontmatter:{field}")}));
        }
    }
    links.sort_by_key(Value::to_string);
    links.dedup();
    raw["links"] = json!(links);
}

/// Publish Drive bytes through the existing document service. Retry by stable cache identity.
pub async fn store_documents(
    state: &crate::state::OrgState,
    owner: &surrealdb::types::RecordId,
    row: &crate::connectors::service::Connector,
    records: &mut [Value],
) -> AppResult<()> {
    for raw in records {
        let Some(document) = raw.get("document").cloned() else {
            continue;
        };
        let env = ComposioSource.map(raw).ok_or_else(protocol)?;
        let external = s(&env, "/external_id");
        let identity = surrealdb::types::RecordId::from_table_key(
            "composio_file",
            crate::documents::sha256_hex(
                format!("{}:{}:{external}", owner.to_string(), row.id.to_string()).as_bytes(),
            ),
        );
        let existing: Option<Value> = crate::store::get(&state.db, &identity).await?;
        let previous = existing
            .as_ref()
            .and_then(|v| v.get("document_id"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let bytes = crate::documents::tool_bytes(None, Some(s(&document, "/base64").into()))?;
        let old = match &previous {
            Some(id) => crate::documents::summary(&state.db, id).await?,
            None => None,
        };
        let doc = if let Some(old) =
            old.filter(|d| d.sha256 == crate::documents::sha256_hex(&bytes) && d.status != "failed")
        {
            old
        } else {
            crate::documents::upload(
                state,
                owner,
                crate::documents::Upload {
                    filename: s(&document, "/filename").into(),
                    media_type: Some(s(&document, "/media_type").into()),
                    bytes,
                    replace: previous,
                    vault: row.routing["default_vault"].as_str().map(str::to_string),
                },
            )
            .await?
        };
        crate::store::dynamic(&state.db,"app.composio_file_link","UPSERT $id SET owner = $owner, connector = $connector, external_id = $external, document_id = $document")
            .bind(("id",identity)).bind(("owner",owner.clone())).bind(("connector",row.id.clone())).bind(("external",external.to_string())).bind(("document",doc.id.clone())).await?.check()?;
        raw["document_id"] = json!(doc.id);
        raw.as_object_mut().ok_or_else(protocol)?.remove("document");
    }
    Ok(())
}

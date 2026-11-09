//! Spotify source: recently played tracks, via
//! `GET /me/player/recently-played` (Spotify keeps only the last 50 plays,
//! so polling regularly is what builds up history). Spotify has no personal
//! tokens: the user brings their own app's `client_id` + `client_secret` and
//! a `refresh_token` with scope `user-read-recently-played` (see
//! docs/connectors.md); each sync refreshes the access token first.
//!
//! Incremental: `after=` the `cursors.after` millisecond timestamp Spotify
//! returned last time.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::connectors::clients::{bearer, refresh_access_token, Api};
use crate::error::AppResult;
use crate::sources::base::{envelope, items, rfc3339, s, Conn, Source, SyncResult};

const BASE_URL: &str = "https://api.spotify.com/v1";
const TOKEN_URL: &str = "https://accounts.spotify.com/api/token";

async fn api(conn: &Conn) -> AppResult<Api> {
    let token = refresh_access_token(&conn.config, TOKEN_URL, &conn.credentials).await?;
    Ok(Api::new(&conn.config, BASE_URL, bearer(&token)))
}

pub struct SpotifySource;

#[async_trait]
impl Source for SpotifySource {
    fn key(&self) -> &'static str {
        "spotify"
    }

    fn label(&self) -> &'static str {
        "Spotify"
    }

    fn record_types(&self) -> &'static [&'static str] {
        &["spotify.play"]
    }

    async fn check(&self, conn: &Conn) -> AppResult<()> {
        api(conn).await?.get("/me", &[]).await.map(|_| ())
    }

    async fn fetch(&self, conn: &Conn, cursor: Option<String>) -> AppResult<SyncResult> {
        let mut query = vec![("limit", "50".to_string())];
        if let Some(after) = &cursor {
            query.push(("after", after.clone()));
        }
        let page = api(conn).await?.get("/me/player/recently-played", &query).await?;
        let next = page.pointer("/cursors/after").and_then(|v| v.as_str()).map(String::from);
        Ok(SyncResult { records: items(&page, "/items"), cursor: next.or(cursor), ..SyncResult::default() })
    }

    fn map(&self, raw: &Value) -> Option<Value> {
        let played_at = raw.get("played_at").and_then(|v| v.as_str())?;
        let track = &raw["track"];
        let name = s(track, "/name");
        let artists = items(track, "/artists").iter().map(|a| s(a, "/name").to_string()).collect::<Vec<_>>().join(", ");
        let album = s(track, "/album/name");
        Some(envelope(
            "spotify",
            "spotify.play",
            &format!("{played_at}:{}", s(track, "/id")),
            &format!("{name} · {artists}"),
            &format!("Played {name} by {artists} from {album}"),
            rfc3339(played_at),
            s(track, "/external_urls/spotify"),
            json!({
                "track": name,
                "artists": artists,
                "album": album,
                "duration_ms": track.get("duration_ms"),
                "context": raw.pointer("/context/type"),
            }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::mock::{assert_fetch_fails, envelopes, route, serve};

    #[tokio::test]
    async fn fetch_refreshes_then_reads_plays_after_the_cursor() {
        let mock = serve(vec![
            route("POST", "/oauth/token", json!({"access_token": "BQ-fresh", "token_type": "Bearer", "expires_in": 3600, "scope": "user-read-recently-played"})),
            route("GET", "/me/player/recently-played", json!({
                "items": [{
                    "track": {"id": "4uLU6hMCjMI75M1A2tKUQC", "name": "Never Gonna Give You Up", "duration_ms": 213573,
                              "artists": [{"name": "Rick Astley"}], "album": {"name": "Whenever You Need Somebody"},
                              "external_urls": {"spotify": "https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC"}},
                    "played_at": "2024-06-01T08:30:00.123Z", "context": {"type": "playlist"},
                }],
                "next": null, "cursors": {"after": "1717230600123", "before": "1717230600123"}, "limit": 50,
            }))
            .query_has("after=1717000000000"),
        ])
        .await;

        let creds = json!({"client_id": "cid", "client_secret": "s", "refresh_token": "AQ-rt"});
        let res = SpotifySource.fetch(&mock.conn(creds), Some("1717000000000".into())).await.unwrap();
        assert_eq!(res.cursor.as_deref(), Some("1717230600123"));
        assert_eq!(mock.requests()[1].header("authorization"), "Bearer BQ-fresh");

        let envs = envelopes(&SpotifySource, &res.records);
        assert_eq!(envs[0].title, "Never Gonna Give You Up · Rick Astley");
        assert_eq!(envs[0].url, "https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC");
    }

    #[tokio::test]
    async fn fetch_errors_are_visible() {
        let creds = json!({"client_id": "cid", "client_secret": "s", "refresh_token": "rt"});
        assert_fetch_fails(&SpotifySource, 401, "/me/player/recently-played", creds.clone(), "HTTP 401").await;
        assert_fetch_fails(&SpotifySource, 429, "/me/player/recently-played", creds.clone(), "HTTP 429").await;
        assert_fetch_fails(&SpotifySource, 500, "/me/player/recently-played", creds, "HTTP 500").await;
    }
}

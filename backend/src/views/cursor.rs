//! Opaque, query-bound keyset boundaries. Record IDs are immutable, unlike names or hop counts.
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use crate::error::{AppError, AppResult};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Cursor {
    pub key: String,
    pub page: usize,
    scope: String,
}
pub(super) fn scope(value: &impl Serialize) -> String {
    hex::encode(Sha256::digest(serde_json::to_vec(value).expect("serialisable selection")))
}
impl Cursor {
    pub fn decode(raw: Option<&str>) -> AppResult<Option<Self>> {
        let Some(raw) = raw else { return Ok(None) };
        let bad = || AppError::bad_request("invalid cursor for this selection; start again without a cursor");
        if raw.len() > 4096 { return Err(bad()); }
        let bytes = URL_SAFE_NO_PAD.decode(raw).map_err(|_| bad())?;
        let cursor: Self = serde_json::from_slice(&bytes).map_err(|_| bad())?;
        if cursor.page == 0 || cursor.page > 1_000_000 || cursor.key.is_empty() || cursor.key.len() > 512
            || cursor.scope.len() != 64 || !cursor.scope.bytes().all(|b| b.is_ascii_hexdigit()) { return Err(bad()); }
        Ok(Some(cursor))
    }
    pub fn read(raw: Option<&str>, scope: &str) -> AppResult<Option<Self>> {
        let cursor = Self::decode(raw)?;
        if cursor.as_ref().is_some_and(|c| c.scope != scope) {
            return Err(AppError::bad_request("invalid cursor for this selection; start again without a cursor"));
        }
        Ok(cursor)
    }
    pub fn next(key: String, page: usize, scope: String) -> String {
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&Self { key, page, scope }).expect("serialisable cursor"))
    }
}

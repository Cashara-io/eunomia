//! `RecordId` helpers. SurrealDB 3.x's `RecordId` is a plain `{ table, key }` struct with no
//! `Display`, `FromStr`, `key()` or `table()`; this restores the 2.x spellings the app uses.
//! `to_string` is `table:key` (the key backtick-escaped when it needs it), so ids look as before.

use surrealdb::types::{RecordId, RecordIdKey, ToSql};

pub trait RecordIdExt {
    fn from_table_key(table: &str, key: impl Into<RecordIdKey>) -> RecordId;
    fn key(&self) -> &RecordIdKey;
    fn table(&self) -> &str;
    fn to_string(&self) -> String;
}

impl RecordIdExt for RecordId {
    fn from_table_key(table: &str, key: impl Into<RecordIdKey>) -> RecordId {
        RecordId::new(table, key)
    }

    fn key(&self) -> &RecordIdKey {
        &self.key
    }

    fn table(&self) -> &str {
        self.table.as_str()
    }

    fn to_string(&self) -> String {
        self.to_sql()
    }
}

/// Parse `table:key`. The key is always a string key (every id this app mints is one).
#[allow(clippy::result_unit_err)] // every caller maps the failure to its own not-found error
pub fn parse(s: &str) -> Result<RecordId, ()> {
    let s = if let Some(inner) = s.strip_prefix("r'") {
        inner.strip_suffix('\'').ok_or(())?
    } else if let Some(inner) = s.strip_prefix("r\"") {
        inner.strip_suffix('"').ok_or(())?
    } else { s };
    RecordId::parse_simple(s).map_err(|_| ())
}

/// The key as a string when it is a string key.
pub fn key_string(key: &RecordIdKey) -> Option<String> {
    match key {
        RecordIdKey::String(s) => Some(s.clone()),
        _ => None,
    }
}

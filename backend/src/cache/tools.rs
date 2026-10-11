//! `recall`/`reflect` tool-facing wrappers, owner-scoped.
//!
//! The tool registration lives in `tools::registry`. This module holds the logic
//! on top of `recall`/`reflect`: coercing a `[since, until]` JSON array into the
//! `(since, until)` tuple `cache::recall::recall` expects, and shaping the
//! `recall` response as `{"results": [...]}`.
//!

use serde_json::{json, Value};
use surrealdb::types::RecordId;

use crate::cache::recall::{self, MemoryType};
use crate::cache::reflect;
use crate::config::Settings;
use crate::pool::OrgDb;
use crate::error::{AppError, AppResult};

/// Coerces a 2-element `[since, until]` JSON array into a `(since, until)` tuple; any other length
/// is a 400 rather than a silently ignored range.
fn time_range_tuple(time_range: Option<&[String]>) -> AppResult<Option<(&str, &str)>> {
    match time_range {
        None => Ok(None),
        Some([since, until]) => Ok(Some((since.as_str(), until.as_str()))),
        Some(_) => Err(AppError::bad_request("time_range must be exactly two ISO 8601 dates: [since, until]")),
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn recall_tool(
    db: &OrgDb,
    settings: &Settings,
    owner: &RecordId,
    query: &str,
    time_range: Option<&[String]>,
    limit: usize,
    max_tokens: Option<usize>,
    types: Option<&[MemoryType]>,
    vault_ids: &[RecordId],
) -> AppResult<Value> {
    let results = recall::recall(db, settings, owner, query, time_range_tuple(time_range)?, limit, max_tokens, types, vault_ids).await?;
    Ok(json!({ "results": results }))
}

pub async fn reflect_tool(
    db: &OrgDb,
    settings: &Settings,
    owner: &RecordId,
    query: &str,
    vault_ids: &[RecordId],
    limit: usize,
) -> AppResult<Value> {
    reflect::reflect(db, settings, owner, query, vault_ids, limit).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_range_tuple_converts_two_element_array() {
        let v = vec!["2024-01-01".to_string(), "2024-02-01".to_string()];
        assert_eq!(time_range_tuple(Some(&v)).unwrap(), Some(("2024-01-01", "2024-02-01")));
    }

    #[test]
    fn time_range_tuple_is_none_when_absent_and_an_error_when_the_wrong_length() {
        assert_eq!(time_range_tuple(None).unwrap(), None);
        let one = vec!["2024-01-01".to_string()];
        assert!(time_range_tuple(Some(&one)).is_err());
    }
}

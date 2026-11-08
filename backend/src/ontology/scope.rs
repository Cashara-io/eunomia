//! Which vault a question is about when the caller names none. `ontology_coverage` checks every vault
//! the caller can read and answers from the best match; `ontology_get`, `metric_eval` and
//! `scenario_run` still read the default vault, but when what they were asked for is not there they
//! name the readable vaults that have it, so the next call can pass `vault_id`. Only vaults the
//! credential may read are checked or named.

use std::future::Future;

use surrealdb::types::RecordId;

use crate::error::{AppError, AppResult};
use crate::pool::OrgDb;
use crate::rid::RecordIdExt;
use crate::vaults::service as vaults;

/// A vault the caller can read, with its name.
#[derive(Debug, Clone)]
pub(crate) struct Readable {
    pub id: RecordId,
    pub name: String,
}

/// The vaults the caller can read: the default vault first (the personal one, or a vault-restricted
/// credential's own), then every org vault they are an active member of. Without a Personal default,
/// only the org vaults are included. `read_scope` already supplies this order.
pub(crate) async fn readable(db: &OrgDb, owner: &RecordId) -> AppResult<Vec<Readable>> {
    let scope = vaults::read_scope(db, owner, &[]).await?;
    Ok(scope
        .vaults
        .iter()
        .map(|v| Readable { id: v.clone(), name: scope.names.get(&v.to_string()).cloned().unwrap_or_default() })
        .collect())
}

/// The readable vaults other than `default` for which `has` is true.
pub(crate) async fn elsewhere<F, Fut>(db: &OrgDb, owner: &RecordId, default: &RecordId, has: F) -> AppResult<Vec<Readable>>
where
    F: Fn(RecordId) -> Fut,
    Fut: Future<Output = AppResult<bool>>,
{
    let mut found = Vec::new();
    let all = readable(db, owner).await?;
    for v in all.iter().filter(|v| &v.id != default) {
        if has(v.id.clone()).await? {
            found.push(v.clone());
        }
    }
    // a name two found vaults share would not resolve, so those are given by id
    let named = |v: &Readable| {
        let shared = v.name.is_empty() || all.iter().filter(|o| o.name.eq_ignore_ascii_case(&v.name)).count() > 1;
        Readable { id: v.id.clone(), name: if shared { v.id.to_string() } else { v.name.clone() } }
    };
    Ok(found.iter().map(named).collect())
}

/// `original` with where `what` is instead: "avg_price is in Northside Bikes: pass vault_id
/// \"Northside Bikes\"". `original` unchanged when no other vault has it.
pub(crate) fn found_in(original: AppError, what: &str, homes: &[Readable]) -> AppError {
    let (first, rest) = match homes {
        [] => return original,
        [first, rest @ ..] => (first, rest),
    };
    let names: Vec<&str> = homes.iter().map(|h| h.name.as_str()).collect();
    let pass = if rest.is_empty() { format!("pass vault_id \"{}\"", first.name) } else { "pass vault_id with the one meant".to_string() };
    let message = format!("{}. {what} is in {}: {pass}.", original.message.trim_end_matches('.'), names.join(", "));
    AppError { message, ..original }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;

    fn home(name: &str) -> Readable {
        Readable { id: RecordId::new("vault", name.to_lowercase().replace(' ', "_")), name: name.to_string() }
    }

    #[test]
    fn the_error_keeps_what_went_wrong_and_names_where_it_is() {
        let e = AppError::coded(ErrorCode::ResourceNotFound, "no metric \"avg_price\" in this vault");
        let out = found_in(e, "avg_price", &[home("Northside Bikes")]);
        assert_eq!(out.message, "no metric \"avg_price\" in this vault. avg_price is in Northside Bikes: pass vault_id \"Northside Bikes\".");
        assert_eq!(out.code, ErrorCode::ResourceNotFound);
        let two = found_in(AppError::bad_request("nope."), "it", &[home("A"), home("B")]);
        assert_eq!(two.message, "nope. it is in A, B: pass vault_id with the one meant.");
        assert_eq!(found_in(AppError::bad_request("nope"), "it", &[]).message, "nope");
    }
}

//! The `subject_erase` job: removes or anonymises every category of the inventory in
//! docs/privacy/subject-rights.md for one person, then proves it with a post-check.
//!
//! Order, per org the person belongs to (each step is idempotent, so a retry re-derives what is left):
//!
//! 1. Spaces other people can still read: what the person wrote there stays and is reassigned to an
//!    owner who remains (the longest-standing member is made an owner if the person was the only
//!    one); their email is replaced in the text of facts and entities there.
//! 2. Spaces only the person could read: their documents' files, then every row in them, then the
//!    spaces.
//! 3. Everything else the person owns (personal records, chat, sources, recordings, settings, the
//!    tool log, exports), their files in the bucket, the actor names in decision trails, and the
//!    org's embedding cache. Their space memberships go last.
//! 4. The control database: credentials, sessions, app connections, identities, jobs, usage
//!    counters and memberships are deleted, their audit events are kept but no longer name them, and
//!    the account row goes.
//! 5. Post-check: every table that can name a user is counted for the person (and their email), and
//!    every file that was theirs must be gone. Anything left fails the job, which retries.
//!
//! On success the `erasure` row becomes the completion record (counts and post-check, the user id
//! dropped) and a `subject.erase` audit event repeats it.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::Deserialize;
use serde_json::{json, Map, Value};
use surrealdb::types::{RecordId, SurrealValue};

use crate::error::{AppError, ErrorCode};
use crate::jobs::{Job, JobError};
use crate::pool::OrgId;
use crate::rid::RecordIdExt;
use crate::state::{AppState, OrgState};
use crate::store::subject as q;

#[derive(Debug, Deserialize, SurrealValue)]
struct ErasureRow {
    #[serde(default)]
    #[surreal(default)]
    subject: Option<RecordId>,
    status: String,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct MemberRow {
    vault: String,
    user: RecordId,
    #[serde(default)]
    #[surreal(default)]
    role: String,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct MyMembership {
    vault: String,
    #[serde(default)]
    #[surreal(default)]
    role: String,
    #[serde(default)]
    #[surreal(default)]
    status: String,
}

/// Adds every number in `from` to `into`, by key.
fn add_counts(into: &mut BTreeMap<String, i64>, from: &Value) {
    if let Some(obj) = from.as_object() {
        for (k, v) in obj {
            *into.entry(k.clone()).or_insert(0) += v.as_i64().unwrap_or(0);
        }
    }
}

/// The keys of a post-check object whose count is not 0.
pub fn residue(check: &Value) -> Vec<String> {
    check.as_object().map(|o| o.iter().filter(|(_, v)| v.as_i64().unwrap_or(0) != 0).map(|(k, _)| k.clone()).collect()).unwrap_or_default()
}

fn job_err(e: impl Into<AppError>) -> JobError {
    JobError::from(e.into())
}

/// A value no text contains (stands in for an email that is no longer known on a retry).
fn never_matches() -> String {
    format!("\u{0}unknown-{}", uuid::Uuid::new_v4().simple())
}

pub async fn erase_job(state: AppState, job: Job) -> Result<(), JobError> {
    let id_str = job.payload.get("erasure").and_then(Value::as_str).ok_or_else(|| JobError::permanent(ErrorCode::ValidationInvalid, "job payload has no erasure"))?;
    let erasure = crate::rid::parse(id_str).map_err(|_| JobError::permanent(ErrorCode::ValidationInvalid, "bad erasure id"))?;
    let row: Option<ErasureRow> = q::ERASURE_GET.on(&state.control).bind(("id", erasure.clone())).await.map_err(job_err)?.take(0).map_err(job_err)?;
    let Some(row) = row else { return Err(JobError::permanent(ErrorCode::ResourceNotFound, "erasure row is gone")) };
    if row.status == "done" {
        return Ok(());
    }
    let Some(subject) = row.subject else { return Err(JobError::permanent(ErrorCode::ValidationInvalid, "erasure has no subject")) };
    let orgs: Vec<(OrgId, RecordId)> = job
        .payload
        .get("orgs")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|o| Some((OrgId::parse(o.get("org")?.as_str()?)?, crate::rid::parse(o.get("heir")?.as_str()?).ok()?)))
                .collect()
        })
        .unwrap_or_default();
    q::ERASURE_RUNNING.on(&state.control).bind(("id", erasure.clone())).await.map_err(job_err)?.check().map_err(job_err)?;

    match erase_subject(&state, &subject, &orgs, &erasure).await {
        Ok((counts, check)) if residue(&check).is_empty() => {
            q::ERASURE_DONE
                .on(&state.control)
                .bind(("id", erasure.clone()))
                .bind(("counts", counts.clone()))
                .bind(("post_check", check))
                .await
                .map_err(job_err)?
                .check()
                .map_err(job_err)?;
            let actor = crate::authz::Caller::system().actor;
            let detail = serde_json::to_string(&counts).unwrap_or_default();
            // filed under the home org's heir, so that workspace's activity log (and only it) lists it
            let scope = orgs.first().map(|(_, heir)| heir);
            crate::audit::record_structured(
                &state.control,
                crate::audit::Event { user: scope, actor: &actor, action: "subject.erase", target: &erasure.to_string(), outcome: "ok", detail: &detail },
            )
            .await;
            Ok(())
        }
        Ok((_, check)) => {
            let left = residue(&check).join(", ");
            let msg = format!("the post-check still found the person in: {left}");
            q::ERASURE_FAILED.on(&state.control).bind(("id", erasure)).bind(("error", msg.clone())).bind(("post_check", check)).await.map_err(job_err)?;
            Err(JobError::retryable(ErrorCode::Internal, msg))
        }
        Err(e) => {
            let msg = e.message.clone();
            q::ERASURE_FAILED.on(&state.control).bind(("id", erasure)).bind(("error", msg)).bind(("post_check", json!({}))).await.map_err(job_err)?;
            Err(JobError::retryable(e.code, e.source.unwrap_or(e.message)))
        }
    }
}

/// Erases `subject` from every org in `orgs` (each with the account that inherits org-level rows),
/// then from the control database. Returns the per-category counts and the post-check.
pub async fn erase_subject(state: &AppState, subject: &RecordId, orgs: &[(OrgId, RecordId)], erasure: &RecordId) -> Result<(Value, Value), AppError> {
    #[derive(Deserialize, SurrealValue)]
    struct EmailRow {
        #[serde(default)]
        #[surreal(default)]
        email: String,
    }
    let email_row: Option<EmailRow> = q::CONFIRM_ROW.on(&state.control).bind(("u", subject.clone())).await?.take(0)?;
    // a retry after the account row went: the email scrubs already ran in the same transaction
    let email = email_row.map(|r| r.email).filter(|e| !e.trim().is_empty());
    let (email_raw, email_lc) = match &email {
        Some(e) => (e.clone(), crate::models_user::normalize_email(e)),
        None => (never_matches(), never_matches()),
    };
    let mut actors: Vec<String> = q::TOKEN_IDS.on(&state.control).bind(("u", subject.clone())).await?.take(0)?;
    actors.push(subject.to_string());

    let mut counts: BTreeMap<String, i64> = BTreeMap::new();
    let mut check = Map::new();
    for (org, heir) in orgs {
        let org_state = state.org(org).await?;
        let (c, files, solo) = erase_in_org(&org_state, subject, heir, &actors, &email_raw, &email_lc).await?;
        add_counts(&mut counts, &c);
        let mut res = q::RESIDUE
            .on(&org_state.db)
            .bind(("u", subject.clone()))
            .bind(("actors", actors.clone()))
            .bind(("email", email_raw.clone()))
            .bind(("email_lc", email_lc.clone()))
            .bind(("solo", solo))
            .await?;
        let r: Value = res.take::<Option<Value>>(0)?.unwrap_or_default();
        add_into(&mut check, &r);
        // every file that was theirs is gone from the bucket
        let mut left = 0i64;
        if !state.settings.documents_backend.is_empty() {
            for path in &files {
                let exists: Option<bool> = q::OBJECT_EXISTS.on(&org_state.db).bind(("path", path.clone())).await?.take(0)?;
                left += i64::from(exists.unwrap_or(false));
            }
        }
        *check.entry("bucket_files").or_insert(json!(0)) = json!(check.get("bucket_files").and_then(Value::as_i64).unwrap_or(0) + left);
    }

    let mut res = q::ERASE_CONTROL
        .on(&state.control)
        .bind(("u", subject.clone()))
        .bind(("actors", actors.clone()))
        .bind(("email", email_raw.clone()))
        .bind(("email_lc", email_lc.clone()))
        .await?;
    let c: Value = res.take::<Option<Value>>(q::ERASE_CONTROL.slot)?.unwrap_or_default();
    add_counts(&mut counts, &c);
    state.credentials.forget_user(subject);

    let mut res = q::CONTROL_RESIDUE
        .on(&state.control)
        .bind(("u", subject.clone()))
        .bind(("actors", actors))
        .bind(("email_lc", email_lc))
        .bind(("erasure", erasure.clone()))
        .await?;
    let r: Value = res.take::<Option<Value>>(0)?.unwrap_or_default();
    add_into(&mut check, &r);
    Ok((json!(counts), Value::Object(check)))
}

fn add_into(into: &mut Map<String, Value>, from: &Value) {
    if let Some(obj) = from.as_object() {
        for (k, v) in obj {
            let n = into.get(k).and_then(Value::as_i64).unwrap_or(0) + v.as_i64().unwrap_or(0);
            into.insert(k.clone(), json!(n));
        }
    }
}

/// Steps 1 to 3 in one org. Returns the counts and the bucket paths that were the person's.
async fn erase_in_org(
    state: &OrgState,
    subject: &RecordId,
    heir: &RecordId,
    actors: &[String],
    email: &str,
    email_lc: &str,
) -> Result<(Value, Vec<String>, Vec<RecordId>), AppError> {
    let db = &state.db;
    let mut counts: BTreeMap<String, i64> = BTreeMap::new();

    // who else can read each space: owners first, then the longest-standing members
    let others: Vec<MemberRow> = q::OTHER_MEMBERS.on(db).bind(("u", subject.clone())).await?.take(0)?;
    let mut heir_of: HashMap<String, (RecordId, bool)> = HashMap::new();
    for m in others {
        heir_of.entry(m.vault).or_insert((m.user, m.role == "owner"));
    }
    let mine: Vec<MyMembership> = q::MEMBERSHIPS.on(db).bind(("u", subject.clone())).await?.take(0)?;
    let solo: Vec<RecordId> = mine
        .iter()
        .filter(|m| m.status == "active" && !heir_of.contains_key(&m.vault))
        .filter_map(|m| crate::rid::parse(&m.vault).ok())
        .collect();
    let kept: Vec<RecordId> = heir_of.keys().filter_map(|v| crate::rid::parse(v).ok()).collect();

    // 1. shared spaces: hand over ownership where the person was the only owner, then reassign their rows
    for m in mine.iter().filter(|m| m.status == "active" && m.role == "owner") {
        if let Some((to, false)) = heir_of.get(&m.vault) {
            let vault = crate::rid::parse(&m.vault).map_err(|_| AppError::internal("bad vault id"))?;
            q::PROMOTE.on(db).bind(("vault", vault)).bind(("to", to.clone())).await?.check()?;
            *counts.entry("ownership_handed_over".into()).or_insert(0) += 1;
        }
    }
    let with_rows: Vec<String> = q::VAULTS_WITH_ROWS.on(db).bind(("u", subject.clone())).await?.take::<Option<Vec<String>>>(0)?.unwrap_or_default();
    for v in with_rows {
        let Some((to, _)) = heir_of.get(&v) else { continue };
        let vault = crate::rid::parse(&v).map_err(|_| AppError::internal("bad vault id"))?;
        let mut res = crate::tx::with_retry(|| async {
            q::REASSIGN.on(db).bind(("u", subject.clone())).bind(("vault", vault.clone())).bind(("to", to.clone())).await
        })
        .await?;
        let n: Option<i64> = res.take(q::REASSIGN.slot)?;
        *counts.entry("reassigned_in_shared_spaces".into()).or_insert(0) += n.unwrap_or(0);
    }

    // the person's files: documents outside the shared spaces and their recordings' audio
    let mut res = q::OWNED_FILES.on(db).bind(("u", subject.clone())).bind(("kept", kept.clone())).await?;
    let mut files: Vec<String> = res.take::<Vec<Option<String>>>(0)?.into_iter().flatten().collect();
    files.extend(res.take::<Vec<Option<String>>>(1)?.into_iter().flatten());

    // 2. spaces only the person could read: their documents (files included), every row, the spaces
    let objects = !state.settings.documents_backend.is_empty();
    for vault in &solo {
        if objects {
            let in_vault: Vec<Value> = crate::store::documents::IN_VAULT.on(db).bind(("vault", vault.clone())).await?.take(0)?;
            files.extend(in_vault.iter().filter_map(|d| d.get("object_path").and_then(Value::as_str).map(str::to_string)));
        }
        let n = crate::documents::purge_vault_documents(db, vault).await?;
        *counts.entry("documents".into()).or_insert(0) += n as i64;
    }
    if !solo.is_empty() {
        let mut res = crate::tx::with_retry(|| async { q::ERASE_SPACES.on(db).bind(("solo", solo.clone())).await }).await?;
        add_counts(&mut counts, &res.take::<Option<Value>>(q::ERASE_SPACES.slot)?.unwrap_or_default());
    }

    // what stays in the workspace no longer carries their email address
    let mut res = q::SCRUB_EMAIL.on(db).bind(("email", email.to_string())).bind(("email_lc", email_lc.to_string())).await?;
    let n: Option<i64> = res.take(q::SCRUB_EMAIL.slot)?;
    counts.insert("email_scrubbed_in_shared_spaces".into(), n.unwrap_or(0));

    // 3. the person's own rows everywhere else, the bucket files, the actor names, the embed cache
    if objects {
        let unique: HashSet<String> = files.iter().cloned().collect();
        for path in &unique {
            crate::documents::delete_object(db, path).await?;
        }
        counts.insert("bucket_files".into(), unique.len() as i64);
    }
    let mut res = crate::tx::with_retry(|| async {
        q::ERASE_OWNED
            .on(db)
            .bind(("u", subject.clone()))
            .bind(("heir", heir.clone()))
            .bind(("actors", actors.to_vec()))
            .bind(("email", email.to_string()))
            .await
    })
    .await?;
    add_counts(&mut counts, &res.take::<Option<Value>>(q::ERASE_OWNED.slot)?.unwrap_or_default());
    for vault in &solo {
        crate::tx::with_retry(|| async {
            crate::store::vaults::DELETE_VAULT.on(db).bind(("vault", vault.clone())).bind(("counter", crate::changes::counter(vault))).await?.check()
        })
        .await?;
    }
    counts.insert("spaces".into(), solo.len() as i64);
    let gone: Vec<Value> = q::ERASE_MEMBERSHIPS.on(db).bind(("u", subject.clone())).await?.take(0)?;
    counts.insert("space_memberships".into(), gone.len() as i64);
    let cache: Vec<Value> = q::ERASE_EMBED_CACHE.on(db).await?.take(0)?;
    counts.insert("embedding_cache_entries".into(), cache.len() as i64);
    Ok((json!(counts), files, solo))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn residue_lists_only_nonzero_tables() {
        let check = json!({ "memory": 0, "session": 2, "bucket_files": 1, "user": 0 });
        let mut left = residue(&check);
        left.sort();
        assert_eq!(left, vec!["bucket_files", "session"]);
        assert!(residue(&json!({})).is_empty());
    }

    #[test]
    fn counts_add_up_across_orgs() {
        let mut c = BTreeMap::new();
        add_counts(&mut c, &json!({ "memories": 2, "spaces": 1 }));
        add_counts(&mut c, &json!({ "memories": 3 }));
        assert_eq!(c.get("memories"), Some(&5));
        assert_eq!(c.get("spaces"), Some(&1));
    }
}

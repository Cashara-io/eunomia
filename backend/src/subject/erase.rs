//! The `subject_erase` job: removes or anonymises every category of the inventory in
//! docs/privacy/subject-rights.md for one person, then proves it with a post-check.
//!
//! The job works from a **working record** saved on the `erasure` row before anything is deleted:
//! the email (lowercase), the person's token ids and key, their file paths, and the plan per
//! workspace (who inherits, which spaces stay and which are theirs alone, whether the workspace
//! closes with them). A retry, or a run after a crash, re-checks with those captured identifiers, so a
//! post-check can never pass because the account it would have looked them up from is already gone.
//!
//! Order (each step is idempotent, so a retry re-derives what is left):
//!
//! 1. Legal hold: checked at the start and again before every irreversible phase. A held erasure
//!    parks as `held` and resumes when the hold is lifted (`subject::set_legal_hold`).
//! 2. The workspace's running slot (unique): one erasure at a time per workspace.
//! 3. The working record, captured once.
//! 4. Per workspace: shared spaces keep what the person wrote, reassigned to an owner who remains;
//!    spaces only the person could read are deleted with their files; everything else of theirs goes;
//!    their email is replaced (any case) where the system holds it. A workspace whose only member
//!    they are is emptied entirely.
//! 5. Org post-checks. Anything left fails the job (it retries) and **the account is kept**.
//! 6. The control database: credentials, sessions, apps, identities, jobs, counters, memberships, the
//!    account; their audit events are kept but no longer name them. Then the control post-check.
//! 7. Closed workspaces are marked closed. The row becomes the completion record.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};
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
    #[serde(default)]
    #[surreal(default)]
    requested_by: String,
    #[serde(default)]
    #[surreal(default)]
    work: Value,
    #[serde(default)]
    #[surreal(default)]
    closes: Vec<String>,
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

/// A shared space that stays: who inherits the person's rows there, and whether they must be made an owner.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Kept {
    pub vault: String,
    pub to: String,
    pub promote: bool,
}

/// The plan for one workspace, captured before anything is deleted.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OrgWork {
    pub org: String,
    /// Inherits org-level rows (the workspace's password-manager setup); none when it closes.
    pub heir: Option<String>,
    /// The person is its only member: everything in it goes, and it is closed.
    pub close: bool,
    /// The person owns it with other members and no other owner (only on a restore re-apply): the
    /// heir is made the owner first, so the workspace keeps one.
    pub transfer: bool,
    pub solo: Vec<String>,
    pub kept: Vec<Kept>,
    pub files: Vec<String>,
}

/// The working record: everything the post-check needs, so a retry checks as much as the first run.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Work {
    pub captured: bool,
    /// The email, trimmed and lowercased; none when the account was already gone (a restore of only
    /// an org database), and then only ids are matched.
    pub email_lc: Option<String>,
    pub actors: Vec<String>,
    pub key: String,
    pub orgs: Vec<OrgWork>,
}

/// Adds every number in `from` to `into`, by key.
fn add_counts(into: &mut BTreeMap<String, i64>, from: &Value) {
    if let Some(obj) = from.as_object() {
        for (k, v) in obj {
            *into.entry(k.clone()).or_insert(0) += v.as_i64().unwrap_or(0);
        }
    }
}

fn add_into(into: &mut Map<String, Value>, from: &Value) {
    if let Some(obj) = from.as_object() {
        for (k, v) in obj {
            let n = into.get(k).and_then(Value::as_i64).unwrap_or(0) + v.as_i64().unwrap_or(0);
            into.insert(k.clone(), json!(n));
        }
    }
}

/// The keys of a post-check object whose count is not 0.
pub fn residue(check: &Value) -> Vec<String> {
    check.as_object().map(|o| o.iter().filter(|(_, v)| v.as_i64().unwrap_or(0) != 0).map(|(k, _)| k.clone()).collect()).unwrap_or_default()
}

/// A value no text contains (stands in for an email that is not known).
fn never_matches() -> String {
    format!("\u{0}unknown-{}", uuid::Uuid::new_v4().simple())
}

/// The case-insensitive pattern for an email (bound as a parameter, never spliced into SQL).
pub fn email_pattern(email_lc: &str) -> String {
    format!("(?i){}", regex::escape(email_lc))
}

fn rid(s: &str) -> Result<RecordId, AppError> {
    crate::rid::parse(s).map_err(|_| AppError::internal("bad record id in the erasure plan"))
}

/// What a job run ended as.
enum Outcome {
    Done,
    Held,
    Busy,
}

pub async fn erase_job(state: AppState, job: Job) -> Result<(), JobError> {
    let id_str = job.payload.get("erasure").and_then(Value::as_str).ok_or_else(|| JobError::permanent(ErrorCode::ValidationInvalid, "job payload has no erasure"))?;
    let erasure = crate::rid::parse(id_str).map_err(|_| JobError::permanent(ErrorCode::ValidationInvalid, "bad erasure id"))?;
    match run(&state, &erasure, &job.payload).await {
        Ok(Outcome::Done | Outcome::Held) => Ok(()),
        Ok(Outcome::Busy) => Err(JobError::retryable(ErrorCode::DbConflict, "another erasure in this workspace is running")),
        Err(e) => {
            // the raw database text stays in the log (it can quote a row), the record gets the code
            tracing::warn!(erasure = %erasure.to_string(), code = e.code.as_str(), source = e.source.as_deref().unwrap_or(""), "erasure step failed; the job retries");
            let msg = if e.code == ErrorCode::SubjectLastOwner { e.message.clone() } else { format!("A step failed ({}); the job retries.", e.code.as_str()) };
            let _ = q::ERASURE_FAILED.on(&state.control).bind(("id", erasure)).bind(("error", msg)).bind(("post_check", json!({}))).await;
            Err(JobError::from(e))
        }
    }
}

async fn held(state: &AppState, subject: &RecordId) -> Result<bool, AppError> {
    let at: Option<surrealdb::types::Datetime> = q::HOLD_OF.on(&state.control).bind(("u", subject.clone())).await?.take::<Option<surrealdb::types::Datetime>>(0).unwrap_or(None);
    Ok(at.is_some())
}

async fn park(state: &AppState, erasure: &RecordId) -> Result<Outcome, AppError> {
    q::ERASURE_HELD.on(&state.control).bind(("id", erasure.clone())).await?.check()?;
    tracing::info!(erasure = %erasure.to_string(), "erasure parked by a legal hold; it resumes when the hold is lifted");
    Ok(Outcome::Held)
}

async fn run(state: &AppState, erasure: &RecordId, payload: &Value) -> Result<Outcome, AppError> {
    let row: Option<ErasureRow> = q::ERASURE_GET.on(&state.control).bind(("id", erasure.clone())).await?.take(0)?;
    let Some(row) = row else { return Err(AppError::coded(ErrorCode::ResourceNotFound, "erasure row is gone")) };
    if row.status == "done" || row.status == "held" {
        return Ok(Outcome::Done);
    }
    let Some(subject) = row.subject.clone() else { return Err(AppError::internal("erasure has no subject")) };
    let reapply = row.requested_by == "reapply";

    if !reapply && held(state, &subject).await? {
        return park(state, erasure).await;
    }
    // one running erasure per workspace: the slot is unique
    let taken = q::ERASURE_RUNNING.on(&state.control).bind(("id", erasure.clone())).await.and_then(|mut r| r.take::<Vec<String>>(0));
    match taken {
        Ok(ids) if ids.is_empty() => return Ok(Outcome::Done),
        Ok(_) => {}
        Err(e) if crate::tx::is_duplicate(&e) => return Ok(Outcome::Busy),
        Err(e) => {
            let e = AppError::from(e);
            if e.code == ErrorCode::DbConflict || e.code == ErrorCode::DbDuplicate {
                return Ok(Outcome::Busy);
            }
            return Err(e);
        }
    }

    let work: Work = match serde_json::from_value::<Work>(row.work.clone()) {
        Ok(w) if w.captured => w,
        _ => {
            let w = capture(state, &subject, &row, payload).await?;
            let closes: Vec<String> = w.orgs.iter().filter(|o| o.close).map(|o| o.org.clone()).collect();
            q::ERASURE_WORK
                .on(&state.control)
                .bind(("id", erasure.clone()))
                .bind(("work", serde_json::to_value(&w).unwrap_or_default()))
                .bind(("closes", closes))
                .await?
                .check()?;
            w
        }
    };

    // the last moment a hold placed after the request stops it: nothing is deleted yet
    if !reapply && held(state, &subject).await? {
        return park(state, erasure).await;
    }

    let email_lc = work.email_lc.clone().unwrap_or_else(never_matches);
    let email_re = email_pattern(&email_lc);
    let mut counts: BTreeMap<String, i64> = BTreeMap::new();
    let mut check = Map::new();
    let mut review = Map::new();
    for o in &work.orgs {
        let Some(org) = OrgId::parse(&o.org) else { continue };
        let org_state = match state.org(&org).await {
            Ok(s) => s,
            // a retry after this workspace was already closed: it was emptied and checked before that
            Err(e) if o.close && e.message.contains("closed") => continue,
            Err(e) => return Err(e),
        };
        let c = erase_in_org(&org_state, &subject, o, &work.actors, &email_lc, &email_re).await?;
        add_counts(&mut counts, &c);
        add_into(&mut check, &org_check(&org_state, &subject, o, &work, &email_lc).await?);
        if !o.close {
            let r: Value = q::REVIEW.on(&org_state.db).bind(("email_lc", email_lc.clone())).await?.take::<Option<Value>>(0)?.unwrap_or_default();
            add_into(&mut review, &r);
        }
    }
    if !residue(&Value::Object(check.clone())).is_empty() {
        return fail_with_residue(state, erasure, check).await;
    }

    if !reapply && held(state, &subject).await? {
        return park(state, erasure).await;
    }
    // the workspace keeps an owner (a restore re-apply of its sole owner): hand it over first
    for o in work.orgs.iter().filter(|o| o.transfer) {
        if let (Some(org), Some(heir)) = (OrgId::parse(&o.org), o.heir.as_deref()) {
            q::ORG_OWNER_TRANSFER.on(&state.control).bind(("org", org.record())).bind(("from", subject.clone())).bind(("to", rid(heir)?)).await?.check()?;
            *counts.entry("workspace_ownership_handed_over".into()).or_insert(0) += 1;
        }
    }
    let mut res = q::ERASE_CONTROL
        .on(&state.control)
        .bind(("key", work.key.clone()))
        .bind(("u", subject.clone()))
        .bind(("actors", work.actors.clone()))
        .bind(("email_re", email_re.clone()))
        .bind(("email_lc", email_lc.clone()))
        .await?;
    add_counts(&mut counts, &res.take::<Option<Value>>(q::ERASE_CONTROL.slot)?.unwrap_or_default());
    state.credentials.forget_user(&subject);
    let mut res = q::CONTROL_RESIDUE
        .on(&state.control)
        .bind(("key", work.key.clone()))
        .bind(("u", subject.clone()))
        .bind(("actors", work.actors.clone()))
        .bind(("email_lc", email_lc.clone()))
        .bind(("erasure", erasure.clone()))
        .await?;
    add_into(&mut check, &res.take::<Option<Value>>(0)?.unwrap_or_default());
    if !residue(&Value::Object(check.clone())).is_empty() {
        return fail_with_residue(state, erasure, check).await;
    }

    for o in work.orgs.iter().filter(|o| o.close) {
        q::CLOSE_ORG.on(&state.control).bind(("okey", o.org.clone())).await?.check()?;
        if let Some(org) = OrgId::parse(&o.org) {
            state.pool.evict(&org);
        }
        *counts.entry("workspaces_closed".into()).or_insert(0) += 1;
    }
    let counts = json!(counts);
    q::ERASURE_DONE
        .on(&state.control)
        .bind(("id", erasure.clone()))
        .bind(("counts", counts.clone()))
        .bind(("post_check", Value::Object(check)))
        .bind(("review", json!({ "mentions_left_in_others_content": Value::Object(review) })))
        .await?
        .check()?;
    // filed under someone who stays in the home workspace, so that workspace's log (and only it) shows it
    let scope = work.orgs.iter().find_map(|o| o.heir.as_deref()).and_then(|h| crate::rid::parse(h).ok()).unwrap_or_else(|| RecordId::new("user", "erased"));
    let actor = crate::authz::Caller::system().actor;
    let detail = serde_json::to_string(&counts).unwrap_or_default();
    crate::audit::record_structured(
        &state.control,
        crate::audit::Event { user: Some(&scope), actor: &actor, action: "subject.erase", target: &erasure.to_string(), outcome: "ok", detail: &detail },
    )
    .await;
    Ok(Outcome::Done)
}

async fn fail_with_residue(state: &AppState, erasure: &RecordId, check: Map<String, Value>) -> Result<Outcome, AppError> {
    let left = residue(&Value::Object(check.clone())).join(", ");
    let msg = format!("The post-check still found the person in: {left}. The account is kept and the job retries.");
    q::ERASURE_FAILED.on(&state.control).bind(("id", erasure.clone())).bind(("error", msg.clone())).bind(("post_check", Value::Object(check))).await?.check()?;
    Err(AppError::coded(ErrorCode::Internal, msg))
}

/// The people with an erasure open, but not `subject` (they inherit nothing).
pub async fn pending_others(state: &AppState, subject: &RecordId) -> Result<Vec<RecordId>, AppError> {
    let all: Vec<RecordId> = q::OPEN_SUBJECTS.on(&state.control).await?.take(0)?;
    Ok(all.into_iter().filter(|u| u != subject).collect())
}

/// Builds the working record. Run once, before anything is deleted.
async fn capture(state: &AppState, subject: &RecordId, row: &ErasureRow, payload: &Value) -> Result<Work, AppError> {
    #[derive(Deserialize, SurrealValue)]
    struct EmailRow {
        #[serde(default)]
        #[surreal(default)]
        email: String,
    }
    let email_row: Option<EmailRow> = q::CONFIRM_ROW.on(&state.control).bind(("u", subject.clone())).await?.take(0)?;
    let email_lc = email_row.map(|r| crate::models_user::normalize_email(&r.email)).filter(|e| !e.is_empty());
    let mut actors: Vec<String> = q::TOKEN_IDS.on(&state.control).bind(("u", subject.clone())).await?.take(0)?;
    actors.push(subject.to_string());
    let key = crate::rid::key_string(subject.key()).unwrap_or_else(never_matches);

    let pending = pending_others(state, subject).await?;
    let allow_close = !row.closes.is_empty() || row.requested_by == "reapply";
    let plans = match super::plan(&state.control, subject, &pending, allow_close, row.requested_by == "reapply").await {
        Ok(p) if !p.is_empty() => p,
        // no memberships left (a restore of only an org database): the plan the request recorded
        Ok(_) => plans_from_payload(payload),
        Err(e) => return Err(e),
    };
    let mut orgs = Vec::new();
    for p in plans {
        let org_state = state.org(&p.org).await?;
        let db = &org_state.db;
        let others: Vec<MemberRow> = q::OTHER_MEMBERS.on(db).bind(("u", subject.clone())).bind(("pending", pending.clone())).await?.take(0)?;
        let mut heir_of: HashMap<String, (RecordId, bool)> = HashMap::new();
        for m in others {
            heir_of.entry(m.vault).or_insert((m.user, m.role == "owner"));
        }
        let mine: Vec<MyMembership> = q::MEMBERSHIPS.on(db).bind(("u", subject.clone())).await?.take(0)?;
        let mut w = OrgWork { org: p.org.key(), heir: p.heir.as_ref().map(|h| h.to_string()), close: p.close, transfer: p.transfer, ..OrgWork::default() };
        if p.close {
            let mut res = q::ALL_SPACES_AND_FILES.on(db).await?;
            w.solo = res.take::<Vec<RecordId>>(0)?.iter().map(|v| v.to_string()).collect();
            w.files = res.take::<Vec<Option<String>>>(1)?.into_iter().flatten().collect();
            w.files.extend(res.take::<Vec<Option<String>>>(2)?.into_iter().flatten());
        } else {
            w.solo = mine.iter().filter(|m| m.status == "active" && !heir_of.contains_key(&m.vault)).map(|m| m.vault.clone()).collect();
            let my_owned: HashSet<&str> = mine.iter().filter(|m| m.status == "active" && m.role == "owner").map(|m| m.vault.as_str()).collect();
            w.kept = heir_of
                .iter()
                .map(|(v, (to, is_owner))| Kept { vault: v.clone(), to: to.to_string(), promote: my_owned.contains(v.as_str()) && !is_owner })
                .collect();
            let kept: Vec<RecordId> = w.kept.iter().filter_map(|k| crate::rid::parse(&k.vault).ok()).collect();
            let mut res = q::OWNED_FILES.on(db).bind(("u", subject.clone())).bind(("kept", kept)).await?;
            w.files = res.take::<Vec<Option<String>>>(0)?.into_iter().flatten().collect();
            w.files.extend(res.take::<Vec<Option<String>>>(1)?.into_iter().flatten());
            for v in &w.solo {
                let docs: Vec<Value> = crate::store::documents::IN_VAULT.on(db).bind(("vault", rid(v)?)).await?.take(0)?;
                w.files.extend(docs.iter().filter_map(|d| d.get("object_path").and_then(Value::as_str).map(str::to_string)));
            }
        }
        w.files.sort();
        w.files.dedup();
        orgs.push(w);
    }
    Ok(Work { captured: true, email_lc, actors, key, orgs })
}

fn plans_from_payload(payload: &Value) -> Vec<super::OrgPlan> {
    payload
        .get("orgs")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|o| {
                    Some(super::OrgPlan {
                        org: OrgId::parse(o.get("org")?.as_str()?)?,
                        heir: o.get("heir").and_then(Value::as_str).and_then(|h| crate::rid::parse(h).ok()),
                        close: o.get("close").and_then(Value::as_bool).unwrap_or(false),
                        transfer: false,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The org post-check for one workspace: the residue statement, the files, and for a closed
/// workspace every table empty.
async fn org_check(state: &OrgState, subject: &RecordId, o: &OrgWork, work: &Work, email_lc: &str) -> Result<Value, AppError> {
    let solo: Vec<RecordId> = o.solo.iter().filter_map(|v| crate::rid::parse(v).ok()).collect();
    let mut res = q::RESIDUE
        .on(&state.db)
        .bind(("u", subject.clone()))
        .bind(("actors", work.actors.clone()))
        .bind(("email_lc", email_lc.to_string()))
        .bind(("solo", solo))
        .await?;
    let mut check: Map<String, Value> = res.take::<Option<Value>>(0)?.and_then(|v| v.as_object().cloned()).unwrap_or_default();
    let mut left = 0i64;
    if !state.settings.documents_backend.is_empty() {
        for path in &o.files {
            let exists: Option<bool> = q::OBJECT_EXISTS.on(&state.db).bind(("path", path.clone())).await?.take(0)?;
            left += i64::from(exists.unwrap_or(false));
        }
    }
    check.insert("bucket_files".into(), json!(left));
    if o.close {
        check.insert("closed_workspace_rows".into(), json!(count_all_rows(&state.db).await?));
    }
    Ok(Value::Object(check))
}

/// Steps 4 in one workspace, from the captured plan. Returns the counts.
async fn erase_in_org(state: &OrgState, subject: &RecordId, o: &OrgWork, actors: &[String], email_lc: &str, email_re: &str) -> Result<Value, AppError> {
    let db = &state.db;
    let mut counts: BTreeMap<String, i64> = BTreeMap::new();
    let solo: Vec<RecordId> = o.solo.iter().filter_map(|v| crate::rid::parse(v).ok()).collect();
    let heir = o.heir.as_deref().map(rid).transpose()?.unwrap_or_else(|| RecordId::new("user", "erased"));

    // shared spaces: hand over ownership where the person was the only owner, then reassign their rows
    for k in o.kept.iter().filter(|k| k.promote) {
        q::PROMOTE.on(db).bind(("vault", rid(&k.vault)?)).bind(("to", rid(&k.to)?)).await?.check()?;
        *counts.entry("ownership_handed_over".into()).or_insert(0) += 1;
    }
    let with_rows: HashSet<String> = q::VAULTS_WITH_ROWS.on(db).bind(("u", subject.clone())).await?.take::<Vec<String>>(0)?.into_iter().collect();
    for k in o.kept.iter().filter(|k| with_rows.contains(&k.vault)) {
        let (vault, to) = (rid(&k.vault)?, rid(&k.to)?);
        let mut res = crate::tx::with_retry(|| async { q::REASSIGN.on(db).bind(("u", subject.clone())).bind(("vault", vault.clone())).bind(("to", to.clone())).await }).await?;
        #[derive(Deserialize, SurrealValue)]
        struct Reassigned {
            #[serde(default)]
            #[surreal(default)]
            n: i64,
            #[serde(default)]
            #[surreal(default)]
            documents: Vec<String>,
        }
        let r = res.take::<Option<Reassigned>>(q::REASSIGN.slot)?.unwrap_or(Reassigned { n: 0, documents: Vec::new() });
        *counts.entry("reassigned_in_shared_spaces".into()).or_insert(0) += r.n;
        // their chunks went with their records: index the document again as the new owner's
        if !state.settings.documents_backend.is_empty() {
            for doc in &r.documents {
                if let Err(e) = crate::documents::reindex(state, &to, doc).await {
                    tracing::warn!(error = %e.message, "re-indexing a reassigned document failed; reindex it from Documents");
                }
            }
        }
    }

    // spaces only the person could read (every space, for a closing workspace): documents and their files, every row
    let objects = !state.settings.documents_backend.is_empty();
    for vault in &solo {
        let n = crate::documents::purge_vault_documents(db, vault).await?;
        *counts.entry("documents".into()).or_insert(0) += n as i64;
    }
    if !solo.is_empty() {
        let mut res = crate::tx::with_retry(|| async { q::ERASE_SPACES.on(db).bind(("solo", solo.clone())).await }).await?;
        add_counts(&mut counts, &res.take::<Option<Value>>(q::ERASE_SPACES.slot)?.unwrap_or_default());
    }

    // what stays in the workspace no longer carries their email address, in any case
    let mut res = q::SCRUB_EMAIL.on(db).bind(("email_re", email_re.to_string())).bind(("email_lc", email_lc.to_string())).await?;
    counts.insert("email_scrubbed".into(), res.take::<Option<i64>>(q::SCRUB_EMAIL.slot)?.unwrap_or(0));

    // the person's own rows everywhere else, the bucket files, the actor names, the embed cache
    if objects {
        for path in &o.files {
            crate::documents::delete_object(db, path).await?;
        }
        counts.insert("bucket_files".into(), o.files.len() as i64);
    }
    let mut res = crate::tx::with_retry(|| async {
        q::ERASE_OWNED
            .on(db)
            .bind(("u", subject.clone()))
            .bind(("heir", heir.clone()))
            .bind(("actors", actors.to_vec()))
            .bind(("email_lc", email_lc.to_string()))
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
    if o.close {
        // the workspace closes with its only member: whatever is left in any table goes too
        empty_all_tables(db).await?;
    }
    Ok(json!(counts))
}

/// Rows left in every org table (a closed workspace must have none).
pub async fn count_all_rows(db: &crate::pool::OrgDb) -> Result<i64, AppError> {
    #[derive(Deserialize, SurrealValue)]
    struct N {
        #[serde(default)]
        #[surreal(default)]
        count: i64,
    }
    let mut rows = 0i64;
    for t in crate::store::TENANT_TABLES {
        let n: Vec<N> = crate::store::dynamic(db, "subject.closed_org_count", format!("SELECT count() FROM {t} GROUP ALL")).await?.take(0)?;
        rows += n.first().map_or(0, |n| n.count);
    }
    Ok(rows)
}

/// Empties every org table (a workspace that closes with its only member).
pub async fn empty_all_tables(db: &crate::pool::OrgDb) -> Result<(), AppError> {
    for t in crate::store::TENANT_TABLES {
        crate::store::dynamic(db, "subject.closed_org_delete", format!("DELETE {t} RETURN NONE")).await?.check()?;
    }
    Ok(())
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

    #[test]
    fn the_email_pattern_matches_any_case_and_only_the_address() {
        let re = regex::Regex::new(&email_pattern("alice@example.com")).unwrap();
        assert!(re.is_match("Reach Alice@Example.COM"));
        assert!(!re.is_match("alice@exampleXcom"), "the dot is literal");
        let saved = serde_json::to_value(Work { captured: true, email_lc: Some("a@b.c".into()), ..Work::default() }).unwrap();
        let back: Work = serde_json::from_value(saved).unwrap();
        assert!(back.captured && back.email_lc.as_deref() == Some("a@b.c"), "the working record round-trips");
    }
}

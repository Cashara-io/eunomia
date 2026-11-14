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
//! 2. A lock row per workspace touched: one running erasure at a time per workspace.
//! 3. The working record, recaptured from live state on every run and merged with the saved one; then
//!    `deleting_at` is set. Until then an operator may cancel; from then on it completes or waits.
//! 4. Per workspace: shared spaces keep what the person wrote, reassigned to a reader who remains and
//!    may inherit (not someone under a hold or being erased; with nobody, the person's own rows there
//!    go and everyone else's stay);
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
    /// The workspace the request was made in (for a re-apply of restored rows: the workspace they came back in).
    #[serde(default)]
    #[surreal(default)]
    org: String,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct OrgMember {
    user: RecordId,
    #[serde(default)]
    #[surreal(default)]
    role: String,
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
    /// Spaces that stay and that the person was in: their names are the person's to scrub; any other
    /// space's name is someone else's, only counted for review.
    #[serde(default)]
    pub member_of: Vec<String>,
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
/// The address an account signs in with, normalised. For an account set aside by "Start fresh"
/// (`email = "set-aside:<id> (was <address>)"`, `routers/oidc.rs`) it is the kept old address.
pub fn sign_in_address(raw: &str) -> String {
    let addr = raw
        .strip_prefix("set-aside:")
        .and_then(|rest| rest.find(" (was ").map(|i| &rest[i + " (was ".len()..]))
        .map(|a| a.strip_suffix(')').unwrap_or(a))
        .unwrap_or(raw);
    crate::models_user::normalize_email(addr)
}

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
    /// Another erasure in the workspace is running: try again shortly, without spending an attempt.
    Busy,
    /// The post-check found the person somewhere: recorded on the row; the job retries.
    Residue(String),
}

pub async fn erase_job(state: AppState, job: Job) -> Result<(), JobError> {
    let id_str = job.payload.get("erasure").and_then(Value::as_str).ok_or_else(|| JobError::permanent(ErrorCode::ValidationInvalid, "job payload has no erasure"))?;
    let erasure = crate::rid::parse(id_str).map_err(|_| JobError::permanent(ErrorCode::ValidationInvalid, "bad erasure id"))?;
    let out = run(&state, &erasure).await;
    // every way out of a run gives the workspace locks back (a crash leaves them to the start-up heal)
    let _ = q::LOCKS_RELEASE.on(&state.control).bind(("id", erasure.clone())).await;
    match out {
        Ok(Outcome::Done | Outcome::Held) => Ok(()),
        Ok(Outcome::Busy) => {
            // waiting for another erasure is not a failure: a fresh job a few seconds on, this one ends
            let key = format!("{}:{}:{}", crate::jobs::kind::SUBJECT_ERASE, erasure.to_string(), uuid::Uuid::new_v4().simple());
            let mut next = crate::jobs::NewJob::new(crate::jobs::kind::SUBJECT_ERASE, job.owner.clone(), key).payload(json!({ "erasure": erasure.to_string() }));
            next.run_at = Some(chrono::Utc::now() + chrono::Duration::seconds(3));
            next.max_attempts = 8;
            crate::jobs::enqueue(&state.control, next).await.map_err(JobError::from)?;
            Ok(())
        }
        Ok(Outcome::Residue(msg)) => Err(JobError::retryable(ErrorCode::Internal, msg)),
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

async fn run(state: &AppState, erasure: &RecordId) -> Result<Outcome, AppError> {
    let row: Option<ErasureRow> = q::ERASURE_GET.on(&state.control).bind(("id", erasure.clone())).await?.take(0)?;
    let Some(row) = row else { return Err(AppError::coded(ErrorCode::ResourceNotFound, "erasure row is gone")) };
    if matches!(row.status.as_str(), "done" | "held" | "cancelled") {
        return Ok(Outcome::Done);
    }
    let Some(subject) = row.subject.clone() else { return Err(AppError::internal("erasure has no subject")) };
    let reapply = row.requested_by == "reapply";

    if !reapply && held(state, &subject).await? {
        return park(state, erasure).await;
    }
    // the plan is rebuilt from live state on every run (a resume after a hold sees new files, tokens and
    // workspaces) and merged with what was saved before (a retry after steps that already ran still checks
    // everything the first run knew)
    let saved = serde_json::from_value::<Work>(row.work.clone()).ok().filter(|w| w.captured);
    let fresh = capture(state, &subject, &row).await?;
    let work = match saved {
        Some(s) => merge(s, fresh),
        None => fresh,
    };
    let closes: Vec<String> = work.orgs.iter().filter(|o| o.close).map(|o| o.org.clone()).collect();
    let saved: Vec<String> =
        q::ERASURE_WORK.on(&state.control).bind(("id", erasure.clone())).bind(("work", serde_json::to_value(&work).unwrap_or_default())).bind(("closes", closes)).await?.take(0)?;
    if saved.is_empty() {
        // cancelled (or finished) meanwhile: nothing is written back to the row and nothing is deleted
        return Ok(Outcome::Done);
    }

    // one running erasure per workspace, over every workspace this one touches
    let orgs: Vec<String> = work.orgs.iter().map(|o| o.org.clone()).collect();
    let locked = q::LOCKS_TAKE.on(&state.control).bind(("id", erasure.clone())).bind(("orgs", orgs)).await.and_then(|r| r.check());
    if let Err(e) = locked {
        tracing::info!(erasure = %erasure.to_string(), error = %e, "another erasure holds a workspace this one touches; waiting");
        return Ok(Outcome::Busy);
    }
    let started: Vec<String> = q::ERASURE_RUNNING.on(&state.control).bind(("id", erasure.clone())).await?.take(0)?;
    if started.is_empty() {
        return Ok(Outcome::Done);
    }

    // the last moment a hold placed after the request stops it: nothing is deleted yet
    if !reapply && held(state, &subject).await? {
        return park(state, erasure).await;
    }
    // from here the erasure deletes: it can no longer be cancelled, only complete or wait
    let marked: Vec<String> = q::ERASURE_DELETING.on(&state.control).bind(("id", erasure.clone())).await?.take(0)?;
    if marked.is_empty() {
        return Ok(Outcome::Done);
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
            let kept: Vec<RecordId> = o.member_of.iter().filter_map(|v| crate::rid::parse(v).ok()).collect();
            let r: Value = q::REVIEW.on(&org_state.db).bind(("email_lc", email_lc.clone())).bind(("kept", kept)).await?.take::<Option<Value>>(0)?.unwrap_or_default();
            add_into(&mut review, &r);
        }
    }
    if !residue(&Value::Object(check.clone())).is_empty() {
        return fail_with_residue(state, erasure, check).await;
    }

    if !reapply && held(state, &subject).await? {
        return park(state, erasure).await;
    }
    // the workspace keeps an owner: hand it over first where the person owned it
    for o in work.orgs.iter().filter(|o| o.transfer) {
        if let (Some(org), Some(heir)) = (OrgId::parse(&o.org), o.heir.as_deref()) {
            let res = q::ORG_OWNER_TRANSFER.on(&state.control).bind(("org", org.record())).bind(("from", subject.clone())).bind(("to", rid(heir)?)).await.and_then(|r| r.check());
            // already handed over by an earlier run: the person is no longer a member there
            if res.is_ok() {
                *counts.entry("workspace_ownership_handed_over".into()).or_insert(0) += 1;
            }
        }
    }

    {
        let mut res = q::ERASE_CONTROL
            .on(&state.control)
            .bind(("key", work.key.clone()))
            .bind(("u", subject.clone()))
            .bind(("actors", work.actors.clone()))
            .bind(("email_re", email_re.clone()))
            .bind(("email_lc", email_lc.clone()))
            .bind(("sealed_detail", crate::audit::chain::ERASED_DETAIL))
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
            .bind(("sealed_detail", crate::audit::chain::ERASED_DETAIL))
            .await?;
        add_into(&mut check, &res.take::<Option<Value>>(0)?.unwrap_or_default());
    }
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
    Ok(Outcome::Residue(msg))
}

/// The people with an erasure open, but not `subject` (they inherit nothing).
pub async fn pending_others(state: &AppState, subject: &RecordId) -> Result<Vec<RecordId>, AppError> {
    let all: Vec<RecordId> = q::OPEN_SUBJECTS.on(&state.control).await?.take(0)?;
    Ok(all.into_iter().filter(|u| u != subject).collect())
}

/// A later capture added to an earlier one: every identifier and file either knew, the live plan where
/// there is one and the saved plan for workspaces the live state no longer shows (steps already ran).
fn merge(saved: Work, fresh: Work) -> Work {
    let mut actors = saved.actors.clone();
    for a in fresh.actors {
        if !actors.contains(&a) {
            actors.push(a);
        }
    }
    let mut orgs: Vec<OrgWork> = Vec::new();
    for f in fresh.orgs {
        let mut o = f;
        if let Some(s) = saved.orgs.iter().find(|s| s.org == o.org) {
            for v in &s.solo {
                if !o.solo.contains(v) {
                    o.solo.push(v.clone());
                }
            }
            for p in &s.files {
                if !o.files.contains(p) {
                    o.files.push(p.clone());
                }
            }
            for k in &s.kept {
                if !o.kept.iter().any(|x| x.vault == k.vault) && !o.solo.contains(&k.vault) {
                    o.kept.push(k.clone());
                }
            }
            for v in &s.member_of {
                if !o.member_of.contains(v) {
                    o.member_of.push(v.clone());
                }
            }
            o.close |= s.close;
            o.transfer |= s.transfer;
            if o.heir.is_none() {
                o.heir = s.heir.clone();
            }
        }
        orgs.push(o);
    }
    for s in saved.orgs {
        if !orgs.iter().any(|o| o.org == s.org) {
            orgs.push(s);
        }
    }
    Work { captured: true, email_lc: saved.email_lc.or(fresh.email_lc), actors, key: saved.key, orgs }
}

/// Builds a working record from live state.
async fn capture(state: &AppState, subject: &RecordId, row: &ErasureRow) -> Result<Work, AppError> {
    #[derive(Deserialize, SurrealValue)]
    struct EmailRow {
        #[serde(default)]
        #[surreal(default)]
        email: String,
    }
    let email_row: Option<EmailRow> = q::CONFIRM_ROW.on(&state.control).bind(("u", subject.clone())).await?.take(0)?;
    let mut email_lc = email_row.as_ref().map(|r| sign_in_address(&r.email)).filter(|e| !e.is_empty());
    // an account set aside by "Start fresh" keeps its old address only in its raw email; when another
    // account now signs in with that address, the address is that account's, so it is not scrubbed
    // from what stays (the set-aside account's ids are still erased everywhere)
    if let (true, Some(addr)) = (email_row.as_ref().is_some_and(|r| r.email.starts_with("set-aside:")), email_lc.clone()) {
        let holder: Vec<RecordId> = q::EMAIL_HOLDER.on(&state.control).bind(("email", addr)).bind(("u", subject.clone())).await?.take(0)?;
        if !holder.is_empty() {
            email_lc = None;
        }
    }
    let mut actors: Vec<String> = q::TOKEN_IDS.on(&state.control).bind(("u", subject.clone())).await?.take(0)?;
    actors.push(subject.to_string());
    let key = crate::rid::key_string(subject.key()).unwrap_or_else(never_matches);

    let (pending, held) = super::pending_and_held(&state.control, subject).await?;
    // nobody with an erasure open and nobody under a legal hold inherits anything (they stay readers)
    let excluded: HashSet<String> = pending.iter().chain(held.iter()).map(|u| u.to_string()).collect();
    let reapply = row.requested_by == "reapply";
    let allow_close = !row.closes.is_empty() || reapply;
    let mut plans = match super::plan(&state.control, subject, &pending, &held, allow_close, reapply).await {
        Ok(p) => p,
        // the account is already gone (a retry after the control step): the saved plan stands
        Err(e) if e.code == ErrorCode::AuthNotFound => Vec::new(),
        Err(e) => return Err(e),
    };
    // no membership left: rows a restore of only an org database brought back are erased in the
    // workspace they came back in, inherited by its owner (or longest-standing member) who may inherit
    if let (true, true, Some(org)) = (plans.is_empty(), reapply, OrgId::parse(&row.org)) {
        let members: Vec<OrgMember> = q::ORG_MEMBERSHIPS.on(&state.control).bind(("org", org.record())).await?.take(0)?;
        let staying: Vec<&OrgMember> = members.iter().filter(|m| &m.user != subject && !excluded.contains(&m.user.to_string())).collect();
        let heir = staying.iter().find(|m| m.role == "owner").or(staying.first()).map(|m| m.user.clone());
        plans.push(super::OrgPlan { org, heir, close: false, transfer: false, joint: Vec::new() });
    }
    let mut orgs = Vec::new();
    for p in plans {
        if !p.joint.is_empty() {
            // everyone left in this workspace is being erased: the last of them closes it
            q::ERASURE_JOINT_CLOSE.on(&state.control).bind(("okey", p.org.key())).bind(("others", p.joint.clone())).await?.check()?;
        }
        let org_state = match state.org(&p.org).await {
            Ok(s) => s,
            Err(e) if e.message.contains("closed") => continue,
            Err(e) => return Err(e),
        };
        let db = &org_state.db;
        let others: Vec<MemberRow> = q::OTHER_MEMBERS.on(db).bind(("u", subject.clone())).await?.take(0)?;
        // every space someone else reads stays; its heir is the first reader who may inherit (none: the
        // person's own rows there go, everyone else's stay)
        let mut heir_of: HashMap<String, Option<(RecordId, bool)>> = HashMap::new();
        for m in others {
            let eligible = !excluded.contains(&m.user.to_string());
            let entry = heir_of.entry(m.vault).or_insert(None);
            if entry.is_none() && eligible {
                *entry = Some((m.user, m.role == "owner"));
            }
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
                .map(|(v, h)| match h {
                    Some((to, is_owner)) => Kept { vault: v.clone(), to: to.to_string(), promote: my_owned.contains(v.as_str()) && !is_owner },
                    None => Kept { vault: v.clone(), to: String::new(), promote: false },
                })
                .collect();
            let mine_all: HashSet<&str> = mine.iter().map(|m| m.vault.as_str()).collect();
            w.member_of = w.kept.iter().filter(|k| mine_all.contains(k.vault.as_str())).map(|k| k.vault.clone()).collect();
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

/// The org post-check for one workspace: the residue statement, the files, and for a closed
/// workspace every table empty.
async fn org_check(state: &OrgState, subject: &RecordId, o: &OrgWork, work: &Work, email_lc: &str) -> Result<Value, AppError> {
    let solo: Vec<RecordId> = o.solo.iter().filter_map(|v| crate::rid::parse(v).ok()).collect();
    let kept: Vec<RecordId> = o.member_of.iter().filter_map(|v| crate::rid::parse(v).ok()).collect();
    let mut res = q::RESIDUE
        .on(&state.db)
        .bind(("u", subject.clone()))
        .bind(("actors", work.actors.clone()))
        .bind(("email_lc", email_lc.to_string()))
        .bind(("solo", solo))
        .bind(("kept", kept))
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
    for k in o.kept.iter().filter(|k| k.promote && !k.to.is_empty()) {
        q::PROMOTE.on(db).bind(("vault", rid(&k.vault)?)).bind(("to", rid(&k.to)?)).await?.check()?;
        *counts.entry("ownership_handed_over".into()).or_insert(0) += 1;
    }
    let with_rows: HashSet<String> = q::VAULTS_WITH_ROWS.on(db).bind(("u", subject.clone())).await?.take::<Vec<String>>(0)?.into_iter().collect();
    // a kept space with nobody who may inherit: the person's own rows there are deleted below (ERASE_OWNED)
    for k in o.kept.iter().filter(|k| with_rows.contains(&k.vault) && !k.to.is_empty()) {
        let (vault, to) = (rid(&k.vault)?, rid(&k.to)?);
        let mut res = crate::tx::with_retry(|| async { q::REASSIGN.on(db).bind(("u", subject.clone())).bind(("vault", vault.clone())).bind(("to", to.clone())).bind(("email_re", email_re.to_string())).await }).await?;
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
    let kept: Vec<RecordId> = o.member_of.iter().filter_map(|v| crate::rid::parse(v).ok()).collect();
    let targets: Vec<Value> = q::SCRUB_TARGETS.on(db).bind(("email_lc", email_lc.to_string())).await?.take::<Vec<Value>>(0)?;
    let mut res = q::SCRUB_EMAIL.on(db).bind(("email_re", email_re.to_string())).bind(("email_lc", email_lc.to_string())).bind(("kept", kept)).await?;
    counts.insert("email_scrubbed".into(), res.take::<Option<i64>>(q::SCRUB_EMAIL.slot)?.unwrap_or(0));
    let mut updated: BTreeMap<String, Vec<(&'static str, &'static str, RecordId)>> = BTreeMap::new();
    for it in &targets {
        let (Some(id), Some(vault)) = (it.get("id").and_then(Value::as_str), it.get("vault").and_then(Value::as_str)) else { continue };
        let kind = if it.get("kind").and_then(Value::as_str) == Some("memory") { crate::changes::MEMORY } else { crate::changes::ENTITY };
        if let Ok(rid) = crate::rid::parse(id) {
            updated.entry(vault.to_string()).or_default().push((crate::changes::UPDATED, kind, rid));
        }
    }
    for (vault, rows) in updated {
        if let Ok(v) = crate::rid::parse(&vault) {
            crate::changes::record_many(db, &v, rows).await;
        }
    }

    // the person's own rows everywhere else, the bucket files, the actor names, the embed cache
    if objects {
        for path in &o.files {
            crate::documents::delete_object(db, path).await?;
        }
        counts.insert("bucket_files".into(), o.files.len() as i64);
    }
    // the same content-free `deleted` change rows a normal delete writes, so sync clients drop their copies
    let items: Vec<Value> = q::CHANGE_ITEMS.on(db).bind(("u", subject.clone())).bind(("solo", solo.clone())).await?.take::<Vec<Value>>(0)?;
    let mut by_vault: BTreeMap<String, Vec<(&'static str, &'static str, RecordId)>> = BTreeMap::new();
    for it in &items {
        let (Some(id), Some(vault), Some(kind)) = (it.get("id").and_then(Value::as_str), it.get("vault").and_then(Value::as_str), it.get("kind").and_then(Value::as_str)) else { continue };
        let kind = match kind {
            "memory" => crate::changes::MEMORY,
            "relation" => crate::changes::RELATION,
            "skill" => crate::changes::SKILL,
            "document" => crate::changes::DOCUMENT,
            _ => crate::changes::ENTITY,
        };
        if let Ok(rid) = crate::rid::parse(id) {
            by_vault.entry(vault.to_string()).or_default().push((crate::changes::DELETED, kind, rid));
        }
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
    for (vault, rows) in by_vault {
        if let Ok(v) = crate::rid::parse(&vault) {
            counts.entry("change_log_rows".into()).and_modify(|n| *n += rows.len() as i64).or_insert(rows.len() as i64);
            crate::changes::record_many(db, &v, rows).await;
        }
    }
    for vault in &solo {
        let audience = crate::changes::audience(db, vault).await;
        crate::tx::with_retry(|| async {
            crate::store::vaults::DELETE_VAULT.on(db).bind(("vault", vault.clone())).bind(("counter", crate::changes::counter(vault))).await?.check()
        })
        .await?;
        // as a normal space delete does: everyone who had it hears it is gone
        crate::changes::announce(db, vault, crate::changes::VAULT_DELETED, &audience).await;
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
        let n: Vec<N> = crate::store::dynamic(db, "subject.closed_org_count", format!("SELECT count() FROM {t}{} GROUP ALL", kept_where(t))).await?.take(0)?;
        rows += n.first().map_or(0, |n| n.count);
    }
    Ok(rows)
}

/// Empties every org table (a workspace that closes with its only member), except completed access
/// reviews and their decisions: tenant 0044 freezes them (any update or delete is refused), so they stay
/// in the closed database, as the access-control record they are, until an operator drops it.
pub async fn empty_all_tables(db: &crate::pool::OrgDb) -> Result<(), AppError> {
    for t in crate::store::TENANT_TABLES {
        crate::store::dynamic(db, "subject.closed_org_delete", format!("DELETE {t}{} RETURN NONE", kept_where(t))).await?.check()?;
    }
    Ok(())
}

/// The rows of `table` a closing workspace may remove: all, but for the frozen completed access reviews.
fn kept_where(table: &str) -> &'static str {
    match table {
        "access_review" => " WHERE status != 'completed'",
        "access_review_decision" => " WHERE review.status != 'completed'",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_set_aside_account_is_known_by_its_kept_address() {
        assert_eq!(sign_in_address("Alice@Example.com"), "alice@example.com");
        assert_eq!(sign_in_address("set-aside:user:abc (was Alice@Example.com)"), "alice@example.com");
        assert_eq!(sign_in_address("set-aside:user:abc"), "set-aside:user:abc");
    }

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

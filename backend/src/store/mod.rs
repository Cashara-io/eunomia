//! Every SurrealQL statement the app runs lives here as a named constant.
//! Request code calls `STMT.on(&db).bind(..).await`; the name tags the
//! query span and a `-- op:<name> trace:<id>` comment, so SurrealDB's slow
//! query log joins the request trace. `all()` feeds the
//! `every_query_executes` test, which runs each statement against a freshly
//! migrated database.

use std::future::{Future, IntoFuture};
use std::marker::PhantomData;
use std::pin::Pin;
use std::time::Instant;

use surrealdb::engine::any::Any;
use surrealdb::IndexedResults;
use surrealdb::method::{IntoVariables, Query};
use surrealdb::types::{RecordId, SurrealValue};
use tracing::Instrument;

use crate::db::Db;
use crate::pool::{ControlDb, OrgDb};

pub mod app;
pub mod capsules;
pub mod cache;
pub mod changes;
pub mod control;
pub mod documents;
pub mod entities;
pub mod extensions;
pub mod jobs;
pub mod redact;
pub mod routing;
pub mod secrets;
pub mod skills;
pub mod tenant;
pub mod vaults;

/// Proof of being inside `store/`: its field is private to this module and its children, so only
/// they can build one, and `ControlDb::raw` / `OrgDb::raw` take it.
pub(crate) struct RawKey(());

/// Marker: the statement runs in an org's database (`Stmt::on` takes an `&OrgDb`).
pub struct Tenant;
/// Marker: the statement runs in the control database (`Stmt::on` takes a `&ControlDb`).
pub struct Control;

pub struct Stmt<S = Tenant> {
    pub name: &'static str,
    pub sql: &'static str,
    /// Index of the statement whose rows the caller wants (`res.take(stmt.slot)`). SurrealDB 3.x
    /// gives every statement a result slot, BEGIN, LET and COMMIT included, so a transaction's
    /// `RETURN` is not slot 0.
    pub slot: usize,
    scope: PhantomData<fn() -> S>,
}

/// A statement for the control database.
pub type ControlStmt = Stmt<Control>;

impl<S> Stmt<S> {
    pub const fn new(name: &'static str, sql: &'static str) -> Self {
        Stmt { name, sql, slot: 0, scope: PhantomData }
    }

    pub const fn at(name: &'static str, sql: &'static str, slot: usize) -> Self {
        Stmt { name, sql, slot, scope: PhantomData }
    }
}

impl Stmt<Tenant> {
    pub fn on<'a>(&self, db: &'a OrgDb) -> Q<'a> {
        Q::new(db.raw(RawKey(())), Scope::Org(db.org()), self.name, self.sql)
    }
}

impl Stmt<Control> {
    pub fn on<'a>(&self, db: &'a ControlDb) -> Q<'a> {
        Q::new(db.raw(RawKey(())), Scope::Control, self.name, self.sql)
    }
}

/// Names of the dynamic statements built so far in this process. `tests/dynamic_queries.rs` drives every
/// builder and then checks this against the names found in the source, so a new one cannot go untested.
#[cfg(feature = "test-support")]
pub static DYNAMIC_SEEN: std::sync::Mutex<std::collections::BTreeSet<&'static str>> = std::sync::Mutex::new(std::collections::BTreeSet::new());

/// Statements sent to the database since the process started; `tests/round_trips.rs` counts them per request.
#[cfg(feature = "test-support")]
pub static SENT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// For the few statements whose text is built at runtime (a table name or
/// an optional clause). Still named, so they still trace.
// ponytail: not covered by every_query_executes; `tests/dynamic_queries.rs` drives each builder instead. Keep these rare.
pub fn dynamic<'a>(db: &'a OrgDb, name: &'static str, sql: impl AsRef<str>) -> Q<'a> {
    #[cfg(feature = "test-support")]
    DYNAMIC_SEEN.lock().unwrap().insert(name);
    Q::new(db.raw(RawKey(())), Scope::Org(db.org()), name, sql.as_ref())
}

/// [`dynamic`] for the control database.
pub fn dynamic_control<'a>(db: &'a ControlDb, name: &'static str, sql: impl AsRef<str>) -> Q<'a> {
    #[cfg(feature = "test-support")]
    DYNAMIC_SEEN.lock().unwrap().insert(name);
    Q::new(db.raw(RawKey(())), Scope::Control, name, sql.as_ref())
}

/// A statement on a root session (provisioning) or the migration runner: either table set, no
/// scope check, no isolation transforms.
pub(crate) fn root<'a>(db: &'a Db, name: &'static str, sql: impl AsRef<str>) -> Q<'a> {
    Q::new(db, Scope::Root, name, sql.as_ref())
}

/// One record by id, if it exists (the replacement for the SDK's `select(id)`).
pub async fn get<T: SurrealValue>(db: &OrgDb, id: &RecordId) -> surrealdb::Result<Option<T>> {
    GET.on(db).bind(("id", id.clone())).await?.take(0)
}

/// [`get`] in the control database.
pub async fn get_control<T: SurrealValue>(db: &ControlDb, id: &RecordId) -> surrealdb::Result<Option<T>> {
    CONTROL_GET.on(db).bind(("id", id.clone())).await?.take(0)
}

const GET: Stmt = Stmt::new("store.get", "SELECT * FROM ONLY $id");
const CONTROL_GET: ControlStmt = Stmt::new("store.control_get", "SELECT * FROM ONLY $id");

#[derive(Clone, Copy)]
pub(crate) enum Scope {
    Org(crate::pool::OrgId),
    Control,
    Root,
}

/// In the error text of a statement refused for naming the other database's tables; `AppError` maps it
/// to `tenant.denied`.
pub const CROSSING_MARK: &str = "reaches across the tenant boundary";

pub struct Q<'a> {
    name: &'static str,
    inner: Query<'a, Any>,
    scope: Scope,
    /// Set when the text names a table of the other database: the await fails and nothing is sent.
    denied: Option<String>,
}

impl<'a> Q<'a> {
    fn new(db: &'a Db, scope: Scope, name: &'static str, sql: &str) -> Self {
        let trace = crate::telemetry::current_trace_id();
        #[cfg(feature = "test-support")]
        let transformed = crate::isolation::transform(name, sql, matches!(scope, Scope::Org(_)));
        #[cfg(feature = "test-support")]
        let sql = &*transformed;
        let crossing = match scope {
            Scope::Org(_) => crossing_table(sql, true),
            Scope::Control => crossing_table(sql, false),
            Scope::Root => None,
        };
        let denied = crossing.map(|table| {
            crate::pool::no_org_context(&format!("{name} reaches for {table} in the wrong database"));
            format!("statement {name} {CROSSING_MARK}: it names {table}, which lives in the other database")
        });
        Q { name, inner: db.query(format!("-- op:{name} trace:{trace}\n{sql}")), scope, denied }
    }

    pub fn bind(mut self, bindings: impl IntoVariables) -> Self {
        self.inner = self.inner.bind(bindings);
        self
    }
}

impl<'a> IntoFuture for Q<'a> {
    type Output = surrealdb::Result<IndexedResults>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        if let Some(why) = self.denied {
            return Box::pin(async move { Err(surrealdb::Error::thrown(why)) });
        }
        let org = match self.scope {
            Scope::Org(o) => o.key(),
            _ => String::new(),
        };
        #[cfg(feature = "test-support")]
        SENT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let span = tracing::info_span!("store.query", op = self.name, org = %org, duration_ms = tracing::field::Empty, ok = tracing::field::Empty);
        let limited = !matches!(self.scope, Scope::Root);
        Box::pin(
            async move {
                // provisioning and migrations (root) are rare and may run long: they do not queue
                let _slot = if limited { in_flight().acquire().await.ok() } else { None };
                let started = Instant::now();
                let res = self.inner.await.and_then(surface_root_cause);
                let span = tracing::Span::current();
                span.record("duration_ms", started.elapsed().as_millis() as u64);
                span.record("ok", res.is_ok());
                res
            }
            .instrument(span),
        )
    }
}

/// Statements one process has at the database at once (`EUNOMIA_DB_INFLIGHT`, default 32); the rest
/// wait here in arrival order. Past the server's cores more parallel queries only share its CPU, so a
/// heavy one (a full-text recall arm, an entity scan) finishes ever later behind a crowd of light ones
/// and requests time out at the tail; queueing in order keeps the tail close to the median. A permit
/// is held for one statement only, never while waiting for another, so it cannot deadlock.
fn in_flight() -> &'static tokio::sync::Semaphore {
    static SLOTS: std::sync::OnceLock<tokio::sync::Semaphore> = std::sync::OnceLock::new();
    SLOTS.get_or_init(|| {
        let n = std::env::var("EUNOMIA_DB_INFLIGHT").ok().and_then(|v| v.trim().parse().ok()).filter(|n: &usize| *n > 0).unwrap_or(32);
        tokio::sync::Semaphore::new(n)
    })
}

/// When a transaction fails to commit, SurrealDB marks every statement in it "not executed due to a
/// failed transaction" and puts the real error (a commit conflict, a unique violation) on a later
/// one. `IndexedResults::check` returns the first, so `tx::is_conflict` never saw the conflict and a
/// lost race surfaced as a 500 instead of a retry. Return the real error from the await itself.
/// Any statement error becomes the `Err` of the await (the same error `.check()` would have given,
/// minus the shadowing), so a batch with a failed statement is never half-read.
fn surface_root_cause(mut res: surrealdb::IndexedResults) -> surrealdb::Result<surrealdb::IndexedResults> {
    let mut errors: Vec<(usize, surrealdb::Error)> = res.take_errors().into_iter().collect();
    if errors.is_empty() {
        return Ok(res);
    }
    errors.sort_by_key(|(i, _)| *i);
    let shadow = |e: &surrealdb::Error| e.to_string().contains("not executed due to a failed transaction");
    let pick = errors
        .iter()
        .position(|(_, e)| crate::tx::is_conflict(e))
        .or_else(|| errors.iter().position(|(_, e)| !shadow(e)))
        .unwrap_or(0);
    Err(errors.swap_remove(pick).1)
}

/// Tables that live in the control database. Everything else a statement names is an org table.
pub const CONTROL_TABLES: &[&str] = &[
    "user", "identity", "org", "membership", "tenant", "api_token", "session", "oauth_client", "oauth_code", "oauth_grant", "oauth_token", "audit_event", "tool_usage", "job",
    "job_leader", "failure_capsule",
];

/// Tables that live in an org database.
pub const TENANT_TABLES: &[&str] = &[
    "app_settings", "vault", "vault_member", "connector", "sync_status", "cache_record", "linked_to", "person", "organisation", "location", "repository", "entity",
    "file", "symbol", "memory", "relates_to", "chat_thread", "chat_message", "audit_log", "embed_cache", "pocket_recording",
    "document", "extension", "extension_event", "skill", "change", "change_seq", "redact_backfill",
    "credential_ref", "secret_provider",
];

/// The first table `sql` names that lives in the other database. A table is recognised where
/// SurrealQL puts one: after FROM, INTO, UPDATE, DELETE, CREATE, UPSERT, RELATE, TABLE, ONLY and ON
/// (and every item of a comma list that follows), as a record literal (`user:abc`), a graph edge
/// segment (`->job`, `<-membership`) or the table argument of `type::thing`/`type::record`/
/// `type::table`. A field that happens to share a table's name (`user.email`, `{ user: $u }`) is
/// not one.
// ponytail: a tokenizer, not a parser -- defense in depth; OrgDb/ControlDb typing does the real
// scoping. A table reached only through a variable or a computed string is not seen.
pub fn crossing_table(sql: &str, in_org: bool) -> Option<String> {
    let foreign = if in_org { CONTROL_TABLES } else { TENANT_TABLES };
    let hit = |t: &str| {
        let t = t.trim_matches(|c| matches!(c, '`' | '"' | '\'' | '?' | '$'));
        foreign.contains(&t).then(|| t.to_string())
    };
    let spaced = sql.replace(',', " , ");
    let words: Vec<&str> = spaced
        .split(|c: char| c.is_whitespace() || matches!(c, ';' | '(' | ')' | '{' | '}' | '[' | ']'))
        .filter(|w| !w.is_empty())
        .collect();
    let (mut expect_table, mut in_list) = (false, false);
    let mut prev = "";
    for w in &words {
        let upper = w.to_ascii_uppercase();
        if *w == "," {
            expect_table = in_list;
            continue;
        }
        // `table:key` record literal anywhere (not `fn::name`, not an object key `user:`)
        if let Some((table, key)) = w.split_once(':')
            && !key.is_empty()
            && !key.starts_with(':')
            && let Some(t) = hit(table)
        {
            return Some(t);
        }
        // graph edges: every segment after the first
        if (w.contains("->") || w.contains("<-"))
            && let Some(t) = w.split("->").flat_map(|p| p.split("<-")).skip(1).find_map(|seg| hit(seg.split(':').next().unwrap_or("")))
        {
            return Some(t);
        }
        let prev_lc = prev.to_ascii_lowercase();
        if matches!(prev_lc.as_str(), "type::thing" | "type::record" | "type::table")
            && let Some(t) = hit(w)
        {
            return Some(t);
        }
        if expect_table {
            if let Some(t) = hit(w.split(':').next().unwrap_or("")) {
                return Some(t);
            }
            in_list = true;
            expect_table = false;
        } else {
            in_list = false;
        }
        if matches!(upper.as_str(), "FROM" | "INTO" | "UPDATE" | "DELETE" | "CREATE" | "UPSERT" | "RELATE" | "TABLE" | "ONLY" | "ON") {
            expect_table = true;
            in_list = false;
        }
        prev = w;
    }
    None
}

/// Every static tenant statement, for the `every_query_executes` test.
pub fn all() -> Vec<&'static Stmt> {
    [app::ALL, cache::ALL, changes::ALL, documents::ALL, entities::ALL, extensions::ALL, jobs::ALL, redact::ALL, routing::ALL, secrets::ALL, skills::ALL, vaults::ALL].concat()
}

/// Every static control statement.
pub fn all_control() -> Vec<&'static ControlStmt> {
    [control::ALL, tenant::ALL, capsules::ALL, jobs::CONTROL_ALL].concat()
}

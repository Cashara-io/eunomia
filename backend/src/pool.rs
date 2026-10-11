//! The two handles request code is allowed to hold, and the pool that hands them out.
//!
//! * [`ControlDb`]: the `control` database (accounts, credentials, the job queue, the tenant routing
//!   table). One per process.
//! * [`OrgDb`]: one org's database. Only [`Pool::for_org`] makes one, signed in as that org's
//!   database user, so a query on it cannot see another org's data even if a filter is missing.
//!
//! The raw client (`Db`) is a private field here; `Stmt::on` takes the handle that matches the
//! statement's scope, so the compiler decides which database a statement runs in. Nothing in this
//! module or the request path signs in as root: that is `provisioning/` alone.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;
use surrealdb::opt::auth::Database;
use surrealdb::types::SurrealValue;
use uuid::Uuid;

use crate::config::Settings;
use crate::db::Db;
use crate::error::{AppError, AppResult, ErrorCode};

/// Name of the control database.
pub const CONTROL_DB: &str = "control";
/// The database user every handle signs in as, in the control database and in each org database.
pub const DB_USER: &str = "app";

/// Queries that ran without an org context: an unknown or unready org asked for, or a statement that
/// reached for the other scope's tables. Logged at error level where it is counted; the isolation
/// suite asserts it stays 0, and `/api/debug/metrics` exposes it.
pub static NO_ORG_CONTEXT: AtomicU64 = AtomicU64::new(0);

pub fn no_org_context(what: &str) {
    NO_ORG_CONTEXT.fetch_add(1, Ordering::Relaxed);
    tracing::error!(what, "query without org context");
}

/// An organisation's id. Its data lives in the database `org_<32 hex>`, and its control records are
/// `org:<32 hex>` and `tenant:<32 hex>`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct OrgId(Uuid);

impl OrgId {
    pub fn new() -> Self {
        OrgId(Uuid::new_v4())
    }

    /// A stable id derived from a label: two processes that start the same job at once agree on the
    /// org it creates (the self-host move uses this, so a simultaneous boot cannot make two orgs).
    pub fn from_label(label: &str) -> Self {
        use sha2::{Digest, Sha256};
        let hash = Sha256::digest(label.as_bytes());
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&hash[..16]);
        OrgId(Uuid::from_bytes(bytes))
    }

    pub fn parse(s: &str) -> Option<Self> {
        let s = s.strip_prefix("org:").unwrap_or(s);
        Uuid::parse_str(s).ok().map(OrgId)
    }

    /// The record key: the uuid as 32 hex digits.
    pub fn key(&self) -> String {
        self.0.simple().to_string()
    }

    pub fn db_name(&self) -> String {
        format!("org_{}", self.key())
    }

    pub fn record(&self) -> surrealdb::types::RecordId {
        surrealdb::types::RecordId::new("org", self.key())
    }

    pub fn tenant_record(&self) -> surrealdb::types::RecordId {
        surrealdb::types::RecordId::new("tenant", self.key())
    }
}

impl Default for OrgId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for OrgId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.key())
    }
}

impl serde::Serialize for OrgId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.key())
    }
}

impl<'de> Deserialize<'de> for OrgId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        OrgId::parse(&s).ok_or_else(|| serde::de::Error::custom("bad org id"))
    }
}

/// One database signed in once per server connection. SurrealDB works through one connection's
/// requests largely one at a time, so a single connection caps the whole process at what one core does
/// (measured: a 10 ms query ran ~100/s over 1 connection, ~310/s over 8). Statements take the
/// connections in turn; each is a self-contained query, so none depends on which one it ran on.
pub(crate) struct Sessions {
    dbs: Vec<Db>,
    next: AtomicUsize,
}

impl Sessions {
    fn new(dbs: Vec<Db>) -> Self {
        Sessions { dbs, next: AtomicUsize::new(0) }
    }

    fn pick(&self) -> &Db {
        &self.dbs[self.next.fetch_add(1, Ordering::Relaxed) % self.dbs.len()]
    }

    /// The first session: for tests that run raw statements, on any one connection.
    #[cfg(feature = "test-support")]
    fn first(&self) -> &Db {
        &self.dbs[0]
    }
}

/// The control database, signed in as its database user.
#[derive(Clone)]
pub struct ControlDb {
    raw: Arc<Sessions>,
}

/// One org's database, signed in as that org's database user.
#[derive(Clone)]
pub struct OrgDb {
    raw: Arc<Sessions>,
    org: OrgId,
}

impl ControlDb {
    /// The raw client. The `RawKey` token can only be made inside `store/`, so nothing else compiles a call.
    pub(crate) fn raw(&self, _: crate::store::RawKey) -> &Db {
        self.raw.pick()
    }

    #[cfg(feature = "test-support")]
    pub fn test_raw(&self) -> &Db {
        self.raw.first()
    }
}

impl OrgDb {
    pub(crate) fn raw(&self, _: crate::store::RawKey) -> &Db {
        self.raw.pick()
    }

    pub fn org(&self) -> OrgId {
        self.org
    }

    #[cfg(feature = "test-support")]
    pub fn test_raw(&self) -> &Db {
        self.raw.first()
    }
}

/// HMAC of a fixed label under the encryption key: the control database user's password. Every
/// process of one install derives the same value, so nothing about it is stored. Only as secret as the
/// key: boot refuses an empty one (`crypto::require_key`) unless the dev-only opt-in is set.
pub fn control_password(settings: &Settings) -> String {
    derived_password(&settings.encryption_key, "eunomia/control-db-user")
}

/// HMAC of `label` under `key`, hex: a database password every process of an install derives alike.
pub fn derived_password(key: &str, label: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes()).expect("hmac takes any key length");
    mac.update(label.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// A random database-user password: alphanumeric only, so it is safe to inline in `DEFINE USER`.
pub fn generate_db_password() -> String {
    use rand::Rng;
    const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut rng = rand::thread_rng();
    (0..40).map(|_| CHARS[rng.gen_range(0..CHARS.len())] as char).collect()
}

/// Where one org's data lives and how to sign in to it.
#[derive(Clone, Debug)]
pub(crate) struct Route {
    pub db: String,
    pub user: String,
    pub pass: String,
}

/// Signs a fresh session in to one database as a database-level user. Fails if the credentials are
/// wrong; a database user cannot `USE` any other database.
pub(crate) async fn session_for(template: &Db, ns: &str, route: &Route) -> surrealdb::Result<Db> {
    let session = template.clone();
    session
        .signin(Database { namespace: ns.to_string(), database: route.db.clone(), username: route.user.clone(), password: route.pass.clone() })
        .await?;
    session.use_ns(ns).use_db(&route.db).await?;
    Ok(session)
}

/// [`session_for`] on every connection, signed in side by side.
async fn sessions_for(templates: &[Db], ns: &str, route: &Route) -> surrealdb::Result<Sessions> {
    let dbs = futures::future::try_join_all(templates.iter().map(|t| session_for(t, ns, route))).await?;
    Ok(Sessions::new(dbs))
}

pub(crate) async fn connect_control(templates: &[Db], settings: &Settings) -> surrealdb::Result<ControlDb> {
    let route = Route { db: CONTROL_DB.into(), user: DB_USER.into(), pass: control_password(settings) };
    Ok(ControlDb { raw: Arc::new(sessions_for(templates, &settings.surreal_ns, &route).await?) })
}

struct Cache {
    tick: u64,
    map: HashMap<OrgId, (OrgDb, u64)>,
}

struct Inner {
    /// One unauthenticated client per server connection (see [`Sessions`]).
    templates: Vec<Db>,
    control: ControlDb,
    ns: String,
    key: String,
    cap: usize,
    cache: Mutex<Cache>,
    /// One lock per org with a sign-in under way, so a burst of cold requests signs in once.
    inflight: Mutex<HashMap<OrgId, Arc<tokio::sync::Mutex<()>>>>,
    /// Sign-ins done on a cache miss (the singleflight test counts them).
    signins: AtomicU64,
    /// Isolation suite only: every org gets this one database (the "shared database" mode).
    #[cfg(feature = "test-support")]
    shared: Mutex<Option<Route>>,
}

/// Per-org connection cache with an LRU cap. `for_org` is the only way to get an [`OrgDb`].
#[derive(Clone)]
pub struct Pool(Arc<Inner>);

/// What a tenant row says, minus the password plaintext.
#[derive(Deserialize, SurrealValue)]
struct TenantRow {
    db: String,
    db_user: String,
    db_pass_enc: String,
    schema_version: i64,
    status: String,
}

impl Pool {
    pub(crate) fn new(templates: Vec<Db>, control: ControlDb, settings: &Settings) -> Self {
        let cap = std::env::var("EUNOMIA_ORG_POOL_CAP").ok().and_then(|v| v.parse().ok()).filter(|n| *n > 0).unwrap_or(256);
        Pool(Arc::new(Inner {
            templates,
            control,
            ns: settings.surreal_ns.clone(),
            key: settings.encryption_key.clone(),
            cap,
            cache: Mutex::new(Cache { tick: 0, map: HashMap::new() }),
            inflight: Mutex::new(HashMap::new()),
            signins: AtomicU64::new(0),
            #[cfg(feature = "test-support")]
            shared: Mutex::new(None),
        }))
    }

    /// The control handle this pool reads routes from.
    pub fn control(&self) -> &ControlDb {
        &self.0.control
    }

    /// The org's database handle. Errors with `tenant.not_found` for an org with no ready route (and
    /// counts it as a query without org context) and `tenant.schema_behind` below schema N-1.
    pub async fn for_org(&self, org: &OrgId) -> AppResult<OrgDb> {
        if let Some(db) = self.cached(org) {
            return Ok(db);
        }
        // Singleflight: one task per org does the control read, decrypt and sign-in (the sign-in is a
        // password hash check, ~200 ms); the rest wait on the org's lock and then find it cached.
        let gate = self.0.inflight.lock().unwrap().entry(*org).or_default().clone();
        let _turn = gate.lock().await;
        let result = async {
            if let Some(db) = self.cached(org) {
                return Ok(db);
            }
            let route = self.route(org).await?;
            self.0.signins.fetch_add(1, Ordering::Relaxed);
            let sessions = sessions_for(&self.0.templates, &self.0.ns, &route).await?;
            let db = OrgDb { raw: Arc::new(sessions), org: *org };
            self.insert(db.clone());
            Ok(db)
        }
        .await;
        // drop the map entry so it does not grow with every org ever seen; a task already waiting holds its own Arc
        let mut inflight = self.0.inflight.lock().unwrap();
        if inflight.get(org).is_some_and(|g| Arc::ptr_eq(g, &gate)) {
            inflight.remove(org);
        }
        result
    }

    /// Sign-ins done on a cache miss since the pool was built.
    pub fn signins(&self) -> u64 {
        self.0.signins.load(Ordering::Relaxed)
    }

    fn cached(&self, org: &OrgId) -> Option<OrgDb> {
        let mut c = self.0.cache.lock().unwrap();
        c.tick += 1;
        let tick = c.tick;
        c.map.get_mut(org).map(|(db, used)| {
            *used = tick;
            db.clone()
        })
    }

    fn insert(&self, db: OrgDb) {
        let mut c = self.0.cache.lock().unwrap();
        c.tick += 1;
        let tick = c.tick;
        c.map.insert(db.org, (db, tick));
        // ponytail: eviction scans the map for the oldest tick (O(cap)); a linked LRU if the cap goes into the thousands.
        while c.map.len() > self.0.cap {
            let Some(oldest) = c.map.iter().min_by_key(|(_, (_, used))| *used).map(|(k, _)| *k) else { break };
            c.map.remove(&oldest);
        }
    }

    /// Forget a cached handle (credentials rotated, org deleted).
    pub fn evict(&self, org: &OrgId) {
        self.0.cache.lock().unwrap().map.remove(org);
    }

    /// Handles currently open.
    pub fn open_handles(&self) -> usize {
        self.0.cache.lock().unwrap().map.len()
    }

    async fn route(&self, org: &OrgId) -> AppResult<Route> {
        #[cfg(feature = "test-support")]
        if let Some(r) = self.0.shared.lock().unwrap().clone() {
            return Ok(r);
        }
        let mut res = crate::store::tenant::BY_ORG.on(&self.0.control).bind(("id", org.tenant_record())).await?;
        let Some(row) = res.take::<Vec<TenantRow>>(0)?.into_iter().next() else {
            no_org_context("for_org: unknown org");
            return Err(AppError::coded(ErrorCode::TenantNotFound, "No data store exists for this organisation."));
        };
        if row.status != "ready" {
            no_org_context("for_org: org not ready");
            return Err(AppError::coded(ErrorCode::TenantNotFound, "This organisation's data store is being set up."));
        }
        if row.schema_version < crate::migrate::MIN_SUPPORTED_TENANT as i64 {
            return Err(AppError::coded(
                ErrorCode::TenantSchemaBehind,
                "This organisation's database is being upgraded. Retry shortly.",
            ));
        }
        let pass = crate::connectors::crypto::decrypt(&self.0.key, &row.db_pass_enc)?;
        Ok(Route { db: row.db, user: row.db_user, pass })
    }

    /// Isolation suite: point every org at one database.
    #[cfg(feature = "test-support")]
    pub fn set_shared_database(&self, db: &str, user: &str, pass: &str) {
        *self.0.shared.lock().unwrap() = Some(Route { db: db.into(), user: user.into(), pass: pass.into() });
        self.0.cache.lock().unwrap().map.clear();
    }

    /// Isolation suite: a handle signed in with arbitrary credentials (the credential-wall test).
    #[cfg(feature = "test-support")]
    pub async fn test_session(&self, org: OrgId, db: &str, user: &str, pass: &str) -> AppResult<OrgDb> {
        let route = Route { db: db.into(), user: user.into(), pass: pass.into() };
        Ok(OrgDb { raw: Arc::new(sessions_for(&self.0.templates[..1], &self.0.ns, &route).await?), org })
    }
}

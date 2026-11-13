use std::sync::Arc;

use crate::config::Settings;
use crate::error::AppResult;
use crate::pool::{ControlDb, OrgDb, OrgId, Pool};
use crate::provisioning::Provisioner;

#[derive(Clone)]
pub struct AppState(pub Arc<AppStateInner>);

pub struct AppStateInner {
    /// Accounts, credentials, the job queue, the tenant routing table.
    pub control: ControlDb,
    /// The only source of [`OrgDb`] handles.
    pub pool: Pool,
    /// Root-session holder; `None` in a build without the `provisioning` feature.
    pub provisioner: Option<Provisioner>,
    pub settings: Settings,
    pub composio_publishing: crate::connectors::composio::storage::Publishing,
    /// In-flight body reads shared by the HTTP and MCP upload routes.
    pub large_bodies: crate::ratelimit::LargeBodies,
    /// Failed sign-ins per account and per OAuth client (see `ratelimit::FailThrottle`).
    pub fail_throttle: crate::ratelimit::FailThrottle,
    /// Generic refusal budget per credential, with timer-flushed suppression summaries.
    pub(crate) scope_refusal_audit: crate::audit::scope_refusals::ScopeRefusals,
    /// Credentials verified in the last few seconds (see `auth::Credentials`).
    pub credentials: crate::auth::Credentials,
    /// `/readyz` result and when it was taken: the route is public, so it answers from here for 2 seconds.
    pub ready_cache: std::sync::Mutex<Option<(std::time::Instant, Option<String>)>>,
    /// OpenID Connect provider client: discovery and key-set caches (see `oidc.rs`).
    pub oidc: crate::oidc::Client,
}

impl std::ops::Deref for AppState {
    type Target = AppStateInner;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// The app plus one org's database: what a request (or a job) works with once it knows whose data
/// it touches. `state.db` is the org handle; `state.settings`, `state.control` and the rest come
/// from the app state.
#[derive(Clone)]
pub struct OrgState {
    pub app: AppState,
    pub db: OrgDb,
}

impl std::ops::Deref for OrgState {
    type Target = AppStateInner;
    fn deref(&self) -> &Self::Target {
        &self.app.0
    }
}

impl AppState {
    /// Connect, set up the control database (and move a pre-tenancy install's data, see
    /// `provisioning::legacy`), and build the pool. The one boot path for `main`, `replay` and tests.
    pub async fn build(settings: &Settings, config: surrealdb::opt::Config) -> AppResult<AppState> {
        // before anything derives a database password from the key
        crate::connectors::crypto::require_key(settings)?;
        // a gap in the registered migrations would skip a number for good (docs/audit-chain.md)
        crate::migrate::require_contiguous()?;
        crate::redact::init();
        crate::audit::chain::set_detail_key(&settings.encryption_key);
        let templates = crate::db::connect_all(settings, config).await?;
        let template = &templates[0];
        let provisioner = Provisioner::connect(template, settings);
        if let Some(p) = &provisioner {
            p.ensure_control().await?;
        }
        let control = crate::pool::connect_control(&templates, settings).await?;
        if let Some(p) = &provisioner {
            crate::provisioning::legacy::move_if_needed(p, &control, settings).await?;
            p.ensure_document_buckets(&control).await;
        }
        crate::connectors::crypto::guard_key(settings, &control).await?;
        match &provisioner {
            Some(p) => p.replace_exposed_db_passwords(&control).await?,
            None => crate::connectors::crypto::rotate_tenant_passwords(settings, &control).await?,
        };
        let pool = Pool::new(templates, control.clone(), settings);
        if let Some(p) = &provisioner {
            let repair = std::env::var("EUNOMIA_REPAIR_MIGRATIONS").is_ok_and(|v| v.trim() == "1");
            guard_skipped_migrations(p, &control, &pool, repair).await?;
        }
        let scope_refusal_audit = crate::audit::scope_refusals::ScopeRefusals::new(control.clone());
        // every process drains its own audit replay queue, leader or not
        crate::audit::chain::start_drain();
        Ok(AppState(Arc::new(AppStateInner { control, pool, provisioner, settings: settings.clone(), composio_publishing: Default::default(), large_bodies: Default::default(), fail_throttle: Default::default(), scope_refusal_audit, credentials: Default::default(), ready_cache: Default::default(), oidc: Default::default() })))
    }

    /// Connect to an install that is already set up, without provisioning or moving anything
    /// (`eunomia replay` reads a live database and must not change it).
    pub async fn attach(settings: &Settings, config: surrealdb::opt::Config) -> AppResult<AppState> {
        crate::connectors::crypto::require_key(settings)?;
        crate::audit::chain::set_detail_key(&settings.encryption_key);
        let templates = crate::db::connect_all(settings, config).await?;
        let control = crate::pool::connect_control(&templates, settings).await?;
        let pool = Pool::new(templates, control.clone(), settings);
        let scope_refusal_audit = crate::audit::scope_refusals::ScopeRefusals::new(control.clone());
        Ok(AppState(Arc::new(AppStateInner { control, pool, provisioner: None, settings: settings.clone(), composio_publishing: Default::default(), large_bodies: Default::default(), fail_throttle: Default::default(), scope_refusal_audit, credentials: Default::default(), ready_cache: Default::default(), oidc: Default::default() })))
    }

    /// The org's database, bundled with the app state.
    pub async fn org(&self, org: &OrgId) -> AppResult<OrgState> {
        Ok(OrgState { app: self.clone(), db: self.pool.for_org(org).await? })
    }
}

/// An org whose database applied a migration numbered above one it lacks is not served: with the
/// missing one unapplied its data may not be what the code expects, and nothing would ever apply
/// it. Each is reported by id on stderr and in the log. `repair` (`EUNOMIA_REPAIR_MIGRATIONS=1` at
/// boot) applies the missing migrations instead (they run after newer ones, so read their notes first);
/// see docs/audit-chain.md, "Migration numbers".
pub async fn guard_skipped_migrations(p: &Provisioner, control: &crate::pool::ControlDb, pool: &Pool, repair: bool) -> AppResult<()> {
    let gaps = p.find_migration_gaps(control).await?;
    for (org, missing) in gaps {
        let list = missing.iter().map(|v| format!("{v:04}")).collect::<Vec<_>>().join(", ");
        if repair {
            eprintln!("warning: org {} lacks tenant migration(s) {list} below ones it has; applying them now (EUNOMIA_REPAIR_MIGRATIONS=1)", org.key());
            p.migrate_org(control, &org).await?;
            pool.unblock(&org);
            continue;
        }
        let why = format!(
            "This organisation's database is missing migration(s) {list}, which newer ones were applied over. It is not served until an operator repairs it (docs/audit-chain.md, \"Migration numbers\")."
        );
        eprintln!("error: org {} lacks tenant migration(s) {list} below ones it has: not serving it. Restart once with EUNOMIA_REPAIR_MIGRATIONS=1 to apply them.", org.key());
        tracing::error!(org = %org.key(), "org database lacks a migration below its newest: not serving it");
        pool.block(&org, why);
    }
    Ok(())
}

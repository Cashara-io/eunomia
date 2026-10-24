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
    /// Failed sign-ins per account and per OAuth client (see `ratelimit::FailThrottle`).
    pub fail_throttle: crate::ratelimit::FailThrottle,
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
        Ok(AppState(Arc::new(AppStateInner { control, pool, provisioner, settings: settings.clone(), fail_throttle: Default::default(), credentials: Default::default(), ready_cache: Default::default(), oidc: Default::default() })))
    }

    /// Connect to an install that is already set up, without provisioning or moving anything
    /// (`eunomia replay` reads a live database and must not change it).
    pub async fn attach(settings: &Settings, config: surrealdb::opt::Config) -> AppResult<AppState> {
        crate::connectors::crypto::require_key(settings)?;
        let templates = crate::db::connect_all(settings, config).await?;
        let control = crate::pool::connect_control(&templates, settings).await?;
        let pool = Pool::new(templates, control.clone(), settings);
        Ok(AppState(Arc::new(AppStateInner { control, pool, provisioner: None, settings: settings.clone(), fail_throttle: Default::default(), credentials: Default::default(), ready_cache: Default::default(), oidc: Default::default() })))
    }

    /// The org's database, bundled with the app state.
    pub async fn org(&self, org: &OrgId) -> AppResult<OrgState> {
        Ok(OrgState { app: self.clone(), db: self.pool.for_org(org).await? })
    }
}

//! The raw SurrealDB client type and how to open one. The schema lives in `migrations/` (see
//! `migrate.rs`). Nothing outside `pool.rs` and `provisioning/` holds a `Db`: request code gets an
//! `OrgDb` or `ControlDb` (see `pool.rs`).

use surrealdb::engine::any::Any;
use surrealdb::opt::auth::Root;
use surrealdb::Surreal;

use crate::config::Settings;

/// `Any` so the same code runs over `ws://` in production and `mem://` in tests.
pub(crate) type Db = Surreal<Any>;

/// An unauthenticated connection. Every `clone()` of a 3.x `Surreal` is its own session (own
/// namespace, database and credentials), so the pool and the provisioner each take a clone and sign
/// in separately; this one is never signed in. The embedded in-memory engine (tests, the spike)
/// starts with authentication enforced, with the configured root user.
pub(crate) async fn connect_raw(settings: &Settings, config: surrealdb::opt::Config) -> surrealdb::Result<Db> {
    let config = if settings.surreal_url.starts_with("mem://") {
        config.user(Root { username: settings.surreal_user.clone(), password: settings.surreal_pass.clone() })
    } else {
        config
    };
    surrealdb::engine::any::connect((settings.surreal_url.as_str(), config)).await.map_err(|e| match old_server_hint(&e.to_string()) {
        Some(hint) => surrealdb::Error::internal(hint),
        None => e,
    })
}

/// Connections the pool opens to a server (`EUNOMIA_DB_CONNECTIONS`, default 8): every database
/// handle is signed in once on each, and statements take them in turn (see `pool::Sessions`). The
/// in-memory engine gets one (a second `mem://` would be a second, empty database).
pub(crate) async fn connect_all(settings: &Settings, config: surrealdb::opt::Config) -> surrealdb::Result<Vec<Db>> {
    let n = if settings.surreal_url.starts_with("mem://") {
        1
    } else {
        std::env::var("EUNOMIA_DB_CONNECTIONS").ok().and_then(|v| v.trim().parse().ok()).filter(|n: &usize| *n > 0).unwrap_or(8)
    };
    futures::future::try_join_all((0..n).map(|_| connect_raw(settings, config.clone()))).await
}

/// The client refuses a SurrealDB 2.x server with a bare "server version ... does not match" text.
/// Name the way out instead of leaving the operator to decode it.
fn old_server_hint(err: &str) -> Option<String> {
    let version = err.split_once("server version `")?.1.split('`').next()?;
    version.starts_with("2.").then(|| {
        format!(
            "the database server is SurrealDB {version} but Eunomia needs 3.x. Upgrade it with scripts/upgrade-surreal-v3.sh \
             (it exports first; see docs/upgrading-to-surrealdb-3.md), then start Eunomia again"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::old_server_hint;

    #[test]
    fn a_2x_server_gets_the_upgrade_hint_and_nothing_else_does() {
        let raw = "server version `2.3.7` does not match the range supported by the client `>=3.0.0-alpha.1, <4.0.0`";
        let hint = old_server_hint(raw).unwrap();
        assert!(hint.contains("scripts/upgrade-surreal-v3.sh") && hint.contains("docs/upgrading-to-surrealdb-3.md") && hint.contains("2.3.7"));
        assert!(old_server_hint(&raw.replace("2.3.7", "4.0.1")).is_none());
        assert!(old_server_hint("connection refused").is_none());
    }
}

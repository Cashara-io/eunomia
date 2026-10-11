//! The store seam's two gates: every named statement parses and runs against
//! a freshly migrated database, and no code outside `src/store/` builds
//! queries.

mod common;

use std::collections::BTreeSet;

use regex::Regex;
use serde_json::Value;

// Bound by SurrealDB itself, never by us.
const BUILTIN_PARAMS: &[&str] = &["value", "this", "parent", "before", "after", "auth", "session", "token", "input", "event"];

/// Runs every static statement once with NONE params and fails on the ones that do not parse or name
/// a function/field/table SurrealDB does not have. `run` binds and executes one statement.
async fn failures<S, F, Fut>(stmts: Vec<&'static S>, text_of: impl Fn(&S) -> (&'static str, &'static str), run: F) -> Vec<String>
where
    F: Fn(&'static S, Vec<String>) -> Fut,
    Fut: std::future::Future<Output = String>,
{
    let param = Regex::new(r"\$([A-Za-z_][A-Za-z0-9_]*)").unwrap();
    let mut broken = Vec::new();
    for stmt in stmts {
        let (name, sql) = text_of(stmt);
        let names: Vec<String> = param
            .captures_iter(sql)
            .map(|c| c.get(1).unwrap().as_str())
            .filter(|n| !BUILTIN_PARAMS.contains(n))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(String::from)
            .collect();
        // NONE params make many statements fail at runtime (type checks);
        // that is fine. What must never happen is a statement that does not
        // parse or names a function SurrealDB does not have.
        let text = run(stmt, names).await;
        let lower = text.to_lowercase();
        // SurrealDB 3.x also reports these at run time: a missing table (every table exists after
        // migrating), a field the schema lacks, a function the server's --allow-funcs denies.
        if ["parse error", "invalid function", "unknown function", "failed to parse", "does not exist", "no such field", "not allowed"]
            .iter()
            .any(|bad| lower.contains(bad))
        {
            broken.push(format!("{name}: {text}"));
        }
    }
    broken
}

async fn outcome(q: surrealdb::Result<surrealdb::IndexedResults>) -> String {
    match q {
        Err(e) => e.to_string(),
        Ok(mut res) => res.take_errors().into_values().map(|e| e.to_string()).collect::<Vec<_>>().join("; "),
    }
}

#[tokio::test]
async fn every_query_executes() {
    let app = common::TestApp::new().await;
    let org = app.db().await;
    // every tenant statement runs against a freshly provisioned (STRICT) org database
    let broken = failures(
        eunomia_backend::store::all(),
        |s| (s.name, s.sql),
        |stmt, names| {
            let org = org.clone();
            async move {
                let mut q = stmt.on(&org);
                for n in names {
                    q = q.bind((n, surrealdb::types::Value::None));
                }
                outcome(q.await).await
            }
        },
    )
    .await;
    assert!(broken.is_empty(), "tenant statements that do not parse:\n{}", broken.join("\n"));

    // and every control statement against the control database
    let control = app.control().clone();
    let broken = failures(
        eunomia_backend::store::all_control(),
        |s| (s.name, s.sql),
        |stmt, names| {
            let control = control.clone();
            async move {
                let mut q = stmt.on(&control);
                for n in names {
                    q = q.bind((n, surrealdb::types::Value::None));
                }
                outcome(q.await).await
            }
        },
    )
    .await;
    assert!(broken.is_empty(), "control statements that do not parse:\n{}", broken.join("\n"));
    assert_eq!(eunomia_backend::pool::NO_ORG_CONTEXT.load(std::sync::atomic::Ordering::Relaxed), 0);
}

/// A statement only names tables of its own database, and the two table lists cover exactly what the
/// migrations define (so a new table cannot slip past the classification).
#[tokio::test]
async fn statements_stay_in_their_database() {
    use eunomia_backend::store::{crossing_table, CONTROL_TABLES, TENANT_TABLES};
    for s in eunomia_backend::store::all() {
        assert_eq!(crossing_table(s.sql, true), None, "{} is a tenant statement but names a control table", s.name);
    }
    for s in eunomia_backend::store::all_control() {
        assert_eq!(crossing_table(s.sql, false), None, "{} is a control statement but names a tenant table", s.name);
    }
    assert!(crossing_table("SELECT * FROM session WHERE owner = $o", true).is_some());
    assert!(crossing_table("SELECT user.email FROM vault_member", true).is_none(), "a field named like a table is not a table");
    assert!(crossing_table("RELATE $a->relates_to->$b", false).is_some());
    // forms the old two-word scanner missed
    assert_eq!(crossing_table("SELECT * FROM memory, session WHERE owner = $o", true).as_deref(), Some("session"), "every item of a FROM list");
    assert_eq!(crossing_table("DEFINE INDEX i ON user FIELDS email", true).as_deref(), Some("user"), "DEFINE ... ON");
    assert_eq!(crossing_table("RETURN user:abc.email", true).as_deref(), Some("user"), "a record literal");
    assert_eq!(crossing_table("SELECT <-membership<-user FROM $o", true).as_deref(), Some("membership"), "an inbound edge");
    assert_eq!(crossing_table("SELECT * FROM type::thing('job', $k)", true).as_deref(), Some("job"), "type::thing's table");
    assert_eq!(crossing_table("SELECT * FROM memory WHERE id IN (SELECT VALUE m FROM ONLY session:x)", true).as_deref(), Some("session"), "inside a subquery");
    // still not tables
    assert!(crossing_table("CREATE memory SET owner = $o, vault = $v, text = string::lowercase($t)", true).is_none(), "fields and functions");
    assert!(crossing_table("UPDATE $id SET meta = { user: $u, job: 1 }", true).is_none(), "object keys");

    let app = common::TestApp::new().await;
    let tables = |v: Value| -> BTreeSet<String> {
        v["tables"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).filter(|n| !n.starts_with("_migration")).collect()
    };
    let info = |db: &surrealdb::Surreal<surrealdb::engine::any::Any>| {
        let db = db.clone();
        async move { db.query("INFO FOR DB STRUCTURE").await.unwrap().take::<Option<Value>>(0).unwrap().unwrap() }
    };
    let org = tables(info(app.db().await.test_raw()).await);
    let control = tables(info(app.control().test_raw()).await);
    assert_eq!(org, TENANT_TABLES.iter().map(|t| t.to_string()).collect(), "tenant tables in the org schema vs store::TENANT_TABLES");
    assert_eq!(control, CONTROL_TABLES.iter().map(|t| t.to_string()).collect(), "control tables vs store::CONTROL_TABLES");
}

/// The words `every_query_executes` greps for must match what this SurrealDB version really says.
#[tokio::test]
async fn gate_vocabulary_matches_the_server() {
    let state = common::bare_state().await;
    let say = |sql: &'static str| {
        let db = state.control.test_raw().clone();
        async move {
            match db.query(sql).await {
                Err(e) => e.to_string().to_lowercase(),
                Ok(mut r) => r.take_errors().into_values().map(|e| e.to_string()).collect::<Vec<_>>().join("; ").to_lowercase(),
            }
        }
    };
    assert!(say("SELECT * FROM no_such_table").await.contains("does not exist"));
    assert!(say("SELECT FROM WHERE").await.contains("parse error"));
    assert!(say("RETURN no::such_fn()").await.contains("invalid function"));
    let extra = say("CREATE user SET email = 'a', password_hash = 'x', bogus = 1").await;
    assert!(extra.contains("no such field"), "{extra}");
}

#[test]
fn store_names_are_unique() {
    let mut seen = BTreeSet::new();
    let names = eunomia_backend::store::all().into_iter().map(|s| s.name).chain(eunomia_backend::store::all_control().into_iter().map(|s| s.name));
    for name in names {
        assert!(seen.insert(name), "duplicate statement name {name}");
    }
}

/// Static gates (plan 3.3, "Proving no leaks", item 5): the raw client, root credentials and
/// database switching live in a few modules, so a query can only reach the database through a typed
/// handle.
#[test]
fn raw_database_access_stays_in_its_modules() {
    let use_stmt = Regex::new(r"(?i)\bUSE\s+(NS|DB|DATABASE|NAMESPACE)\b").unwrap();
    // (what, text that gives it away, where it may appear)
    let rules: [(&str, &[&str], &[&str]); 5] = [
        ("a query on the raw client", &[".query(", ".raw(", ".select::<", ".live()"], &["src/store/", "src/pool.rs"]),
        ("a root sign-in", &["auth::Root", "Root {"], &["src/provisioning/", "src/db.rs"]),
        ("signing in", &["signin("], &["src/provisioning/", "src/pool.rs"]),
        ("switching database", &[".use_db(", ".use_ns("], &["src/provisioning/", "src/pool.rs"]),
        ("the raw accessor", &[".raw("], &["src/store/", "src/pool.rs"]),
    ];
    let mut offenders = Vec::new();
    for entry in walk("src") {
        let text = std::fs::read_to_string(&entry).unwrap();
        let code: String = text.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
        for (what, needles, homes) in rules {
            let at_home = homes.iter().any(|h| entry.starts_with(h));
            if !at_home && needles.iter().any(|n| code.contains(n)) {
                offenders.push(format!("{entry}: {what}"));
            }
        }
        if use_stmt.is_match(&code) {
            offenders.push(format!("{entry}: USE inside statement text"));
        }
    }
    assert!(offenders.is_empty(), "go through src/store/ (Stmt::on, store::dynamic) and the pool: {offenders:#?}");
}

fn walk(dir: &str) -> Vec<String> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        let s = p.to_string_lossy().replace('\\', "/");
        if p.is_dir() {
            out.extend(walk(&s));
        } else if s.ends_with(".rs") {
            out.push(s);
        }
    }
    out
}

/// A real optimistic-commit conflict must be recognised by `tx::is_conflict` (the retry loop and
/// the DbConflict error code hang on it); 3.x changed the error from an enum variant to a kind.
#[tokio::test]
async fn real_commit_conflict_is_detected() {
    let state = common::bare_state().await;
    let db = state.control.test_raw();
    db.query("DEFINE TABLE ctr SCHEMALESS; CREATE ctr:one SET n = 0;").await.unwrap().check().unwrap();
    let a = db.clone().begin().await.unwrap();
    let b = db.clone().begin().await.unwrap();
    a.query("UPDATE ctr:one SET n = n + 1").await.unwrap().check().unwrap();
    b.query("UPDATE ctr:one SET n = n + 1").await.unwrap().check().unwrap();
    a.commit().await.unwrap();
    let err = b.commit().await.expect_err("second commit must conflict");
    eprintln!("conflict error: {err:?}");
    assert!(eunomia_backend::tx::is_conflict(&err), "not recognised: {err:?}");
}

/// Semantic recall is scoped to one owner and ranked by cosine similarity, whether the HNSW walk
/// fills the page or comes up short and the exact scan answers.
#[tokio::test]
async fn nearest_ids_is_owner_scoped_and_falls_back_to_an_exact_scan() {
    use eunomia_backend::cache::search::nearest_ids;
    let app = common::TestApp::new().await;
    let orgdb = app.db().await;
    let db = orgdb.test_raw();
    let unit = |i: usize| {
        let mut v = vec![0.0f32; 1536];
        v[i] = 1.0;
        v
    };
    // Owner b has many records close to the query; owner a has two, one of them deleted.
    for (owner, key, axis, deleted) in [("b", "b1", 0, false), ("b", "b2", 0, false), ("b", "b3", 0, false), ("a", "a1", 1, false), ("a", "a2", 0, true), ("a", "a3", 2, false)] {
        db.query("CREATE $k SET owner = $o, source = 's', type = 't', external_id = $x, content_hash = 'h', ingested_at = time::now(), updated_at = time::now(), deleted = $d, embedding = $e")
            .bind(("k", surrealdb::types::RecordId::new("cache_record", format!("{owner}:{key}"))))
            .bind(("o", surrealdb::types::RecordId::new("user", owner)))
            .bind(("x", key))
            .bind(("d", deleted))
            .bind(("e", unit(axis)))
            .await
            .unwrap()
            .check()
            .unwrap();
    }
    let a = eunomia_backend::rid::parse("user:a").unwrap();
    // Asking for more than owner a has: HNSW is short, the exact scan returns exactly a's live rows, best first.
    let scope = eunomia_backend::cache::search::RecordScope::own(&a);
    let ids = nearest_ids(&orgdb, &scope, unit(1), &Default::default(), 10).await.unwrap();
    let keys: Vec<String> = ids.iter().map(|r| eunomia_backend::rid::key_string(&r.key).unwrap()).collect();
    assert_eq!(keys, ["a:a1", "a:a3"]);
}

/// Editing a memory and staling its subject's observation commit together, like WRITE_FACT and
/// DELETE_MEMORY: an edit must never leave an observation summarising the old text as fresh.
#[test]
fn a_memory_edit_and_its_stale_observation_are_one_transaction() {
    use eunomia_backend::store::entities::{UPDATE_MEMORY_TEXT, UPDATE_MEMORY_TEXT_TYPE, UPDATE_MEMORY_TYPE, WRITE_FACT};
    for s in [&UPDATE_MEMORY_TEXT, &UPDATE_MEMORY_TYPE, &UPDATE_MEMORY_TEXT_TYPE, &WRITE_FACT] {
        let sql = s.sql.trim();
        assert!(sql.starts_with("BEGIN TRANSACTION;") && sql.ends_with("COMMIT TRANSACTION;"), "{} is not one transaction", s.name);
        assert!(sql.contains(r#"status = "stale""#), "{} does not stale the observation", s.name);
    }
}

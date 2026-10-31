//! Versioned schema runner: fresh apply, upgrade from a 2.x export, dedupe, idempotence, tamper.
mod common;

use common::test_settings;
use eunomia_backend::migrate;
use serde_json::Value;

type Db = surrealdb::Surreal<surrealdb::engine::any::Any>;

/// A root session on an empty, non-strict scratch database: the migration runner's own view of a
/// database (tenancy aside), the same way a legacy single-database install is seen.
async fn fresh() -> Db {
    let state = common::bare_state().await;
    state.provisioner.as_ref().expect("provisioning feature").scratch("scratch").await.unwrap()
}

async fn info(db: &Db) -> Value {
    let mut r = db.query("INFO FOR DB STRUCTURE").await.expect("info");
    let mut v: Value = r.take::<Option<Value>>(0).expect("take").expect("row");
    // The ledger is bookkeeping, not schema: legacy-then-migrate has it, a raw legacy DB does not.
    if let Some(a) = v.get_mut("tables").and_then(Value::as_array_mut) {
        a.retain(|t| !t["name"].as_str().is_some_and(|n| n.starts_with("_migration")));
        for t in a.iter_mut() {
            // Table ids follow creation order, which differs between a fresh and an imported database.
            t.as_object_mut().unwrap().remove("id");
            // Indexes live on the table, not in the database overview.
            let name = t["name"].as_str().unwrap().to_string();
            let mut r = db.query(format!("INFO FOR TABLE `{name}` STRUCTURE")).await.expect("table info");
            let info: Value = r.take::<Option<Value>>(0).expect("take").expect("row");
            t["indexes"] = info["indexes"].clone();
        }
    }
    v
}

/// Row count for a `SELECT` tail, e.g. `count(&db, "person WHERE vault = vault:v")`.
async fn count(db: &Db, from: &str) -> usize {
    let v: Option<usize> =
        db.query(format!("RETURN array::len((SELECT VALUE id FROM {from}))")).await.unwrap().take(0).unwrap();
    v.unwrap()
}

#[tokio::test]
async fn fresh_schema_matches_snapshot() {
    let db = fresh().await;
    migrate::migrate(&db, &test_settings()).await.unwrap();
    insta::assert_json_snapshot!(info(&db).await);
}

#[tokio::test]
async fn control_schema_matches_snapshot_and_reapplies_cleanly() {
    let db = fresh().await;
    migrate::migrate_control(&db).await.unwrap();
    let first = info(&db).await;
    migrate::migrate_control(&db).await.unwrap();
    assert_eq!(first, info(&db).await, "re-running the control migrations changes nothing");
    assert_eq!(count(&db, "_migration").await, migrate::CONTROL_MIGRATIONS.len());
    insta::assert_json_snapshot!(first);
}

#[tokio::test]
async fn duplicates_are_merged_into_oldest() {
    let db = fresh().await;
    migrate::apply_up_to(&db, 1).await.unwrap();
    db.query(
        "CREATE user:u SET email = 'a@b.c', password_hash = 'x';
         CREATE vault:v SET name = 'v';
         CREATE vault:w SET name = 'w';
         CREATE organisation:o SET owner = user:u, vault = vault:v, name = 'Acme';
         CREATE person:a SET owner = user:u, vault = vault:v, name = 'Ann', created_at = d'2024-01-01T00:00:00Z';
         CREATE person:b SET owner = user:u, vault = vault:v, name = 'Ann', aliases = ['A', 'Ann'], created_at = d'2024-02-01T00:00:00Z';
         CREATE person:c SET owner = user:u, vault = vault:v, name = 'Ann', created_at = d'2024-03-01T00:00:00Z';
         CREATE person:other SET owner = user:u, vault = vault:w, name = 'Ann';
         CREATE memory:m1 SET owner = user:u, vault = vault:v, subject = person:a, text = 'one';
         CREATE memory:m2 SET owner = user:u, vault = vault:v, subject = person:b, text = 'two';
         CREATE memory:m3 SET owner = user:u, vault = vault:v, subject = person:c, text = 'three';
         RELATE person:b->relates_to->organisation:o SET label = 'works_at';
         RELATE person:c->relates_to->person:a SET label = 'knows';",
    )
    .await
    .unwrap()
    .check()
    .unwrap();

    migrate::migrate(&db, &test_settings()).await.unwrap();

    assert_eq!(count(&db, "person WHERE vault = vault:v").await, 1);
    assert_eq!(count(&db, "person:a").await, 1, "oldest survives");
    assert_eq!(count(&db, "person:other").await, 1, "other vault untouched");
    assert_eq!(count(&db, "memory WHERE subject = person:a").await, 3);
    assert_eq!(count(&db, "relates_to WHERE in = person:a AND out = organisation:o").await, 1);
    assert_eq!(count(&db, "relates_to").await, 1, "self-loop dropped");
    let aliases: Option<Vec<String>> = db.query("SELECT VALUE aliases FROM person:a").await.unwrap().take(0).unwrap();
    assert_eq!(aliases.unwrap(), vec!["A".to_string()]);
    // The unique index now exists and bites.
    assert!(db
        .query("CREATE person SET owner = user:u, vault = vault:v, name = 'Ann'")
        .await
        .unwrap()
        .check()
        .is_err());
}

#[tokio::test]
async fn rerun_is_noop() {
    let db = fresh().await;
    let s = test_settings();
    migrate::migrate(&db, &s).await.unwrap();
    let before = info(&db).await;
    migrate::migrate(&db, &s).await.unwrap();
    assert_eq!(before, info(&db).await);
    assert_eq!(count(&db, "_migration").await, migrate::MIGRATIONS.len());
}

#[tokio::test]
async fn tampered_checksum_is_a_hard_error() {
    let db = fresh().await;
    let s = test_settings();
    migrate::migrate(&db, &s).await.unwrap();
    db.query("UPDATE _migration SET checksum = 'tampered' WHERE version = 1").await.unwrap().check().unwrap();
    let err = migrate::migrate(&db, &s).await.unwrap_err().to_string();
    assert!(err.contains("edited after it was applied"), "{err}");
}

#[tokio::test]
async fn existing_tokens_and_sessions_survive_0003() {
    let db = fresh().await;
    migrate::apply_up_to(&db, 2).await.unwrap();
    db.query(
        "CREATE user:u SET email = 'a@b.c', password_hash = 'x';
         CREATE api_token:t SET owner = user:u, token_hash = 'h';
         CREATE session:s SET owner = user:u, sid = 'sid';",
    )
    .await
    .unwrap()
    .check()
    .unwrap();
    // the legacy database is only ever taken this far: 0009 drops the tables the move reads
    migrate::apply_up_to(&db, migrate::LEGACY_TENANT_VERSION).await.unwrap();
    let scopes: Option<Vec<String>> = db.query("SELECT VALUE scopes FROM api_token:t").await.unwrap().take(0).unwrap();
    assert_eq!(scopes.unwrap(), ["memory:read", "memory:write", "vaults:admin", "connectors"]);
    assert_eq!(count(&db, "api_token WHERE expires_at = NONE AND vault = NONE").await, 1, "non-expiring, unrestricted");
    assert_eq!(count(&db, "session WHERE expires_at > time::now()").await, 1, "existing sessions get an expiry");
}

// ---- SurrealDB 2.x to 3.x upgrade -------------------------------------------------------------
//
// An upgraded install is `surreal v2 export --v3` of the 2.x database, imported into a fresh 3.x
// one. The fixture below has that shape: the schema 2.x had after migrations 1 to 7, the
// `_migration` ledger rows 2.x recorded (0001 and 0005 carry their old 2.x checksums), the
// converter's index output, and seed rows. Regenerate it with
//   cargo test --test migrations -- --ignored regenerate_v2_fixture
// NOTE: it is produced by the embedded 3.x engine's own export from a database built to look like
// the converted state; the index lines are our reading of what the converter emits, not a capture
// of a real 2.7 export. See docs/surrealdb-3-upgrade.md for the real export commands.

const V2_FIXTURE: &str = include_str!("fixtures/v2_export_converted.surql");
const LEGACY_SUMS: [(u32, &str); 2] = [
    (1, "31199597d8ffdb899e26dd741ec3886f6af03467f65710f4c7a1db2bfc3ce5a5"),
    (5, "7227cfd0e1a0f672f6938c4bada0b9d7c720e42ff06ff7d8d93bfd11f231acd2"),
];

fn unit_vector(i: usize) -> Vec<f32> {
    let mut v = vec![0.0f32; 1536];
    v[i] = 1.0;
    v
}

#[tokio::test]
#[ignore = "writes tests/fixtures/v2_export_converted.surql"]
async fn regenerate_v2_fixture() {
    let db = fresh().await;
    // Everything 2.x had applied: all migrations except the 3.x index one (the highest version).
    let last_2x = migrate::MIGRATIONS.iter().map(|m| m.0).filter(|v| *v < 8).max().unwrap();
    migrate::apply_up_to(&db, last_2x).await.unwrap();
    for (v, sum) in LEGACY_SUMS {
        db.query("UPDATE _migration SET checksum = $c WHERE version = $v").bind(("c", sum)).bind(("v", v)).await.unwrap().check().unwrap();
    }
    db.query(
        "DEFINE INDEX cache_record_embedding_idx ON cache_record FIELDS embedding HNSW DIMENSION 1536 DIST COSINE TYPE F32;
         DEFINE INDEX cache_record_fts_idx ON cache_record FIELDS title FULLTEXT ANALYZER cache_text_analyzer BM25 HIGHLIGHTS;
         DEFINE INDEX memory_text_fts_idx ON memory FIELDS text FULLTEXT ANALYZER cache_text_analyzer BM25;
         CREATE user:u SET email = 'a@b.c', password_hash = 'x';
         CREATE vault:v SET name = 'Personal', kind = 'personal';
         CREATE vault_member:vm SET vault = vault:v, user = user:u, role = 'owner';
         CREATE user:u2 SET email = 'Zed@Example.com', password_hash = 'x', created_at = d'2026-10-08T13:41:00Z';
         CREATE user:u3 SET email = 'zed@example.COM', password_hash = 'x', created_at = d'2026-10-08T13:42:00Z';
         CREATE vault_member:vm2 SET vault = vault:v, user = user:u2, role = 'member';
         CREATE vault_member:vm3 SET vault = vault:v, user = user:u3, role = 'member', status = 'pending';
         CREATE person:ann SET owner = user:u, vault = vault:v, name = 'Ann';
         CREATE organisation:acme SET owner = user:u, vault = vault:v, name = 'Acme';
         RELATE person:ann->relates_to->organisation:acme SET label = 'works_at';
         CREATE memory:m1 SET owner = user:u, vault = vault:v, subject = person:ann, text = 'Ann likes green tea';
         CREATE memory:m2 SET owner = user:u, vault = vault:v, subject = person:ann, text = 'Ann works remotely';",
    )
    .bind(("e1", unit_vector(0)))
    .await
    .unwrap()
    .check()
    .unwrap();
    for (i, (id, title, body)) in [("r1", "Woolworths groceries", "weekly shop"), ("r2", "Netflix subscription", "monthly streaming")].iter().enumerate() {
        db.query("CREATE $rid SET owner = user:u, source = 'demo', type = 'note', external_id = $id, title = $t, body_text = $b, content_hash = 'h', ingested_at = time::now(), updated_at = time::now(), embedding = $e")
            .bind(("rid", surrealdb::types::RecordId::new("cache_record", *id)))
            .bind(("id", *id))
            .bind(("t", *title))
            .bind(("b", *body))
            .bind(("e", unit_vector(i)))
            .await
            .unwrap()
            .check()
            .unwrap();
    }
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/v2_export_converted.surql");
    db.export(path).await.unwrap();
}

#[tokio::test]
async fn v2_export_upgrades_to_the_fresh_schema() {
    let settings = test_settings();
    let db = fresh().await;
    db.query(V2_FIXTURE).await.unwrap().check().unwrap(); // what `surreal import` does
    assert_eq!(count(&db, "_migration").await, 7, "arrives carrying the 2.x ledger");

    // Old checksums for 0001 and 0005 are accepted; the 3.x index migration then applies.
    migrate::migrate(&db, &settings).await.unwrap();
    assert_eq!(count(&db, "_migration").await, migrate::MIGRATIONS.len());
    assert_eq!(count(&db, "memory").await, 2);
    assert_eq!(count(&db, "cache_record").await, 2);
    assert_eq!(count(&db, "relates_to").await, 1);

    // Same schema, indexes included, as a database that never ran 2.x.
    let new = fresh().await;
    migrate::migrate(&new, &settings).await.unwrap();
    assert_eq!(info(&db).await, info(&new).await);

    // The re-defined indexes answer queries over the imported rows (the vector index is exercised
    // end to end by tenancy.rs, which moves this same fixture into an org database).
    let hits: Vec<surrealdb::types::RecordId> =
        db.query("SELECT VALUE id FROM cache_record WHERE title @1@ 'netflix'").await.unwrap().take(0).unwrap();
    assert_eq!(hits.len(), 1);
    let mems: Vec<surrealdb::types::RecordId> = db.query("SELECT VALUE id FROM memory WHERE text @1@ 'tea'").await.unwrap().take(0).unwrap();
    assert_eq!(mems.len(), 1);
}

/// Two replicas booting together both run the control and tenant migrations on the same empty
/// database. The loser used to hit the unique `_migration.version` index inside the migration
/// transaction and the boot panicked; now it sees the other process applied the version and moves on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_migrations_on_one_database_both_succeed() {
    let state = common::bare_state().await;
    let p = state.provisioner.as_ref().expect("provisioning feature");
    for round in 0..6 {
        let ctl = p.scratch(&format!("race_ctl_{round}")).await.unwrap();
        let (a, b) = (ctl.clone(), ctl.clone());
        let (ra, rb) = tokio::join!(tokio::spawn(async move { migrate::migrate_control(&a).await }), tokio::spawn(async move { migrate::migrate_control(&b).await }));
        ra.unwrap().unwrap_or_else(|e| panic!("round {round}, first control migrator: {e}"));
        rb.unwrap().unwrap_or_else(|e| panic!("round {round}, second control migrator: {e}"));
        assert_eq!(count(&ctl, "_migration").await, migrate::CONTROL_MIGRATIONS.len());

        let ten = p.scratch(&format!("race_ten_{round}")).await.unwrap();
        let (a, b, settings) = (ten.clone(), ten.clone(), test_settings());
        let s2 = settings.clone();
        let (ra, rb) = tokio::join!(tokio::spawn(async move { migrate::migrate(&a, &settings).await }), tokio::spawn(async move { migrate::migrate(&b, &s2).await }));
        ra.unwrap().unwrap_or_else(|e| panic!("round {round}, first tenant migrator: {e}"));
        rb.unwrap().unwrap_or_else(|e| panic!("round {round}, second tenant migrator: {e}"));
        assert_eq!(count(&ten, "_migration").await, migrate::MIGRATIONS.len());
    }
}

/// While one process holds a database's migration lock another waits (DDL from two boots is not
/// isolated: a racing one saw "The table 'x' does not exist"); a lock left by a dead process goes stale
/// and is taken over; the lock is released when the run ends.
#[tokio::test]
async fn migrations_wait_for_the_lock_and_take_over_a_stale_one() {
    let db = fresh().await;
    db.query("DEFINE TABLE _migration_lock SCHEMALESS; CREATE _migration_lock:run SET at = time::now();").await.unwrap().check().unwrap();
    let held = tokio::time::timeout(std::time::Duration::from_millis(1500), migrate::migrate_control(&db)).await;
    assert!(held.is_err(), "migrated while another process held the lock");

    db.query("UPDATE _migration_lock:run SET at = time::now() - 10m").await.unwrap().check().unwrap();
    migrate::migrate_control(&db).await.unwrap();
    assert_eq!(count(&db, "_migration").await, migrate::CONTROL_MIGRATIONS.len());
    assert_eq!(count(&db, "_migration_lock").await, 0, "lock released");
}

/// A migration that runs four times longer than the staleness threshold keeps its lock (its heartbeat
/// moves `at` forward): the second process waits for it to finish instead of running DDL beside it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_live_holder_keeps_the_migration_lock_past_the_stale_threshold() {
    use std::sync::atomic::{AtomicBool, Ordering};
    const STALE: std::time::Duration = std::time::Duration::from_millis(600);
    let db = fresh().await;
    let done = std::sync::Arc::new(AtomicBool::new(false));
    let holder = {
        let (db, done) = (db.clone(), done.clone());
        tokio::spawn(async move {
            migrate::locked_for_tests(&db, STALE, async {
                tokio::time::sleep(STALE * 4).await;
                done.store(true, Ordering::SeqCst);
                Ok(())
            })
            .await
        })
    };
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let saw_done = migrate::locked_for_tests(&db, STALE, async { Ok(done.load(Ordering::SeqCst)) }).await.unwrap();
    holder.await.unwrap().unwrap();
    assert!(saw_done, "the lock was taken over while its holder was still migrating");
    assert_eq!(count(&db, "_migration_lock").await, 0, "both holders released the lock");
}

/// A holder that dies mid-migration (no release, no more heartbeats) leaves a lock that is taken over
/// once it is stale, and not before.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dead_holders_migration_lock_is_taken_after_the_stale_threshold() {
    use std::sync::atomic::{AtomicBool, Ordering};
    const STALE: std::time::Duration = std::time::Duration::from_millis(600);
    let db = fresh().await;
    let started = std::sync::Arc::new(AtomicBool::new(false));
    let crashed = {
        let (db, started) = (db.clone(), started.clone());
        tokio::spawn(async move {
            migrate::locked_for_tests(&db, STALE, async {
                started.store(true, Ordering::SeqCst);
                std::future::pending::<surrealdb::Result<()>>().await
            })
            .await
        })
    };
    while !started.load(Ordering::SeqCst) {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    crashed.abort(); // the process is gone: its heartbeat and release never run again
    let died = std::time::Instant::now();
    migrate::locked_for_tests(&db, STALE, async { Ok(()) }).await.unwrap();
    let waited = died.elapsed();
    assert!(waited >= STALE - std::time::Duration::from_millis(100), "took a live-looking lock after {waited:?}");
    assert!(waited < STALE * 5, "a dead holder's lock was not taken over in time ({waited:?})");
}

/// 0010 backfills `name_key` / `alias_keys` on rows that predate them and, before the case-insensitive
/// UNIQUE index builds, folds entities that differ only by case into the oldest: memories move over,
/// two observations become one stale one, edges follow, and a lone survivor keeps its own data.
#[tokio::test]
async fn case_variants_are_merged_and_existing_rows_get_their_keys_in_0010() {
    let db = fresh().await;
    migrate::apply_up_to(&db, 9).await.unwrap();
    db.query(
        "CREATE user:u SET email = 'a@b.c', password_hash = 'x';
         CREATE vault:v SET name = 'v';
         CREATE vault:w SET name = 'w';
         CREATE organisation:o SET owner = user:u, vault = vault:v, name = 'Acme';
         CREATE person:a SET owner = user:u, vault = vault:v, name = 'Ada', aliases = ['The Countess'], created_at = d'2024-01-01T00:00:00Z';
         CREATE person:b SET owner = user:u, vault = vault:v, name = 'ada', aliases = ['AL'], created_at = d'2024-02-01T00:00:00Z';
         CREATE person:solo SET owner = user:u, vault = vault:v, name = 'Grace Hopper', aliases = ['Amazing Grace'];
         CREATE person:other SET owner = user:u, vault = vault:w, name = 'ADA';
         CREATE memory:f1 SET owner = user:u, vault = vault:v, subject = person:a, text = 'fact one';
         CREATE memory:f2 SET owner = user:u, vault = vault:v, subject = person:b, text = 'fact two';
         CREATE memory:o1 SET owner = user:u, vault = vault:v, subject = person:a, type = 'observation', text = 'belief one', status = 'fresh', source_memories = [memory:f1], created_at = d'2024-01-02T00:00:00Z';
         CREATE memory:o2 SET owner = user:u, vault = vault:v, subject = person:b, type = 'observation', text = 'belief two', status = 'fresh', source_memories = [memory:f2], created_at = d'2024-02-02T00:00:00Z';
         RELATE person:b->relates_to->organisation:o SET label = 'works_at';",
    )
    .await
    .unwrap()
    .check()
    .unwrap();

    migrate::migrate(&db, &test_settings()).await.unwrap();

    assert_eq!(count(&db, "person WHERE vault = vault:v").await, 2, "Ada/ada folded; Grace stays");
    assert_eq!(count(&db, "person:a").await, 1, "oldest survives");
    assert_eq!(count(&db, "person:other").await, 1, "same name in another vault is untouched");
    assert_eq!(count(&db, "memory WHERE subject = person:a AND type != 'observation'").await, 2);
    assert_eq!(count(&db, "memory WHERE subject = person:a AND type = 'observation'").await, 1, "one observation");
    let obs: Option<Value> = db
        .query("SELECT text, status, source_memories FROM memory WHERE subject = person:a AND type = 'observation'")
        .await
        .unwrap()
        .take(0)
        .unwrap();
    let obs = obs.unwrap();
    assert_eq!(obs["status"], "stale");
    assert!(obs["text"].as_str().unwrap().contains("belief one") && obs["text"].as_str().unwrap().contains("belief two"));
    assert_eq!(obs["source_memories"].as_array().unwrap().len(), 2);
    assert_eq!(count(&db, "relates_to WHERE in = person:a AND out = organisation:o").await, 1, "edge followed");
    let keys: Option<Value> = db.query("SELECT name_key, alias_keys FROM person:solo").await.unwrap().take(0).unwrap();
    let keys = keys.unwrap();
    assert_eq!(keys["name_key"], "grace hopper");
    assert_eq!(keys["alias_keys"], serde_json::json!(["amazing grace"]));
    let winner: Option<Value> = db.query("SELECT aliases, alias_keys FROM person:a").await.unwrap().take(0).unwrap();
    let winner = winner.unwrap();
    assert!(winner["aliases"].as_array().unwrap().iter().any(|a| a == "AL"), "the loser's aliases survive: {winner}");
    assert!(winner["alias_keys"].as_array().unwrap().iter().any(|a| a == "al"));
    // the case-insensitive unique index exists and bites; a computed key follows later renames
    assert!(db.query("CREATE person SET owner = user:u, vault = vault:v, name = 'aDa'").await.unwrap().check().is_err());
    db.query("UPDATE person:solo SET name = 'Rear Admiral'").await.unwrap().check().unwrap();
    let key: Option<String> = db.query("SELECT VALUE name_key FROM ONLY person:solo").await.unwrap().take(0).unwrap();
    assert_eq!(key.unwrap(), "rear admiral");
}

/// 0011 restarts Pocket sync cursors with only allow-listed functions: it must run (TEST_HARDENED=1
/// too) on a database that already has a heypocket cursor, which is when its WHERE clause evaluates.
#[tokio::test]
async fn pocket_cursor_reset_in_0011_runs_with_cursors_present() {
    let db = fresh().await;
    migrate::apply_up_to(&db, 10).await.unwrap();
    db.query(
        "CREATE user:u SET email = 'a@b.c', password_hash = 'x';
         CREATE sync_status:`u:heypocket` SET owner = user:u, cursor = '2026-09-01T00:00:00Z';
         CREATE sync_status:`u:github` SET owner = user:u, cursor = 'keep';",
    )
    .await
    .unwrap()
    .check()
    .unwrap();
    migrate::apply_up_to(&db, 11).await.unwrap();
    let cursors: Vec<String> = db.query("SELECT VALUE cursor FROM sync_status ORDER BY id").await.unwrap().take(0).unwrap();
    assert_eq!(cursors, ["keep", ""], "github kept, heypocket restarted");
}

/// 0016 widens `memory.subject` and `relates_to` to the custom-kind table on a database that already
/// holds memories and relations: they all survive, the edge stays unique, and a custom-kind entity
/// takes memories and relations.
#[tokio::test]
async fn custom_kinds_in_0016_keep_existing_memories_and_relations() {
    let db = fresh().await;
    migrate::apply_up_to(&db, 15).await.unwrap();
    db.query(
        "CREATE person:ada SET owner = user:u, vault = vault:v, name = 'Ada';
         CREATE organisation:acme SET owner = user:u, vault = vault:v, name = 'Acme';
         CREATE memory:m SET owner = user:u, vault = vault:v, subject = person:ada, text = 'Ada likes tea';
         RELATE person:ada->relates_to->organisation:acme SET label = 'works_at';",
    )
    .await
    .unwrap()
    .check()
    .unwrap();
    migrate::apply_up_to(&db, 16).await.unwrap();
    assert_eq!(count(&db, "memory WHERE subject = person:ada").await, 1);
    assert_eq!(count(&db, "relates_to WHERE in = person:ada AND label = 'works_at'").await, 1);
    let dup = db.query("RELATE person:ada->relates_to->organisation:acme SET label = 'works_at'").await.unwrap().check();
    assert!(dup.is_err(), "relates_to_unique survives the table redefinition");
    db.query(
        "CREATE entity:orders SET owner = user:u, vault = vault:v, kind = 'db', name = 'Orders';
         CREATE memory:n SET owner = user:u, vault = vault:v, subject = entity:orders, text = 'runs Postgres';
         RELATE person:ada->relates_to->entity:orders SET label = 'owns';",
    )
    .await
    .unwrap()
    .check()
    .unwrap();
    // a name is unique per kind and vault, case-insensitively; the same name under another kind is fine
    assert!(db.query("CREATE entity SET owner = user:u, vault = vault:v, kind = 'db', name = 'ORDERS'").await.unwrap().check().is_err());
    db.query("CREATE entity SET owner = user:u, vault = vault:v, kind = 'table', name = 'Orders'").await.unwrap().check().unwrap();
}

/// 0017 (multi-connect) over an install with one connector per kind: each existing connection becomes
/// its kind's first one (`instance = ""`) and keeps its row, credentials and cursor; another first
/// connection of the same kind is still refused, and an added one is allowed.
#[tokio::test]
async fn existing_connectors_become_first_connections_in_0017() {
    let db = fresh().await;
    migrate::apply_up_to(&db, 15).await.unwrap();
    db.query(
        "CREATE user:u SET email = 'a@b.c', password_hash = 'x';
         CREATE connector:old SET owner = user:u, kind = 'gmail', enabled = true, credentials_encrypted = 'enc';
         CREATE sync_status:`u:gmail` SET owner = user:u, cursor = 'keep';",
    )
    .await
    .unwrap()
    .check()
    .unwrap();
    migrate::migrate(&db, &test_settings()).await.unwrap();
    let row: Option<Value> =
        db.query("SELECT instance, label, enabled, credentials_encrypted FROM ONLY connector:old").await.unwrap().take(0).unwrap();
    assert_eq!(row.unwrap(), serde_json::json!({"instance": "", "label": "", "enabled": true, "credentials_encrypted": "enc"}));
    let found: Vec<Value> =
        db.query("SELECT id FROM connector WHERE owner = user:u AND kind = 'gmail' AND instance = ''").await.unwrap().take(0).unwrap();
    assert_eq!(found.len(), 1, "the index finds the migrated row");
    assert!(db.query("CREATE connector SET owner = user:u, kind = 'gmail'").await.unwrap().check().is_err(), "one first connection per kind");
    db.query("CREATE connector SET owner = user:u, kind = 'gmail', instance = 'x', label = 'work'").await.unwrap().check().unwrap();
    let cursor: Option<String> = db.query("SELECT VALUE cursor FROM ONLY sync_status:`u:gmail`").await.unwrap().take(0).unwrap();
    assert_eq!(cursor.as_deref(), Some("keep"));
}

/// 0017 over connectors made before 0015 (2.0.2 and older): those rows have no `routing`, which 0015
/// typed `object` without backfilling, so 0017's `UPDATE connector` failed the type check and the org
/// stayed at schema 16 (issue #85). The pre-step gives them the default empty routing first.
#[tokio::test]
async fn connectors_from_before_routing_migrate_through_0017() {
    let db = fresh().await;
    migrate::apply_up_to(&db, 14).await.unwrap();
    db.query(
        "CREATE user:u SET email = 'a@b.c', password_hash = 'x';
         CREATE connector:old SET owner = user:u, kind = 'gmail', enabled = true, credentials_encrypted = 'enc';",
    )
    .await
    .unwrap()
    .check()
    .unwrap();
    migrate::apply_up_to(&db, 16).await.unwrap();
    let routing: Option<Value> = db.query("SELECT VALUE routing FROM ONLY connector:old").await.unwrap().take(0).unwrap();
    assert_eq!(routing, None, "0015 does not backfill: the row still has no routing");
    migrate::migrate(&db, &test_settings()).await.unwrap();
    let row: Option<Value> = db.query("SELECT routing, instance, label FROM ONLY connector:old").await.unwrap().take(0).unwrap();
    assert_eq!(row.unwrap(), serde_json::json!({"routing": {}, "instance": "", "label": ""}));
}

/// The three HNSW indexes 0022 rebuilds, as (table, index).
const VECTOR_INDEXES: [(&str, &str); 3] =
    [("memory", "memory_embedding_idx"), ("skill", "skill_embedding_idx"), ("cache_record", "cache_record_embedding_idx")];

/// A 1536-dimension vector near axis `i`, with a little of axis `i + 1`, so neighbours are ordered.
fn axis(i: usize) -> Vec<f32> {
    let mut v = vec![0.0f32; 1536];
    v[i] = 1.0;
    v[i + 1] = 0.25;
    v
}

/// `table:rNN`, bound rather than built with `type::record` (the hardened server denies that function).
fn rid(table: &str, i: usize) -> surrealdb::types::RecordId {
    surrealdb::types::RecordId::new(table, format!("r{i:02}"))
}

/// Waits (bounded) until every vector index reports its background build `ready`, and returns the
/// last `INFO FOR INDEX` of each, so a test can see a build happened.
async fn vector_indexes_ready(db: &Db) -> Vec<Value> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let mut infos = Vec::new();
        for (table, index) in VECTOR_INDEXES {
            let info: Option<Value> = db.query(format!("INFO FOR INDEX {index} ON {table}")).await.unwrap().take(0).unwrap();
            infos.push(info.unwrap_or(Value::Null));
        }
        // no `building` entry: built inline, nothing running
        if infos.iter().all(|i| i["building"].is_null() || i["building"]["status"] == "ready") {
            return infos;
        }
        assert!(std::time::Instant::now() < deadline, "vector indexes not ready: {infos:?}");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// The `k` nearest ids of `table` to `v` through the HNSW index (`<|K,EF|>`), nearest first.
async fn knn(db: &Db, table: &str, v: Vec<f32>, k: usize) -> Vec<String> {
    let sql = format!("SELECT id, vector::similarity::cosine(embedding, $v) AS sim FROM {table} WHERE embedding <|{k},64|> $v ORDER BY sim DESC");
    let rows: Vec<Value> = db.query(sql).bind(("v", v)).await.unwrap().take(0).unwrap();
    rows.iter().map(|r| r["id"].as_str().map(String::from).unwrap_or_else(|| r["id"].to_string())).collect()
}

/// 0022: an org at schema 21 whose vector tables had rows inserted and then some deleted (on 3.3.1 that
/// could leave dead points in the HNSW graph) is rebuilt; kNN then finds exactly the live rows, nearest
/// first, row counts are unchanged, a second pass applies nothing, and running the statements again is
/// harmless.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rebuild_vector_indexes_in_0022_keeps_rows_and_knn_finds_the_live_ones() {
    let db = fresh().await;
    let s = test_settings();
    migrate::apply_up_to(&db, 21).await.unwrap();
    vector_indexes_ready(&db).await;
    // 40 rows per table on axes 0..40; then every row on an odd axis is deleted
    for i in 0..40usize {
        db.query(
            "CREATE $m SET owner = user:u, vault = vault:v, subject = person:p, text = $t, embedding = $e, embedding_spec = 'stub';
             CREATE $s SET owner = user:u, vault = vault:v, name = $t, description = 'd', body = 'b', embedding = $e, embedding_spec = 'stub';
             CREATE $c SET owner = user:u, source = 'demo', type = 'note', external_id = $t, content_hash = 'h',
                 ingested_at = time::now(), updated_at = time::now(), embedding = $e;",
        )
        .bind(("m", rid("memory", i)))
        .bind(("s", rid("skill", i)))
        .bind(("c", rid("cache_record", i)))
        .bind(("t", format!("row {i}")))
        .bind(("e", axis(i)))
        .await
        .unwrap()
        .check()
        .unwrap();
    }
    for i in (1..40usize).step_by(2) {
        db.query("DELETE $m; DELETE $s; DELETE $c;")
            .bind(("m", rid("memory", i)))
            .bind(("s", rid("skill", i)))
            .bind(("c", rid("cache_record", i)))
            .await
            .unwrap()
            .check()
            .unwrap();
    }
    let before: Vec<usize> = futures::future::join_all(VECTOR_INDEXES.iter().map(|(t, _)| count(&db, t))).await;
    assert_eq!(before, [20, 20, 20]);

    migrate::migrate(&db, &s).await.unwrap();
    let ledger: Vec<String> = db.query("SELECT VALUE name FROM _migration WHERE version = 22").await.unwrap().take(0).unwrap();
    assert_eq!(ledger, ["rebuild_vector_indexes"]);
    vector_indexes_ready(&db).await;

    let check = |label: &'static str| {
        let db = db.clone();
        async move {
            for (table, _) in VECTOR_INDEXES {
                assert_eq!(count(&db, table).await, 20, "{label}: {table} rows unchanged");
                // a live row is its own nearest neighbour, then the next live row on
                let near = knn(&db, table, axis(10), 3).await;
                assert_eq!(near.first().map(String::as_str), Some(format!("{table}:r10").as_str()), "{label}: {table} {near:?}");
                assert_eq!(near.len(), 3, "{label}: {table} kNN fills K from the live rows: {near:?}");
                // near a deleted row: K live rows, none of them deleted
                let around = knn(&db, table, axis(11), 5).await;
                assert_eq!(around.len(), 5, "{label}: {table} {around:?}");
                for id in &around {
                    let n: usize = id.rsplit('r').next().unwrap().trim_end_matches(|c: char| !c.is_ascii_digit()).parse().unwrap();
                    assert_eq!(n % 2, 0, "{label}: {table} kNN returned a deleted row: {around:?}");
                }
            }
        }
    };
    check("after 0022").await;

    // a second boot applies nothing
    let schema = info(&db).await;
    migrate::migrate(&db, &s).await.unwrap();
    assert_eq!(count(&db, "_migration").await, migrate::MIGRATIONS.len());
    assert_eq!(schema, info(&db).await);

    // running the rebuild statements again (twice) is harmless. Right after a rebuild the retired index
    // is still being reclaimed in the background, so a redefinition can meet a commit conflict; it is
    // retryable, and retried, as the runner does.
    let sql = migrate::MIGRATIONS.iter().find(|m| m.0 == 22).unwrap().2;
    for _ in 0..2 {
        eunomia_backend::tx::with_retry(|| async { db.query(sql).await?.check().map(|_| ()) }).await.unwrap();
        vector_indexes_ready(&db).await;
    }
    check("after running 0022 twice more").await;
    assert_eq!(schema, info(&db).await, "the definitions are unchanged");
}

/// A fresh org: 0022 runs right after 0008, 0019 and 0021 define the indexes (their builds may still be
/// running). The indexes come up and answer kNN over rows written afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fresh_org_runs_0022_right_after_the_index_definitions() {
    let db = fresh().await;
    migrate::migrate(&db, &test_settings()).await.unwrap();
    assert_eq!(count(&db, "_migration").await, migrate::MIGRATIONS.len());
    vector_indexes_ready(&db).await;
    for i in 0..5usize {
        db.query("CREATE $m SET owner = user:u, vault = vault:v, subject = person:p, text = 't', embedding = $e")
            .bind(("m", rid("memory", i)))
            .bind(("e", axis(i)))
            .await
            .unwrap()
            .check()
            .unwrap();
    }
    assert_eq!(knn(&db, "memory", axis(3), 1).await, ["memory:r03"]);
}

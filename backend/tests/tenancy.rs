//! Org tenancy: provisioning, the pool, the credential wall, schema gating, the self-host move.
mod common;

use common::TestApp;
use eunomia_backend::error::ErrorCode;
use eunomia_backend::pool::OrgId;
use eunomia_backend::provisioning::legacy;
use serde_json::{json, Value};
use surrealdb::types::RecordId;

type Db = surrealdb::Surreal<surrealdb::engine::any::Any>;

#[tokio::test]
async fn boots_and_serves_a_tool_call() {
    let app = TestApp::new().await;
    let out = app.tool("vault_list", json!({})).await;
    assert_eq!(out["results"].as_array().unwrap().len(), 1, "{out}");
}

async fn count(db: &Db, from: &str) -> i64 {
    let n: Option<i64> = db.query(format!("RETURN array::len((SELECT VALUE id FROM {from}))")).await.unwrap().take(0).unwrap();
    n.unwrap()
}

async fn scalar<T: surrealdb::types::SurrealValue>(db: &Db, sql: &str) -> T {
    db.query(sql).await.unwrap().take::<Option<T>>(0).unwrap().unwrap()
}

#[tokio::test]
async fn signup_joins_the_install_org_or_gets_a_personal_one() {
    let state = common::bare_state().await;
    let first = common::register(&state, "first@example.com").await;
    let second = common::register(&state, "second@example.com").await;
    let solo = common::register_personal(&state, "solo@example.com").await;
    assert_eq!(first.org, second.org, "later signups join the install's org");
    assert_ne!(first.org, solo.org, "a personal signup gets an org of its own");

    // the first user owns the org; the second is a member
    let role = |u: &eunomia_backend::models_user::User| {
        let (control, id) = (state.control.clone(), u.id.clone());
        async move {
            let mut res = control.test_raw().query("SELECT VALUE role FROM membership WHERE user = $u").bind(("u", id)).await.unwrap();
            res.take::<Vec<String>>(0).unwrap()
        }
    };
    assert_eq!(role(&first).await, ["owner"]);
    assert_eq!(role(&second).await, ["member"]);
    assert_eq!(role(&solo).await, ["owner"]);

    // each org has its own database, and each user's personal vault is in their org's
    let (a, b) = (common::org_db(&state, &first).await, common::org_db(&state, &solo).await);
    assert_ne!(a.org(), b.org());
    assert_eq!(count(a.test_raw(), "vault").await, 2, "two members, two personal vaults");
    assert_eq!(count(b.test_raw(), "vault").await, 1);
}

#[tokio::test]
async fn an_unknown_org_has_no_database_and_is_counted() {
    let state = common::bare_state().await;
    let before = eunomia_backend::pool::NO_ORG_CONTEXT.load(std::sync::atomic::Ordering::Relaxed);
    let err = state.pool.for_org(&OrgId::new()).await.err().expect("no such org");
    assert_eq!(err.code, ErrorCode::TenantNotFound);
    assert!(eunomia_backend::pool::NO_ORG_CONTEXT.load(std::sync::atomic::Ordering::Relaxed) > before);
}

#[tokio::test]
async fn an_org_two_versions_behind_answers_schema_behind_until_migrated() {
    let app = TestApp::new().await;
    let org = app.user.org;
    let latest = eunomia_backend::migrate::LATEST_TENANT as i64;
    let set = |v: i64| {
        let control = app.state.control.clone();
        async move {
            control.test_raw().query("UPDATE $t SET schema_version = $v").bind(("t", org.tenant_record())).bind(("v", v)).await.unwrap().check().unwrap();
        }
    };
    // N-1 is served
    set(latest - 1).await;
    app.state.pool.evict(&org);
    assert!(app.state.pool.for_org(&org).await.is_ok(), "schema N-1 is supported");
    // below N-1 is not, and the API says so
    set(latest - 2).await;
    app.state.pool.evict(&org);
    let ((status, body), _) = common::http(&app.router, "POST", "/api/tools/vault_list", None, Some(&app.token), None).await;
    assert_eq!((status.as_u16(), body["code"].as_str()), (503, Some("tenant.schema_behind")), "{body}");

    // the scheduler queues one migration per behind org, once, and the job brings it current
    eunomia_backend::jobs::leader::reconcile_tenant_migrations(&app.state).await;
    eunomia_backend::jobs::leader::reconcile_tenant_migrations(&app.state).await;
    let jobs: Vec<String> = app.state.control.test_raw().query("SELECT VALUE kind FROM job").await.unwrap().take(0).unwrap();
    assert_eq!(jobs, ["migrate_tenant"]);
    let job = eunomia_backend::jobs::Job {
        id: RecordId::new("job", "x"),
        org: Some(org.key()),
        kind: "migrate_tenant".into(),
        owner: app.user.id.clone(),
        payload: Value::Null,
        idempotency_key: "k".into(),
        attempts: 1,
        max_attempts: 5,
        status: "running".into(),
        locked_until: None,
        traceparent: None,
    };
    let reg = eunomia_backend::jobs::handlers::registry();
    let _ = reg; // the handler is exercised through the worker in tests/jobs.rs; here call the provisioner it uses
    app.state.provisioner.as_ref().unwrap().migrate_org(&app.state.control, &job.org_id().unwrap()).await.unwrap();
    assert!(app.state.pool.for_org(&org).await.is_ok());
    assert_eq!(app.tool("vault_list", json!({})).await["results"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn provisioning_twice_is_harmless_and_keeps_the_password() {
    let app = TestApp::new().await;
    let p = app.state.provisioner.as_ref().unwrap();
    app.tool("memory_write", json!({"subject_name": "Ann", "subject_kind": "person", "text": "keeps"})).await;
    let before: String = scalar(app.state.control.test_raw(), &format!("SELECT VALUE db_pass_enc FROM tenant:{}", app.user.org.key())).await;
    p.provision_org(&app.state.control, app.user.org, "Again").await.unwrap();
    let after: String = scalar(app.state.control.test_raw(), &format!("SELECT VALUE db_pass_enc FROM tenant:{}", app.user.org.key())).await;
    assert_ne!(before, after, "re-encrypted with a fresh nonce");
    // the same password still signs in, and the data is still there
    app.state.pool.evict(&app.user.org);
    let out = app.tool("entities_search", json!({"query": "Ann"})).await;
    assert_eq!(out["results"].as_array().unwrap().len(), 1, "{out}");
}

// ---- the self-host move -------------------------------------------------------------------

const V2_FIXTURE: &str = include_str!("fixtures/v2_export_converted.surql");

fn unit_vector(i: usize) -> Vec<f32> {
    let mut v = vec![0.0f32; 1536];
    v[i] = 1.0;
    v
}

/// A fresh install's state with the 2.x-export fixture imported as the old single database.
async fn state_with_legacy() -> (eunomia_backend::state::AppState, Db) {
    let state = common::bare_state().await;
    let legacy_db = state.provisioner.as_ref().unwrap().scratch(&state.settings.surreal_db).await.unwrap();
    legacy_db.query(V2_FIXTURE).await.unwrap().check().unwrap(); // what `surreal import` does
    (state, legacy_db)
}

async fn run_move(state: &eunomia_backend::state::AppState) {
    legacy::move_if_needed(state.provisioner.as_ref().unwrap(), &state.control, &state.settings).await.unwrap();
}

async fn only_org(state: &eunomia_backend::state::AppState) -> OrgId {
    let orgs: Vec<RecordId> = state.control.test_raw().query("SELECT VALUE id FROM org").await.unwrap().take(0).unwrap();
    assert_eq!(orgs.len(), 1, "{orgs:?}");
    OrgId::parse(&eunomia_backend::rid::key_string(&orgs[0].key).unwrap()).unwrap()
}

#[tokio::test]
async fn the_move_carries_a_2x_export_into_one_org_and_is_a_noop_the_second_time() {
    let (state, old) = state_with_legacy().await;
    // a 1.4 install also has Pocket recordings stored whole (the fixture predates the table)
    // ...and, from 1.3, observations carry `obs_subject`, a field 2.0 no longer has (it must be left out,
    // not make the observation vanish from the move)
    old.query(
        "DEFINE TABLE pocket_recording SCHEMALESS; CREATE pocket_recording:`u:rec1` SET owner = user:u, recording_id = 'rec1', title = 'Standup', transcript = 'Ann: hi';
         DEFINE FIELD obs_subject ON memory VALUE IF type = 'observation' THEN subject ELSE NONE END;
         LET $m = (SELECT * FROM memory LIMIT 1)[0];
         CREATE memory:obs1 SET owner = $m.owner, vault = $m.vault, subject = $m.subject, text = 'a belief', type = 'observation', status = 'stale';",
    )
        .await
        .unwrap()
        .check()
        .unwrap();
    run_move(&state).await;
    let org = only_org(&state).await;

    // accounts moved to control, the oldest user owns the org
    assert_eq!(count(state.control.test_raw(), "user").await, 3);
    assert_eq!(count(state.control.test_raw(), "membership WHERE role = 'owner' AND user = user:u").await, 1, "the oldest user owns");
    assert_eq!(count(state.control.test_raw(), "membership WHERE role = 'owner'").await, 1);
    let status: String = scalar(state.control.test_raw(), &format!("SELECT VALUE status FROM tenant:{}", org.key())).await;
    assert_eq!(status, "ready");

    // org data moved, record ids unchanged, and the indexes answer over it
    let db = state.pool.for_org(&org).await.unwrap();
    for (table, n) in [("memory", 3), ("cache_record", 2), ("person", 1), ("organisation", 1), ("relates_to", 1), ("vault_member", 3), ("pocket_recording", 1)] {
        assert_eq!(count(db.test_raw(), table).await, n, "{table}");
    }
    let owner = eunomia_backend::rid::parse("user:u").unwrap();
    let scope = eunomia_backend::cache::search::RecordScope::own(&owner);
    let key = |r: &RecordId| eunomia_backend::rid::key_string(&r.key).unwrap();
    let ids = eunomia_backend::cache::search::nearest_ids(&db, &scope, unit_vector(1), &Default::default(), 5).await.unwrap();
    assert_eq!(ids.first().map(key).as_deref(), Some("r2"));
    let top1 = eunomia_backend::cache::search::nearest_ids(&db, &scope, unit_vector(1), &Default::default(), 1).await.unwrap();
    assert_eq!(top1.iter().map(key).collect::<Vec<_>>(), ["r2"], "HNSW path, no fallback needed");
    let hits: Vec<RecordId> = db.test_raw().query("SELECT VALUE id FROM cache_record WHERE title @1@ 'netflix'").await.unwrap().take(0).unwrap();
    assert_eq!(hits.len(), 1);

    // the app works on it as the moved user
    let user = eunomia_backend::models_user::load_user(&state.control, owner, "a@b.c".into()).await.unwrap();
    assert_eq!(user.org, org);
    let out = common::sys(eunomia_backend::tools::registry::call(&state, &user, "entities_search", json!({"query": "Ann"}))).await.unwrap();
    assert_eq!(out["results"].as_array().unwrap().len(), 1, "{out}");

    // the old database keeps its rows (it was only brought to schema 8 in place)
    assert_eq!(count(&old, "user").await, 3);
    assert_eq!(count(&old, "memory").await, 3);

    // a second run is a no-op: no second org, nothing changes
    run_move(&state).await;
    assert_eq!(only_org(&state).await, org);
    assert_eq!(count(db.test_raw(), "memory").await, 3);
}

#[tokio::test]
async fn an_interrupted_move_resumes_and_verifies() {
    let (state, _old) = state_with_legacy().await;
    run_move(&state).await;
    let org = only_org(&state).await;
    let db = state.pool.for_org(&org).await.unwrap();

    // pretend the process died mid-copy: the org is still `moving` and some rows are missing
    state.control.test_raw().query("UPDATE $t SET status = 'moving'").bind(("t", org.tenant_record())).await.unwrap().check().unwrap();
    db.test_raw().query("DELETE memory:m2; DELETE cache_record:r1;").await.unwrap().check().unwrap();
    state.pool.evict(&org);
    assert_eq!(state.pool.for_org(&org).await.err().unwrap().code, ErrorCode::TenantNotFound, "a half-moved org is not served");

    run_move(&state).await;
    let db = state.pool.for_org(&org).await.unwrap();
    assert_eq!(count(db.test_raw(), "memory").await, 2);
    assert_eq!(count(db.test_raw(), "cache_record").await, 2);
    assert_eq!(only_org(&state).await, org, "resumed, not restarted");
    assert_eq!(count(state.control.test_raw(), "membership").await, 3, "no duplicate membership");
}

#[tokio::test]
async fn a_fresh_install_has_nothing_to_move() {
    let state = common::bare_state().await;
    run_move(&state).await;
    assert_eq!(count(state.control.test_raw(), "org").await, 0);
}

/// No request path holds root: the provisioner keeps credentials, not a signed-in session. It opens
/// root for one operation (here an org creation and a migration) and drops it.
#[tokio::test]
async fn the_provisioner_holds_no_root_session_between_operations() {
    let app = TestApp::new().await;
    let p = app.state.provisioner.as_ref().unwrap();
    let probe = p.scratch("probe").await.unwrap(); // a real root session, to show the check can say no
    assert!(probe.query("INFO FOR ROOT").await.unwrap().check().is_ok());
    assert!(p.holds_no_root_session().await, "idle provisioner must not be signed in as root");
    p.provision_org(&app.state.control, OrgId::new(), "Another").await.unwrap();
    p.migrate_org(&app.state.control, &app.user.org).await.unwrap();
    assert!(p.holds_no_root_session().await, "still not signed in after provisioning and migrating");
}

/// Two replicas booting together on a pre-tenancy install both run the move; both finish, there is
/// one org, one membership per user and the same counts as a single run.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_concurrent_boots_of_a_legacy_install_make_one_org() {
    let (state, _old) = state_with_legacy().await;
    let (s1, s2) = (state.clone(), state.clone());
    let (a, b) = tokio::join!(tokio::spawn(async move { run_move(&s1).await }), tokio::spawn(async move { run_move(&s2).await }));
    a.unwrap();
    b.unwrap();
    let org = only_org(&state).await;
    let db = state.pool.for_org(&org).await.unwrap();
    assert_eq!(count(db.test_raw(), "memory").await, 2);
    assert_eq!(count(state.control.test_raw(), "membership").await, 3);
}

/// A fresh install booted by two replicas at once: both create the namespace, the control database,
/// its user and the control schema, and both come up.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_replicas_creating_the_control_database_both_succeed() {
    let state = common::bare_state().await;
    let p = state.provisioner.clone().unwrap();
    for _ in 0..5 {
        let (a, b) = (p.clone(), p.clone());
        let (ra, rb) = tokio::join!(tokio::spawn(async move { a.ensure_control().await }), tokio::spawn(async move { b.ensure_control().await }));
        ra.unwrap().unwrap();
        rb.unwrap().unwrap();
    }
    assert!(state.pool.control().test_raw().query("SELECT count() FROM user GROUP ALL").await.is_ok());
}

/// A burst of cold requests for one org signs in once: the rest wait for the first and share its handle.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn twenty_concurrent_cold_for_org_calls_sign_in_once() {
    let app = TestApp::new().await;
    let org = app.user.org;
    app.state.pool.evict(&org);
    let before = app.state.pool.signins();
    let calls = (0..20).map(|_| {
        let pool = app.state.pool.clone();
        tokio::spawn(async move { pool.for_org(&org).await.map(|_| ()) })
    });
    for r in futures::future::join_all(calls).await {
        r.unwrap().unwrap();
    }
    assert_eq!(app.state.pool.signins() - before, 1, "one sign-in for twenty cold calls");
    assert_eq!(app.state.pool.open_handles(), 1);
}

/// The 2.x export has 3 users (two share an email in different case) and 3 vault members, none with an
/// `email_lc`: `INSERT IGNORE` must not trip the UNIQUE index on the missing value, and the backfill gives
/// the oldest of the duplicates the address; the later one is moved but cannot sign in by email.
#[tokio::test]
async fn a_2x_export_with_a_mixed_case_duplicate_email_moves_whole() {
    let (state, old) = state_with_legacy().await;
    let hash = bcrypt::hash("pw-12345678", 4).unwrap();
    old.query("UPDATE user SET password_hash = $h").bind(("h", hash)).await.unwrap().check().unwrap();
    run_move(&state).await;
    let org = only_org(&state).await;
    assert_eq!(count(state.control.test_raw(), "user").await, 3);
    assert_eq!(count(state.control.test_raw(), "membership").await, 3);
    let lcs: Vec<String> = state.control.test_raw().query("SELECT VALUE email_lc FROM user ORDER BY created_at").await.unwrap().take(0).unwrap();
    assert_eq!(lcs[0], "a@b.c");
    assert_eq!(lcs[1], "zed@example.com", "the oldest of the duplicates takes the address");
    assert!(lcs[2].starts_with("duplicate:"), "{lcs:?}");
    let db = state.pool.for_org(&org).await.unwrap();
    assert_eq!(count(db.test_raw(), "vault_member").await, 3);
    for email in ["a@b.c", "A@B.C", "zed@example.com", "ZED@EXAMPLE.COM"] {
        let u = eunomia_backend::models_user::authenticate(&state.control, email, "pw-12345678").await.unwrap();
        assert!(u.is_some(), "{email} cannot sign in after the move");
    }
}

/// An old install with an empty ENCRYPTION_KEY must not get its org database password written under the
/// public zero key: the move refuses until a key is set, and nothing is created.
#[tokio::test]
async fn the_move_refuses_an_empty_encryption_key() {
    let (state, _old) = state_with_legacy().await;
    let empty = eunomia_backend::config::Settings { encryption_key: String::new(), ..state.settings.clone() };
    let err = legacy::move_if_needed(state.provisioner.as_ref().unwrap(), &state.control, &empty).await.unwrap_err();
    assert!(err.message.contains("ENCRYPTION_KEY"), "{}", err.message);
    assert_eq!(count(state.control.test_raw(), "org").await, 0);
    run_move(&state).await; // with the real key it goes through
    assert_eq!(count(state.control.test_raw(), "org").await, 1);
}

/// The move inserts users after the control migrations ran, so their backfills must be applied to the
/// moved rows: sign-in by `email_lc`, a case-insensitive duplicate signup, and one owner with member slots.
#[tokio::test]
async fn moved_users_can_sign_in_in_any_case_and_memberships_carry_slots() {
    let (state, old) = state_with_legacy().await;
    let hash = bcrypt::hash("pw-12345678", 4).unwrap();
    old.query(
        "CREATE user:b SET email = 'Bob@Example.COM', password_hash = $h, created_at = time::now() + 1s;
         CREATE user:c SET email = 'carol@x.io', password_hash = $h, created_at = time::now() + 2s;
         CREATE user:d SET email = 'dave@x.io', password_hash = $h, created_at = time::now() + 3s;",
    )
    .bind(("h", hash))
    .await
    .unwrap()
    .check()
    .unwrap();
    run_move(&state).await;

    for email in ["Bob@Example.COM", "bob@example.com", "BOB@EXAMPLE.COM", "carol@x.io", "Carol@X.io", "DAVE@x.io"] {
        let u = eunomia_backend::models_user::authenticate(&state.control, email, "pw-12345678").await.unwrap();
        assert!(u.is_some(), "{email} cannot sign in after the move");
    }
    let dup = eunomia_backend::models_user::register_user_with(&state, "bOb@example.com", "pw-12345678", false).await;
    assert!(dup.is_err(), "a different-case signup of a moved email must be refused");
    assert_eq!(count(state.control.test_raw(), "user").await, 6);

    let owners = count(state.control.test_raw(), "membership WHERE role = 'owner'").await;
    assert_eq!(owners, 1);
    let slots = count(state.control.test_raw(), "membership WHERE slot != NONE").await;
    assert_eq!(slots, 6, "every moved membership has a slot");
    // the owner slot is taken: a further owner cannot be added
    let again = state.control.test_raw().query("CREATE membership SET user = user:zz, org = $o, role = 'owner', slot = string::concat(<string>$o, '/owner')")
        .bind(("o", only_org(&state).await.record())).await.unwrap().check();
    assert!(again.is_err());
}

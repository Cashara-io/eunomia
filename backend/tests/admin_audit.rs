//! Workspace admin: space changes land in the security ledger and read back through
//! `/api/admin/audit` as sentences; `/api/admin/people` lists every account. Both refuse non-admins.

mod common;

use axum::http::StatusCode;
use surrealdb::types::SurrealValue;
use eunomia_backend::models_user;
use eunomia_backend::rid;
use eunomia_backend::vaults::service as vaults;

#[tokio::test]
async fn admin_sees_space_events_and_people() {
    let app = common::TestApp::new().await;
    let bob = common::register(&app.state, "bob@example.com").await;
    let carol = common::register(&app.state, "carol@example.com").await;
    let db = app.db().await;
    let control = app.state.control.clone();
    let owner = app.user.id.clone();

    common::sys(Box::pin(async {
        let v = vaults::create_vault(&db, &control, &owner, "acme-engineering", "org").await.unwrap();
        let v = rid::parse(&v.id).unwrap();
        vaults::invite_member(&db, &control, &owner, &v, &bob.email, "member").await.unwrap();
        vaults::invite_member(&db, &control, &owner, &v, &carol.email, "member").await.unwrap();
        vaults::accept_invitation(&db, &control, &bob.id, &v).await.unwrap();
        vaults::set_member_role(&db, &control, &owner, &v, &bob.email, "owner").await.unwrap();
        // carol never accepted: removing her withdraws the invitation
        vaults::remove_member(&db, &control, &owner, &v, &carol.email).await.unwrap();
        // a non-owner is refused, and the refusal is recorded too
        assert!(vaults::invite_member(&db, &control, &carol.id, &v, &carol.email, "member").await.is_err());
        vaults::rename_vault(&db, &control, &owner, &v, "acme-eng").await.unwrap();
        let copy = vaults::clone_vault(&db, &control, &owner, &v, Some("acme-copy"), "org").await.unwrap();
        vaults::merge_vaults(&db, &control, &owner, &v, &rid::parse(&copy.id).unwrap(), Some("acme-all"), "org").await.unwrap();
        let gone = vaults::create_vault(&db, &control, &owner, "scratch", "org").await.unwrap();
        vaults::delete_vault(&db, &control, &owner, &rid::parse(&gone.id).unwrap()).await.unwrap();
    }))
    .await;

    let (status, page) = app.http("GET", "/api/admin/audit?category=spaces", None, true).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let rows = page["results"].as_array().unwrap();
    let summaries: Vec<&str> = rows.iter().map(|r| r["summary"].as_str().unwrap()).collect();
    // Rows name spaces by id, so sentences use each space's current name (acme-engineering was
    // renamed acme-eng); a deleted space keeps the name it had only on its delete row.
    for want in [
        "Deleted the space scratch",
        "Created a space that has since been deleted",
        "Merged acme-eng and acme-copy into a new space, acme-all",
        "Copied acme-eng into a new space, acme-copy",
        "Renamed acme-engineering to acme-eng",
        "Withdrew the invitation for carol@example.com to acme-eng",
        "Made bob@example.com an owner of acme-eng",
        "Joined acme-eng",
        "Invited bob@example.com to acme-eng",
        "Created the space acme-eng",
    ] {
        assert!(summaries.contains(&want), "missing {want:?} in {summaries:?}");
    }
    // newest first
    assert_eq!(summaries[0], "Deleted the space scratch");
    assert!(rows.iter().all(|r| r["category"] == "spaces"));
    let refused = rows.iter().find(|r| r["ok"] == false).expect("the refused invite is logged");
    assert_eq!(refused["actor_email"], "carol@example.com");
    assert_eq!(refused["result"], "Not allowed");
    assert_eq!(rows.iter().find(|r| r["action"] == "vault.invite" && r["ok"] == true).unwrap()["actor_email"], app.user.email.as_str());

    // the sign-in filter shows sign-ins only, and the tool-call rows never show
    app.session_cookie().await;
    let (_, signins) = app.http("GET", "/api/admin/audit?category=signins&limit=5", None, true).await;
    let signins = signins["results"].as_array().unwrap();
    assert!(!signins.is_empty() && signins.iter().all(|r| r["category"] == "signins"), "{signins:?}");
    // a server setting change reads in plain words, under "Server" and "Changes"
    let (status, _) = app.http("PATCH", "/api/settings", Some(serde_json::json!({ "openai_api_key": "sk-test" })), true).await;
    assert_eq!(status, StatusCode::OK);
    for category in ["server", "changes"] {
        let (_, page) = app.http("GET", &format!("/api/admin/audit?category={category}&limit=5"), None, true).await;
        let row = &page["results"][0];
        assert_eq!(row["action"], "server.ai_provider", "{category}: {page}");
        assert!(row["summary"].as_str().unwrap().starts_with("Replaced the"), "{row}");
        assert!(!row.to_string().contains("sk-test"), "the key never reaches the log");
    }
    let (_, all) = app.http("GET", "/api/admin/audit?limit=200", None, true).await;
    assert!(all["results"].as_array().unwrap().iter().all(|r| !r["action"].as_str().unwrap().starts_with("tool.")));

    let (status, people) = app.http("GET", "/api/admin/people", None, true).await;
    assert_eq!(status, StatusCode::OK, "{people}");
    let list = people["people"].as_array().unwrap();
    assert_eq!(list.len(), 3);
    let bob_row = list.iter().find(|p| p["email"] == "bob@example.com").unwrap();
    assert_eq!(bob_row["shared_spaces"], 1);
    assert_eq!(list.iter().find(|p| p["email"] == app.user.email.as_str()).unwrap()["admin"], "creator");
    assert!(people["signup"]["summary"].is_string());

    // a member is refused both
    let bob_token = models_user::create_api_token_with(&app.state.control, &bob.id, "t", &eunomia_backend::scopes::ALL.iter().map(|s| s.to_string()).collect::<Vec<_>>(), None, None).await.unwrap().token;
    for path in ["/api/admin/audit", "/api/admin/people"] {
        let ((status, _), _) = common::http(&app.router, "GET", path, None, Some(&bob_token), None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }
}


// These are the service refusal events that selector preflight must preserve. The
// inputs deliberately contain private arguments and a nonexistent foreign vault.
const DOMAIN_REFUSALS: &[(&str, &str)] = &[
    ("vault_invite", "vault.invite"),
    ("vault_remove_member", "vault.member_remove"),
    ("vault_set_role", "vault.role"),
    ("vault_transfer_ownership", "vault.transfer"),
    ("vault_delete", "vault.delete"),
    ("vault_rename", "vault.rename"),
    ("vault_clone", "vault.clone"),
    ("vault_merge", "vault.merge"),
    ("vault_accept_invitation", "vault.join"),
    ("vault_decline_invitation", "vault.decline"),
];

async fn restricted_caller(app: &common::TestApp) -> eunomia_backend::authz::Caller {
    use eunomia_backend::{authz::{Actor, Caller}, scopes};
    let bound = app.tool("ontology_get", serde_json::json!({})).await["vault"].as_str().unwrap().to_string();
    Caller { actor: Actor { kind: "token", id: "api_token:audit-fixture".into() },
        scopes: scopes::ALL.iter().map(|s| s.to_string()).collect(),
        vault: Some(rid::parse(&bound).unwrap()), expires_at: None }
}

async fn refuse_selector(app: &common::TestApp, caller: &eunomia_backend::authz::Caller, tool: &str) {
    use eunomia_backend::{authz, tools::registry};
    let args = serde_json::json!({
        "vault_id": "vault:nonexistent-foreign", "vault_id_a": "vault:nonexistent-foreign",
        "vault_id_b": "vault:also-foreign", "vault_ids": ["vault:nonexistent-foreign"],
        "email": "private-invitee@example.test", "name": "PRIVATE_ARGUMENT_CANARY",
        "role": "owner", "text": "PRIVATE_CONTENT_CANARY", "password": "PRIVATE_SECRET_CANARY"
    });
    let error = authz::with_caller(caller.clone(), registry::call(&app.state, &app.user, tool, args)).await.unwrap_err();
    assert_eq!(error.code, eunomia_backend::error::ErrorCode::AuthScope, "{tool}");
}

async fn ledger(app: &common::TestApp, category: &str) -> serde_json::Value {
    let (status, page) = app.http("GET", &format!("/api/admin/audit?limit=200&category={category}"), None, true).await;
    assert_eq!(status, StatusCode::OK);
    page
}

#[tokio::test]
async fn preflight_invite_refusal_is_visible_to_admins_with_safe_context() {
    let app = common::TestApp::new().await;
    let caller = restricted_caller(&app).await;
    let foreign = app.tool("vault_create", serde_json::json!({"name":"Other space","kind":"org"})).await["id"].as_str().unwrap().to_string();
    let issued = models_user::create_api_token_with(app.control(), &app.user.id, "restricted audit test", &caller.scopes, caller.vault.as_ref(), None).await.unwrap();
    let mut caller = caller;
    use eunomia_backend::rid::RecordIdExt;
    caller.actor.id = issued.id.to_string();
    let ((status, result), _) = common::http(&app.router, "POST", "/api/tools/vault_invite", Some(serde_json::json!({
        "vault_id":foreign,"email":"private-invitee@example.test","password":"PRIVATE_SECRET_CANARY"
    })), Some(&issued.token), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(result["code"], "auth.scope");
    let page = ledger(&app, "spaces").await;
    let row = page["results"].as_array().unwrap().iter().find(|r| r["action"] == "vault.invite").expect("preflight invite refusal in security ledger");
    assert_eq!(row["ok"], false);
    assert_eq!(row["result"], "Not allowed");
    assert_eq!(row["actor_email"], app.user.email);
    assert_eq!(row["via"], "API token");
    assert_safe_refusal(&app, &caller, "vault_invite", "vault.invite", &foreign).await;
}

async fn assert_safe_refusal(app: &common::TestApp, caller: &eunomia_backend::authz::Caller, tool: &str, action: &str, requested: &str) {
    use eunomia_backend::rid::RecordIdExt;
    #[derive(serde::Deserialize, surrealdb::types::SurrealValue)]
    struct Row { user: surrealdb::types::RecordId, actor_kind: String, actor_id: String, target: String, outcome: String, detail: String }
    let mut res = app.control().test_raw().query("SELECT user, actor_kind, actor_id, target, outcome, detail FROM audit_event WHERE action = $action")
        .bind(("action", action.to_string())).await.unwrap();
    let rows: Vec<Row> = res.take(0).unwrap();
    assert_eq!(rows.len(), 1, "{tool}");
    let row = &rows[0];
    assert_eq!(row.user, app.user.id);
    assert_eq!(row.actor_kind, "token");
    assert_eq!(row.actor_id, caller.actor.id);
    assert_eq!(row.outcome, "auth.scope");
    assert_eq!(row.target, eunomia_backend::vaults::service::HIDDEN_SPACE);
    let detail: serde_json::Value = serde_json::from_str(&row.detail).unwrap();
    assert_eq!(detail, serde_json::json!({"tool":tool, "bound_vault":caller.vault.as_ref().unwrap().to_string(),
        "requested_vault":rid::parse(requested).unwrap().to_string(), "selector": if tool == "vault_merge" { "vault_id_a" } else { "vault_id" },
        "reason":"vault_binding_mismatch"}));
    assert_no_private_input(app, tool).await;
}

async fn assert_no_private_input(app: &common::TestApp, tool: &str) {
    // Refused input must not leak into any audit row or replay capsule.
    for table in ["audit_event", "failure_capsule"] {
        let mut res = app.control().test_raw().query(format!("SELECT * FROM {table}")).await.unwrap();
        let stored = format!("{:?}", res.take::<surrealdb::types::Value>(0).unwrap());
        for private in ["private-invitee@example.test", "PRIVATE_ARGUMENT_CANARY", "PRIVATE_CONTENT_CANARY", "PRIVATE_SECRET_CANARY"] {
            assert!(!stored.contains(private), "{tool} leaked private input into {table}");
        }
    }
    let mut res = app.db().await.test_raw().query("SELECT * FROM audit_log").await.unwrap();
    let stored = format!("{:?}", res.take::<surrealdb::types::Value>(0).unwrap());
    assert!(!stored.contains("PRIVATE_") && !stored.contains("private-invitee"));
}

#[tokio::test]
async fn preflight_preserves_every_domain_refusal_event() {
    let app = common::TestApp::new().await;
    let caller = restricted_caller(&app).await;
    for &(tool, action) in DOMAIN_REFUSALS {
        refuse_selector(&app, &caller, tool).await;
        let page = ledger(&app, "spaces").await;
        assert!(page["results"].as_array().unwrap().iter().any(|r| r["action"] == action && r["ok"] == false), "{tool}: missing {action}");
        assert_safe_refusal(&app, &caller, tool, action, "vault:nonexistent-foreign").await;
    }
}

#[tokio::test]
async fn preflight_generic_refusals_are_visible_for_reads_and_writes() {
    // None of these handlers previously emitted a domain refusal event. In
    // particular, peer.share_code is audited by REST, not by the tool service.
    for tool in ["vault_share_code", "vault_repair", "vault_peers", "document_upload", "document_list", "entities_search", "ontology_get", "secret_request"] {
        let app = common::TestApp::new().await;
        let caller = restricted_caller(&app).await;
        refuse_selector(&app, &caller, tool).await;
        let page = ledger(&app, "all").await;
        assert!(page["results"].as_array().unwrap().iter().any(|r| r["action"] == "auth.scope_refused" && r["ok"] == false), "{tool}: missing generic security refusal");
        assert_safe_refusal(&app, &caller, tool, "auth.scope_refused", "vault:nonexistent-foreign").await;
    }
}

#[tokio::test]
async fn preflight_refusal_volume_is_bounded_across_tools_tokens_and_targets() {
    let app = common::TestApp::new().await;
    let mut caller = restricted_caller(&app).await;
    let started = std::time::Instant::now();
    for i in 0..60 {
        caller.actor.id = format!("api_token:flood-{i}");
        let tool = if i % 2 == 0 { "vault_invite" } else { "vault_peers" };
        // Different targets must not create fresh audit budgets.
        let error = eunomia_backend::authz::with_caller(caller.clone(), eunomia_backend::tools::registry::call(&app.state, &app.user, tool,
            serde_json::json!({"vault_id":format!("vault:foreign-{i}"),"email":"private@example.test"}))).await.unwrap_err();
        assert_eq!(error.code, eunomia_backend::error::ErrorCode::AuthScope);
    }
    let page = ledger(&app, "all").await;
    let refusals = page["results"].as_array().unwrap().iter().filter(|r| r["action"] == "vault.invite" || r["action"] == "auth.scope_refused").count();
    // Account for refill if a loaded machine takes more than two seconds.
    let budget = 30 + (started.elapsed().as_secs() / 2) as usize;
    assert!((1..=budget).contains(&refusals), "{refusals} security rows exceeded budget {budget}");
}


#[tokio::test]
async fn preflight_refusals_store_no_argument_values() {
    let app = common::TestApp::new().await;
    let caller = restricted_caller(&app).await;
    refuse_selector(&app, &caller, "vault_invite").await;
    assert_no_private_input(&app, "vault_invite").await;
}

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
    let result = authz::with_caller(caller.clone(), registry::call(&app.state, &app.user, tool, args)).await.unwrap_or_else(|e| e.to_tool_value());
    assert_eq!(result["code"], "auth.scope", "{tool}");
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
    assert_eq!(status, StatusCode::OK);
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

// Keep database I/O from automatically advancing Tokio's paused clock.
struct RefusalClock(tokio::task::JoinHandle<()>);
impl RefusalClock {
    fn freeze() -> Self {
        tokio::time::pause();
        Self(tokio::spawn(async { loop { tokio::task::yield_now().await; } }))
    }
}
impl Drop for RefusalClock {
    fn drop(&mut self) { self.0.abort(); }
}

#[tokio::test]
async fn preflight_domain_refusals_survive_another_tokens_exhausted_budget() {
    let app = common::TestApp::new().await;
    let admin = restricted_caller(&app).await;
    let mut reader = admin.clone();
    reader.actor.id = "api_token:reader".into();
    reader.scopes = vec![eunomia_backend::scopes::MEMORY_READ.into()];
    let _clock = RefusalClock::freeze();
    for _ in 0..30 { refuse_selector(&app, &reader, "vault_peers").await; }
    // Exhaust this token too, so a per-token budget cannot hide domain events.
    for _ in 0..30 { refuse_selector(&app, &admin, "vault_peers").await; }
    refuse_selector(&app, &admin, "vault_delete").await;
    assert_safe_refusal(&app, &admin, "vault_delete", "vault.delete", "vault:nonexistent-foreign").await;
    // The other nine old domain events survive too.
    for &(tool, action) in DOMAIN_REFUSALS.iter().filter(|(tool, _)| *tool != "vault_delete") {
        refuse_selector(&app, &admin, tool).await;
        assert_safe_refusal(&app, &admin, tool, action, "vault:nonexistent-foreign").await;
    }
}

#[tokio::test]
async fn preflight_generic_budget_is_per_token_across_tools_and_targets() {
    let app = common::TestApp::new().await;
    let mut caller = restricted_caller(&app).await;
    let _clock = RefusalClock::freeze();
    for i in 0..40 {
        let tool = if i % 2 == 0 { "vault_peers" } else { "ontology_get" };
        let result = eunomia_backend::authz::with_caller(caller.clone(), eunomia_backend::tools::registry::call(&app.state, &app.user, tool,
            serde_json::json!({"vault_id":format!("vault:foreign-{i}")}))).await.unwrap();
        assert_eq!(result["code"], "auth.scope");
    }
    assert_eq!(refusal_details(&app, "auth.scope_refused").await.len(), 30);
    caller.actor.id = "api_token:another-token".into();
    refuse_selector(&app, &caller, "vault_peers").await;
    assert_eq!(refusal_details(&app, "auth.scope_refused").await.len(), 31);
}

async fn refusal_details(app: &common::TestApp, action: &str) -> Vec<serde_json::Value> {
    #[derive(serde::Deserialize, surrealdb::types::SurrealValue)]
    struct Row { detail: String }
    let mut res = app.control().test_raw().query("SELECT detail FROM audit_event WHERE action = $action")
        .bind(("action", action.to_string())).await.unwrap();
    let rows: Vec<Row> = res.take(0).unwrap();
    rows.into_iter().map(|r| serde_json::from_str(&r.detail).expect("complete JSON detail")).collect()
}

async fn suppressed_refusals(app: &common::TestApp, caller: &eunomia_backend::authz::Caller) {
    for _ in 0..30 { refuse_selector(app, caller, "vault_peers").await; }
    for tool in ["vault_peers", "ontology_get", "vault_peers", "ontology_get", "vault_peers"] {
        refuse_selector(app, caller, tool).await;
    }
    assert!(refusal_details(app, "auth.scope_refused_suppressed").await.is_empty());
}

async fn assert_suppression_summary(app: &common::TestApp) {
    let rows = refusal_details(app, "auth.scope_refused_suppressed").await;
    assert_eq!(rows.len(), 1, "one summary for the suppression window");
    let detail = &rows[0];
    assert_eq!(detail["count"], 5);
    assert_eq!(detail["tools"], serde_json::json!(["ontology_get", "vault_peers"]));
    let start = chrono::DateTime::parse_from_rfc3339(detail["window_start"].as_str().unwrap()).unwrap();
    let end = chrono::DateTime::parse_from_rfc3339(detail["window_end"].as_str().unwrap()).unwrap();
    assert!(end >= start);
    let keys: std::collections::BTreeSet<_> = detail.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(keys, ["count", "tools", "window_start", "window_end"].into_iter().collect());
    assert_no_private_input(app, "suppression summary").await;
    let page = ledger(app, "all").await;
    let row = page["results"].as_array().unwrap().iter().find(|r| r["action"] == "auth.scope_refused_suppressed").expect("ledger-visible summary");
    assert!(row["summary"].as_str().unwrap().contains("5"), "{row}");
    assert_eq!(row["ok"], false);
}

#[tokio::test]
async fn preflight_suppression_is_coalesced_when_the_next_event_is_allowed() {
    let app = common::TestApp::new().await;
    let caller = restricted_caller(&app).await;
    let _clock = RefusalClock::freeze();
    suppressed_refusals(&app, &caller).await;
    tokio::time::advance(std::time::Duration::from_secs(2)).await;
    refuse_selector(&app, &caller, "vault_peers").await;
    assert_suppression_summary(&app).await;
    assert_eq!(refusal_details(&app, "auth.scope_refused").await.len(), 31);
    tokio::time::advance(std::time::Duration::from_secs(60)).await;
    for _ in 0..20 { tokio::task::yield_now().await; }
    assert_suppression_summary(&app).await;
}

#[tokio::test]
async fn preflight_suppression_flushes_for_a_quiet_user_without_another_request() {
    let app = common::TestApp::new().await;
    let caller = restricted_caller(&app).await;
    let _clock = RefusalClock::freeze();
    suppressed_refusals(&app, &caller).await;
    tokio::time::advance(std::time::Duration::from_secs(3)).await;
    // Allow for Tokio's millisecond timer rounding, then poll the database only.
    // No registry call or user request can flush the pending count here.
    for _ in 0..500 {
        if !refusal_details(&app, "auth.scope_refused_suppressed").await.is_empty() { break; }
        tokio::task::yield_now().await;
    }
    assert_suppression_summary(&app).await;
    assert_eq!(refusal_details(&app, "auth.scope_refused").await.len(), 30);
}

#[tokio::test]
async fn preflight_suppression_preserves_the_complete_distinct_tool_set() {
    let app = common::TestApp::new().await;
    let caller = restricted_caller(&app).await;
    let _clock = RefusalClock::freeze();
    for _ in 0..30 { refuse_selector(&app, &caller, "vault_peers").await; }
    let mut tools: Vec<_> = eunomia_backend::tools::registry::all_tools().iter()
        .filter(|(name, spec)| spec.schema["properties"].get("vault_id").is_some()
            && **name != "memory_write_batch" && !DOMAIN_REFUSALS.iter().any(|(tool, _)| tool == *name))
        .map(|(&name, _)| name).collect();
    tools.sort_unstable();
    let minimum_detail = serde_json::json!({"count":tools.len(), "tools":tools,
        "window_start":"2000-01-01T00:00:00Z", "window_end":"2000-01-01T00:00:02Z"});
    assert!(minimum_detail.to_string().len() > 500, "exercise a summary longer than the free-text audit cap");
    for tool in &tools { refuse_selector(&app, &caller, tool).await; }
    tokio::time::advance(std::time::Duration::from_secs(3)).await;
    for _ in 0..500 {
        if !refusal_details(&app, "auth.scope_refused_suppressed").await.is_empty() { break; }
        tokio::task::yield_now().await;
    }
    let rows = refusal_details(&app, "auth.scope_refused_suppressed").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["count"], tools.len());
    assert_eq!(rows[0]["tools"], serde_json::json!(tools));
    assert_no_private_input(&app, "complete summary").await;
}

#[tokio::test]
async fn preflight_long_selectors_keep_complete_json_detail() {
    let app = common::TestApp::new().await;
    let caller = restricted_caller(&app).await;
    let requested = format!("vault:{}", "long-selector".repeat(500));
    let result = eunomia_backend::authz::with_caller(caller, eunomia_backend::tools::registry::call(&app.state, &app.user, "vault_peers",
        serde_json::json!({"vault_id":requested}))).await.unwrap();
    assert_eq!(result["code"], "auth.scope");
    let rows = refusal_details(&app, "auth.scope_refused").await;
    assert_eq!(rows.len(), 1);
    assert!(rows[0]["requested_vault"].as_str().unwrap().len() < 300);
    assert_eq!(rows[0]["reason"], "vault_binding_mismatch");
}

#[tokio::test]
async fn preflight_refusals_store_no_argument_values() {
    let app = common::TestApp::new().await;
    let caller = restricted_caller(&app).await;
    refuse_selector(&app, &caller, "vault_invite").await;
    assert_no_private_input(&app, "vault_invite").await;
}

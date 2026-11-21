//! A hosted Cupel: organisations made, sized, paused and resumed by an outside billing service through the
//! operator API (`operator.rs`), plans and seats (`plan.rs`), and suspension enforced at the gate. See
//! docs/deployment.md, "Running a hosted service".
//!
//! The whole binary is a shared host like `org_admin.rs` (`EUNOMIA_SIGNUP_ORG=personal`, `SIGNUP=closed`): the
//! install's first account (`tester@example.com`, the operator of the server) is in an organisation of its own,
//! and every other organisation here comes from the operator API.

mod common;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use eunomia_backend::config::Settings;
use eunomia_backend::jobs::worker::{self, Registry};
use eunomia_backend::jobs::{self, NewJob, WorkerConfig};
use eunomia_backend::mail::MailBackend;
use eunomia_backend::models_user;
use eunomia_backend::plan::HostedConfig;
use eunomia_backend::ratelimit::RateConfig;
use eunomia_backend::rid::RecordIdExt;
use serde_json::{json, Value};

const TOKEN: &str = "operator-token-for-tests-0123456789abcdef";
const BILLING: &str = "https://billing.example.test/manage";

fn host_env() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // SAFETY: set once, before any test reads them, to the same values for every test in this binary
        unsafe {
            std::env::set_var("EUNOMIA_SIGNUP_ORG", "personal");
            std::env::set_var("SIGNUP", "closed");
            std::env::set_var("EUNOMIA_ADMIN_EMAILS", "op@corp.test");
            std::env::set_var("RATE_LIMIT_AUTH_PER_MIN", "1000");
        }
    });
}

fn hosted() -> HostedConfig {
    HostedConfig { operator_token: Some(TOKEN.into()), max_orgs: None, billing_url: Some(BILLING.into()) }
}

async fn host_with(hosted: HostedConfig) -> common::TestApp {
    host_env();
    common::TestApp::with_settings(Settings { mail: MailBackend::Capture, hosted, ..common::test_settings() }).await
}

async fn host() -> common::TestApp {
    host_with(hosted()).await
}

/// One call to the operator API with the operator token.
async fn op(app: &common::TestApp, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    common::http(&app.router, method, path, body, Some(TOKEN), None).await.0
}

async fn call(app: &common::TestApp, cookie: &str, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    common::http(&app.router, method, path, body, None, Some(cookie)).await.0
}

/// The fragment of a claim or invitation link.
fn token_of(link: &str) -> String {
    link.split_once("/invite#").unwrap_or_else(|| panic!("not an invitation link: {link}")).1.to_string()
}

/// Makes an organisation through the operator API and returns the whole answer.
async fn create_org(app: &common::TestApp, external_ref: &str, owner: &str, seats: i64) -> Value {
    let (status, body) = op(
        app,
        "POST",
        "/api/operator/v1/orgs",
        Some(json!({ "name": format!("Org {external_ref}"), "owner_email": owner, "plan": "team", "seat_limit": seats, "external_ref": external_ref })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

#[derive(Clone)]
struct P {
    email: String,
    cookie: String,
    bearer: String,
}

async fn person(app: &common::TestApp, email: &str, cookie: Option<String>) -> P {
    let cookie = match cookie {
        Some(c) => c,
        None => {
            let ((status, body), c) = common::http(&app.router, "POST", "/api/auth/login", Some(json!({ "email": email, "password": common::PASSWORD })), None, None).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            c.unwrap()
        }
    };
    let user = models_user::user_by_email(&app.state.control, email).await.unwrap().unwrap();
    let scopes: Vec<String> = eunomia_backend::scopes::ALL.iter().map(|s| s.to_string()).collect();
    let bearer = models_user::create_api_token_with(&app.state.control, &user.id, "t", &scopes, None, None).await.unwrap().token;
    P { email: email.into(), cookie: cookie.split(';').next().unwrap().to_string(), bearer }
}

/// Claims the organisation `link` names as `email`, with a new password account.
async fn claim(app: &common::TestApp, link: &str, email: &str) -> P {
    let ((status, body), cookie) = common::http(
        &app.router,
        "POST",
        "/api/auth/register",
        Some(json!({ "email": email, "password": common::PASSWORD, "invitation": token_of(link) })),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["org_role"], "owner", "{body}");
    person(app, email, cookie).await
}

/// An organisation made through the operator API and claimed: its id and its owner.
async fn owned_org(app: &common::TestApp, external_ref: &str, owner: &str, seats: i64) -> (String, P) {
    let made = create_org(app, external_ref, owner, seats).await;
    let owner = claim(app, made["claim_link"].as_str().unwrap(), owner).await;
    (made["organisation_id"].as_str().unwrap().to_string(), owner)
}

/// The owner invites `email`; returns the invitation token.
async fn invite(app: &common::TestApp, by: &P, email: &str) -> (StatusCode, Value) {
    call(app, &by.cookie, "POST", "/api/org/invitations", Some(json!({ "email": email, "role": "member" }))).await
}

/// Accepts an invitation by signing up through it.
async fn accept(app: &common::TestApp, token: &str, email: &str) -> (StatusCode, Value, Option<String>) {
    let ((status, body), cookie) =
        common::http(&app.router, "POST", "/api/auth/register", Some(json!({ "email": email, "password": common::PASSWORD, "invitation": token })), None, None).await;
    (status, body, cookie)
}

/// Invites `email` and signs them up through the link.
async fn join(app: &common::TestApp, owner: &P, email: &str) -> P {
    let (status, created) = invite(app, owner, email).await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let (status, body, cookie) = accept(app, created["token"].as_str().unwrap(), email).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    person(app, email, cookie).await
}

async fn org_summary(app: &common::TestApp, id: &str) -> Value {
    let (status, body) = op(app, "GET", &format!("/api/operator/v1/orgs/{id}"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

async fn plan_of(app: &common::TestApp, p: &P) -> Value {
    let (status, body) = call(app, &p.cookie, "GET", "/api/org/plan", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

async fn ledger(app: &common::TestApp, action: &str) -> Vec<Value> {
    let mut res = app
        .control()
        .test_raw()
        .query("SELECT action, actor_kind, actor_id, target, outcome, detail, (IF user != NONE THEN <string> user ELSE '' END) AS user FROM audit_event WHERE action = $a ORDER BY created_at")
        .bind(("a", action.to_string()))
        .await
        .unwrap();
    res.take(0).unwrap()
}

/// Writes plan fields on the organisation record directly, as another replica (or a bug) would.
async fn set_org(app: &common::TestApp, id: &str, set: &str) {
    let org = eunomia_backend::pool::OrgId::parse(id).expect("an org id").record();
    app.control().test_raw().query(format!("UPDATE $org SET {set}")).bind(("org", org)).await.unwrap().check().unwrap();
}

fn code(v: &Value) -> &str {
    v["code"].as_str().unwrap_or_default()
}

async fn memory_write(app: &common::TestApp, p: &P, text: &str) -> (StatusCode, Value) {
    common::http(&app.router, "POST", "/api/tools/memory_write", Some(json!({ "subject_name": "Ada", "subject_kind": "person", "text": text })), Some(&p.bearer), None).await.0
}

async fn mcp(app: &common::TestApp, p: &P) -> (StatusCode, Value) {
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "recall", "arguments": { "query": "Ada" } } });
    common::http(&app.router, "POST", "/mcp", Some(body), Some(&p.bearer), None).await.0
}

// ------------------------------------------------------------------ the routes exist only with a token

#[tokio::test]
async fn the_operator_routes_do_not_exist_without_a_token() {
    let app = host_with(HostedConfig::default()).await;
    for (m, path) in [
        ("GET", "/api/operator/v1/capacity"),
        ("GET", "/api/operator/v1/orgs"),
        ("POST", "/api/operator/v1/orgs"),
        ("GET", "/api/operator/v1/orgs/abc"),
        ("POST", "/api/operator/v1/orgs/abc/suspend"),
    ] {
        // with the token a hosted server would use, with a session, and with nothing: the same 404
        assert_eq!(common::http(&app.router, m, path, None, Some(TOKEN), None).await.0 .0, StatusCode::NOT_FOUND, "{m} {path} (token)");
        assert_eq!(common::http(&app.router, m, path, None, None, None).await.0 .0, StatusCode::NOT_FOUND, "{m} {path} (nothing)");
        assert_eq!(common::http(&app.router, m, path, None, Some(&app.token), None).await.0 .0, StatusCode::NOT_FOUND, "{m} {path} (api token)");
    }
    // nothing else changed: an organisation with no plan answers with nulls and an unlimited seat count
    let me = person(&app, "tester@example.com", None).await;
    let plan = plan_of(&app, &me).await;
    // every plan field is null; the `ai` block (model use) says AI is on with no allowance and no key of its own
    assert!(plan.as_object().unwrap().iter().all(|(k, v)| k == "ai" || v.is_null()), "{plan}");
    assert_eq!(plan["ai"], json!({ "enabled": true, "own_key": false, "usage": { "state": null, "percent": null } }), "{plan}");
    for i in 0..6 {
        let (status, body) = invite(&app, &me, &format!("free{i}@example.com")).await;
        assert_eq!(status, StatusCode::OK, "unlimited: {body}");
    }
    assert!(ledger(&app, "operator.org_create").await.is_empty());
}

// ------------------------------------------------------------------ creating, idempotently, and claiming

#[tokio::test]
async fn creating_is_idempotent_and_the_claim_link_makes_the_owner() {
    let app = host().await;
    let before = op(&app, "GET", "/api/operator/v1/capacity", None).await.1["organisations"].as_i64().unwrap();
    let made = create_org(&app, "sub_100", "Owner@Acme.test", 5).await;
    assert_eq!(made["created"], true, "{made}");
    let id = made["organisation_id"].as_str().unwrap().to_string();
    assert_eq!(id.len(), 32, "{id}");
    let link = made["claim_link"].as_str().unwrap().to_string();
    assert!(link.contains("/invite#"), "{link}");
    // mail is on (captured): the link went to the owner and the response says so
    assert_eq!((made["email_enabled"].clone(), made["emailed"].clone()), (json!(true), json!(true)), "{made}");
    let mails = app.state.mailer.captured();
    assert_eq!(mails.len(), 1);
    assert_eq!(mails[0].to, "owner@acme.test");
    assert!(mails[0].body.contains(&link) && mails[0].body.contains("Org sub_100"), "{}", mails[0].body);
    let org = &made["organisation"];
    assert_eq!((org["plan"].clone(), org["status"].clone(), org["seat_limit"].clone()), (json!("team"), json!("active"), json!(5)), "{org}");
    assert_eq!((org["owner_email"].clone(), org["owner_claimed"].clone(), org["seats"]["pending_invitations"].clone()), (json!("owner@acme.test"), json!(false), json!(1)), "{org}");
    assert_eq!(org["external_ref"], "sub_100");

    // the billing service retries: the same organisation, no second one, no second link
    let again = create_org(&app, "sub_100", "owner@acme.test", 5).await;
    assert_eq!((again["created"].clone(), again["organisation_id"].clone(), again["claim_link"].clone()), (json!(false), json!(id), Value::Null), "{again}");
    assert_eq!(app.state.mailer.captured().len(), 1, "a retry sends no second mail");
    let after = op(&app, "GET", "/api/operator/v1/capacity", None).await.1["organisations"].as_i64().unwrap();
    assert_eq!(after, before + 1);
    // the same reference for another owner is a mistake, not a retry
    let (status, body) = op(&app, "POST", "/api/operator/v1/orgs", Some(json!({ "name": "X", "owner_email": "someone@else.test", "plan": "team", "seat_limit": 3, "external_ref": "sub_100" }))).await;
    assert_eq!((status, code(&body)), (StatusCode::CONFLICT, "operator.reference_reused"), "{body}");
    // and a retried call with concurrent twins is still one organisation
    let (a, b) = tokio::join!(create_org(&app, "sub_200", "twin@acme.test", 2), create_org(&app, "sub_200", "twin@acme.test", 2));
    assert_eq!(a["organisation_id"], b["organisation_id"]);
    let made_it = [&a, &b].iter().filter(|r| r["created"] == true).count();
    let has_link = [&a, &b].iter().filter(|r| r["claim_link"].is_string()).count();
    assert_eq!((made_it, has_link), (1, 1), "exactly one of two overlapping calls made the organisation and holds its link: {a} {b}");
    assert_eq!(app.state.mailer.captured().len(), 2, "one mail for sub_100, one for sub_200");
    assert_eq!(op(&app, "GET", "/api/operator/v1/capacity", None).await.1["organisations"].as_i64().unwrap(), before + 2);

    // the claim page names the organisation and the owner role
    let (status, lookup) = common::http(&app.router, "POST", "/api/invitations/lookup", Some(json!({ "token": token_of(&link) })), None, None).await.0;
    assert_eq!(status, StatusCode::OK, "{lookup}");
    assert_eq!((lookup["role"].clone(), lookup["org_name"].clone(), lookup["email"].clone()), (json!("owner"), json!("Org sub_100"), json!("owner@acme.test")), "{lookup}");

    // the person who opens it creates their account and is the organisation's owner (SIGNUP=closed does not matter)
    let owner = claim(&app, &link, "owner@acme.test").await;
    let (status, o) = call(&app, &owner.cookie, "GET", "/api/admin/org", None).await;
    assert_eq!((status, o["your_role"].clone(), o["name"].clone()), (StatusCode::OK, json!("owner"), json!("Org sub_100")), "{o}");
    let summary = org_summary(&app, &id).await;
    assert_eq!((summary["owner_claimed"].clone(), summary["owner_email"].clone()), (json!(true), json!("owner@acme.test")), "{summary}");
    assert_eq!((summary["seats"]["active_members"].clone(), summary["seats"]["pending_invitations"].clone(), summary["seats"]["in_use"].clone()), (json!(1), json!(0), json!(1)));
    // the second use of the link is refused, by sign-up and by the page
    let (status, body, _) = accept(&app, &token_of(&link), "intruder@acme.test").await;
    assert_eq!((status, code(&body)), (StatusCode::NOT_FOUND, "invitation.not_found"), "{body}");
    let (status, _) = common::http(&app.router, "POST", "/api/invitations/lookup", Some(json!({ "token": token_of(&link) })), None, None).await.0;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // a retry after the claim returns the organisation, with no link
    let late = create_org(&app, "sub_100", "owner@acme.test", 5).await;
    assert_eq!((late["created"].clone(), late["claim_link"].clone(), late["organisation"]["owner_claimed"].clone()), (json!(false), Value::Null, json!(true)));
    // the owner's plan status
    let plan = plan_of(&app, &owner).await;
    assert_eq!(
        (plan["plan"].clone(), plan["status"].clone(), plan["seat_limit"].clone(), plan["seats_in_use"].clone(), plan["manage_url"].clone()),
        (json!("team"), json!("active"), json!(5), json!(1), json!(BILLING)),
        "{plan}"
    );
}

#[tokio::test]
async fn a_claim_expires_and_the_resend_kills_the_old_link() {
    let app = host().await;
    let made = create_org(&app, "sub_300", "late@acme.test", 3).await;
    let (id, old) = (made["organisation_id"].as_str().unwrap().to_string(), made["claim_link"].as_str().unwrap().to_string());
    let (status, again) = op(&app, "POST", &format!("/api/operator/v1/orgs/{id}/owner-claim/resend"), None).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    let fresh = again["claim_link"].as_str().unwrap().to_string();
    assert_ne!(fresh, old);
    assert_eq!((again["email_enabled"].clone(), again["emailed"].clone()), (json!(true), json!(true)));
    assert_eq!(app.state.mailer.captured().len(), 2);
    // the old link is dead, for the page and for sign-up
    let (status, _) = common::http(&app.router, "POST", "/api/invitations/lookup", Some(json!({ "token": token_of(&old) })), None, None).await.0;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body, _) = accept(&app, &token_of(&old), "late@acme.test").await;
    assert_eq!((status, code(&body)), (StatusCode::NOT_FOUND, "invitation.not_found"), "{body}");
    // an expired claim is refused as expired, and resending it makes a working one
    app.control().test_raw().query("UPDATE org_invitation SET expires_at = time::now() - 1h WHERE role = 'owner'").await.unwrap().check().unwrap();
    let (status, body, _) = accept(&app, &token_of(&fresh), "late@acme.test").await;
    assert_eq!((status, code(&body)), (StatusCode::GONE, "invitation.expired"), "{body}");
    let summary = org_summary(&app, &id).await;
    assert_eq!(summary["owner_claimed"], false);
    let (_, third) = op(&app, "POST", &format!("/api/operator/v1/orgs/{id}/owner-claim/resend"), None).await;
    claim(&app, third["claim_link"].as_str().unwrap(), "late@acme.test").await;
    // claimed: nothing left to resend
    let (status, body) = op(&app, "POST", &format!("/api/operator/v1/orgs/{id}/owner-claim/resend"), None).await;
    assert_eq!((status, code(&body)), (StatusCode::CONFLICT, "operator.owner_claimed"), "{body}");
}

#[tokio::test]
async fn an_owner_email_with_an_account_elsewhere_or_reserved_is_refused() {
    let app = host().await;
    common::register_personal(&app.state, "alice@example.com").await;
    let before = op(&app, "GET", "/api/operator/v1/capacity", None).await.1["organisations"].as_i64().unwrap();
    let (status, body) = op(&app, "POST", "/api/operator/v1/orgs", Some(json!({ "name": "Alice Co", "owner_email": "Alice@Example.com", "plan": "trial", "seat_limit": 3, "external_ref": "sub_alice" }))).await;
    assert_eq!((status, code(&body)), (StatusCode::CONFLICT, "operator.owner_in_other_org"), "{body}");
    // an operator's address can never be made an owner
    let (status, body) = op(&app, "POST", "/api/operator/v1/orgs", Some(json!({ "name": "Op Co", "owner_email": "OP@corp.test", "plan": "trial", "seat_limit": 3, "external_ref": "sub_op" }))).await;
    assert_eq!((status, code(&body)), (StatusCode::BAD_REQUEST, "operator.owner_email_reserved"), "{body}");
    // neither made an organisation
    assert_eq!(op(&app, "GET", "/api/operator/v1/capacity", None).await.1["organisations"].as_i64().unwrap(), before);
    // a claim whose email got an account meanwhile cannot be resent
    let made = create_org(&app, "sub_race", "soon@acme.test", 3).await;
    common::register_personal(&app.state, "soon@acme.test").await;
    let id = made["organisation_id"].as_str().unwrap();
    let (status, body) = op(&app, "POST", &format!("/api/operator/v1/orgs/{id}/owner-claim/resend"), None).await;
    assert_eq!((status, code(&body)), (StatusCode::CONFLICT, "operator.owner_in_other_org"), "{body}");
    // and the claim itself cannot be used by that account
    let (status, body, _) = accept(&app, &token_of(made["claim_link"].as_str().unwrap()), "soon@acme.test").await;
    assert_eq!((status, code(&body)), (StatusCode::CONFLICT, "auth.email_taken"), "{body}");
    // bad input is a 400, not a half-made organisation
    for bad in [
        json!({ "name": "", "owner_email": "a@b.test", "plan": "team", "seat_limit": 3, "external_ref": "r1" }),
        json!({ "name": "A@B", "owner_email": "a@b.test", "plan": "team", "seat_limit": 3, "external_ref": "r2" }),
        json!({ "name": "A", "owner_email": "not-an-email", "plan": "team", "seat_limit": 3, "external_ref": "r3" }),
        json!({ "name": "A", "owner_email": "a@b.test", "plan": "team", "seat_limit": 0, "external_ref": "r4" }),
        json!({ "name": "A", "owner_email": "a@b.test", "plan": "team", "seat_limit": 3, "external_ref": "" }),
    ] {
        let (status, body) = op(&app, "POST", "/api/operator/v1/orgs", Some(bad.clone())).await;
        assert_eq!((status, code(&body)), (StatusCode::BAD_REQUEST, "validation.invalid"), "{bad}: {body}");
    }
    let (status, _) = op(&app, "POST", "/api/operator/v1/orgs", Some(json!({ "name": "A", "owner_email": "a@b.test", "plan": "gold", "seat_limit": 3, "external_ref": "r5" }))).await;
    assert!(status.is_client_error(), "an unknown plan is refused");
}

#[tokio::test]
async fn the_server_refuses_new_organisations_when_full() {
    // the install's own organisation counts: one exists before anything is made
    let app = host_with(HostedConfig { max_orgs: Some(3), ..hosted() }).await;
    let cap = op(&app, "GET", "/api/operator/v1/capacity", None).await.1;
    assert_eq!((cap["organisations"].clone(), cap["max_organisations"].clone(), cap["at_capacity"].clone()), (json!(1), json!(3), json!(false)), "{cap}");
    create_org(&app, "cap_1", "c1@acme.test", 2).await;
    create_org(&app, "cap_2", "c2@acme.test", 2).await;
    let cap = op(&app, "GET", "/api/operator/v1/capacity", None).await.1;
    assert_eq!((cap["organisations"].clone(), cap["at_capacity"].clone()), (json!(3), json!(true)), "{cap}");
    let (status, body) = op(&app, "POST", "/api/operator/v1/orgs", Some(json!({ "name": "Three", "owner_email": "c3@acme.test", "plan": "team", "seat_limit": 2, "external_ref": "cap_3" }))).await;
    assert_eq!((status, code(&body)), (StatusCode::CONFLICT, "operator.at_capacity"), "{body}");
    assert!(body["detail"].as_str().unwrap().contains('3'), "{body}");
    assert_eq!(op(&app, "GET", "/api/operator/v1/capacity", None).await.1["organisations"], 3, "a refused create leaves nothing behind");
    // a retry of an organisation that exists is not a new one
    let again = create_org(&app, "cap_1", "c1@acme.test", 2).await;
    assert_eq!(again["created"], false);
    // no limit set: never at capacity
    let open = host().await;
    assert_eq!(op(&open, "GET", "/api/operator/v1/capacity", None).await.1["max_organisations"], Value::Null);
}

// ------------------------------------------------------------------ list, get, update

#[tokio::test]
async fn list_get_and_update_show_plan_state_and_refuse_to_go_below_usage() {
    let app = host().await;
    let (id, owner) = owned_org(&app, "upd_1", "u1@acme.test", 4).await;
    join(&app, &owner, "m1@acme.test").await;
    invite(&app, &owner, "pending@acme.test").await;
    // list: every organisation, the install's own with no plan
    let (status, list) = op(&app, "GET", "/api/operator/v1/orgs?limit=50", None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let rows = list["organisations"].as_array().unwrap();
    assert_eq!(list["total"], rows.len());
    let mine = rows.iter().find(|r| r["id"] == id).unwrap();
    assert_eq!((mine["seats"]["active_members"].clone(), mine["seats"]["pending_invitations"].clone(), mine["seats"]["in_use"].clone()), (json!(2), json!(1), json!(3)), "{mine}");
    let default = rows.iter().find(|r| r["plan"].is_null()).expect("the server's own organisation, unmanaged");
    assert_eq!((default["status"].clone(), default["seat_limit"].clone()), (json!("active"), Value::Null));
    // get an organisation that does not exist, and ids that are not ids
    for bad in ["0123456789abcdef0123456789abcdef", "nope", "org:", "%20"] {
        let (status, body) = op(&app, "GET", &format!("/api/operator/v1/orgs/{bad}"), None).await;
        assert_eq!((status, code(&body)), (StatusCode::NOT_FOUND, "operator.org_not_found"), "{bad}: {body}");
    }
    // lowering the limit below the 3 seats in use is refused, with the number, and changes nothing
    let (status, body) = op(&app, "PATCH", &format!("/api/operator/v1/orgs/{id}"), Some(json!({ "seat_limit": 2, "plan": "enterprise" }))).await;
    assert_eq!((status, code(&body)), (StatusCode::CONFLICT, "operator.seats_in_use"), "{body}");
    assert!(body["detail"].as_str().unwrap().contains("3 seats"), "{body}");
    let same = org_summary(&app, &id).await;
    assert_eq!((same["seat_limit"].clone(), same["plan"].clone()), (json!(4), json!("team")), "a refused update changes nothing: {same}");
    // exactly the usage is fine; so are the other fields, a null to clear, and a rename
    let (status, body) = op(
        &app,
        "PATCH",
        &format!("/api/operator/v1/orgs/{id}"),
        Some(json!({ "seat_limit": 3, "plan": "enterprise", "name": "Renamed Co", "trial_ends_at": "2030-01-02T03:04:05Z" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!((body["seat_limit"].clone(), body["plan"].clone(), body["name"].clone()), (json!(3), json!("enterprise"), json!("Renamed Co")), "{body}");
    assert!(body["trial_ends_at"].as_str().unwrap().starts_with("2030-01-02"), "{body}");
    let plan = plan_of(&app, &owner).await;
    assert!(plan["trial_ends_at"].as_str().unwrap().starts_with("2030-01-02") && plan["seat_limit"] == 3, "{plan}");
    let (_, body) = op(&app, "PATCH", &format!("/api/operator/v1/orgs/{id}"), Some(json!({ "seat_limit": null, "trial_ends_at": null }))).await;
    assert_eq!((body["seat_limit"].clone(), body["trial_ends_at"].clone(), body["plan"].clone()), (Value::Null, Value::Null, json!("enterprise")), "{body}");
    // unlimited now: invitations pass the old limit
    for i in 0..3 {
        let (status, b) = invite(&app, &owner, &format!("more{i}@acme.test")).await;
        assert_eq!(status, StatusCode::OK, "{b}");
    }
    // typos and bad values
    for bad in [json!({ "seatlimit": 3 }), json!({ "seat_limit": 0 }), json!({ "plan": "gold" }), json!({ "name": "a@b" })] {
        let (status, _) = op(&app, "PATCH", &format!("/api/operator/v1/orgs/{id}"), Some(bad.clone())).await;
        assert!(status.is_client_error(), "{bad}");
    }
}

// ------------------------------------------------------------------ seats

#[tokio::test]
async fn invitations_stop_at_the_seat_limit_and_suspended_members_free_a_seat() {
    let app = host().await;
    let (id, owner) = owned_org(&app, "seat_1", "s1@acme.test", 3).await;
    let (status, a) = invite(&app, &owner, "a@acme.test").await;
    assert_eq!(status, StatusCode::OK, "{a}");
    let (status, b) = invite(&app, &owner, "b@acme.test").await;
    assert_eq!(status, StatusCode::OK, "{b}");
    // owner + 2 pending = 3 of 3: the next is refused, with the number
    let (status, body) = invite(&app, &owner, "c@acme.test").await;
    assert_eq!((status, code(&body)), (StatusCode::CONFLICT, "org.seat_limit"), "{body}");
    assert!(body["detail"].as_str().unwrap().contains("3 seats"), "{body}");
    // accepting turns a pending seat into a member: still 3 of 3
    let (status, joined, cookie) = accept(&app, a["token"].as_str().unwrap(), "a@acme.test").await;
    assert_eq!(status, StatusCode::OK, "{joined}");
    let a_person = person(&app, "a@acme.test", cookie).await;
    assert_eq!(invite(&app, &owner, "c@acme.test").await.0, StatusCode::CONFLICT);
    // the seats are on the members' plan status and the owner's seat counts
    assert_eq!(plan_of(&app, &a_person).await["seats_in_use"], 3);
    // suspending a member frees a seat for an invitation ...
    let a_id = models_user::user_by_email(&app.state.control, "a@acme.test").await.unwrap().unwrap().id.to_string();
    let (status, body) = call(&app, &owner.cookie, "PUT", &format!("/api/admin/people/{a_id}/suspended"), Some(json!({ "suspended": true }))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, c) = invite(&app, &owner, "c@acme.test").await;
    assert_eq!(status, StatusCode::OK, "a suspended member holds no seat: {c}");
    assert_eq!(org_summary(&app, &id).await["seats"]["suspended_members"], 1);
    // ... and restoring takes it back, so with the org full the restore is refused
    let (status, body) = call(&app, &owner.cookie, "PUT", &format!("/api/admin/people/{a_id}/suspended"), Some(json!({ "suspended": false }))).await;
    assert_eq!((status, code(&body)), (StatusCode::CONFLICT, "org.seat_limit"), "{body}");
    assert!(body["detail"].as_str().unwrap().contains("3 seats"), "{body}");
    let still = call(&app, &owner.cookie, "GET", "/api/admin/people", None).await.1;
    let row = still["people"].as_array().unwrap().iter().find(|r| r["email"] == "a@acme.test").unwrap().clone();
    assert!(!row["suspended_at"].is_null(), "still suspended: {row}");
    // withdraw the invitation and the restore goes through
    let invitation = c["invitation"]["id"].as_str().unwrap();
    let (status, _) = call(&app, &owner.cookie, "DELETE", &format!("/api/org/invitations/{invitation}"), None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = call(&app, &owner.cookie, "PUT", &format!("/api/admin/people/{a_id}/suspended"), Some(json!({ "suspended": false }))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // resending an invitation that had lapsed takes a seat again: refused when full
    app.control().test_raw().query("UPDATE org_invitation SET expires_at = time::now() - 1h WHERE email = 'b@acme.test'").await.unwrap().check().unwrap();
    assert_eq!(org_summary(&app, &id).await["seats"]["pending_invitations"], 0);
    let (status, _) = invite(&app, &owner, "d@acme.test").await;
    assert_eq!(status, StatusCode::OK, "the lapsed invitation's seat was free");
    let b_id = b["invitation"]["id"].as_str().unwrap();
    let (status, body) = call(&app, &owner.cookie, "POST", &format!("/api/org/invitations/{b_id}/resend"), None).await;
    assert_eq!((status, code(&body)), (StatusCode::CONFLICT, "org.seat_limit"), "{body}");
    // an unmanaged organisation (the server's own) has no limit at all
    let me = person(&app, "tester@example.com", None).await;
    for i in 0..4 {
        assert_eq!(invite(&app, &me, &format!("u{i}@example.com")).await.0, StatusCode::OK);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_people_racing_for_the_last_seat_do_not_both_get_it() {
    let app = host().await;
    let (id, owner) = owned_org(&app, "race_1", "r1@acme.test", 5).await;
    for i in 0..3 {
        assert_eq!(invite(&app, &owner, &format!("held{i}@acme.test")).await.0, StatusCode::OK);
    }
    // owner + 3 pending = 4 of 5: one seat, six invitations at once
    let mut tasks = Vec::new();
    for i in 0..6 {
        let (router, cookie) = (app.router.clone(), owner.cookie.clone());
        tasks.push(tokio::spawn(async move {
            common::http(&router, "POST", "/api/org/invitations", Some(json!({ "email": format!("racer{i}@acme.test"), "role": "member" })), None, Some(&cookie)).await.0
        }));
    }
    let mut won = 0;
    for t in tasks {
        let (status, body) = t.await.unwrap();
        match status {
            StatusCode::OK => won += 1,
            StatusCode::CONFLICT => assert_eq!(code(&body), "org.seat_limit", "{body}"),
            other => panic!("{other}: {body}"),
        }
    }
    assert_eq!(won, 1, "exactly one invitation got the last seat");
    assert_eq!(org_summary(&app, &id).await["seats"]["in_use"], 5);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_restores_racing_for_the_last_seat_do_not_both_get_it() {
    let app = host().await;
    let (id, owner) = owned_org(&app, "race_2", "r2@acme.test", 3).await;
    let m1 = join(&app, &owner, "m1@acme.test").await;
    let m2 = join(&app, &owner, "m2@acme.test").await;
    let mut ids = Vec::new();
    for m in [&m1, &m2] {
        let uid = models_user::user_by_email(&app.state.control, &m.email).await.unwrap().unwrap().id.to_string();
        let (status, body) = call(&app, &owner.cookie, "PUT", &format!("/api/admin/people/{uid}/suspended"), Some(json!({ "suspended": true }))).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        ids.push(uid);
    }
    // the owner + one invitation = 2 of 3: one seat for two restores
    assert_eq!(invite(&app, &owner, "held@acme.test").await.0, StatusCode::OK);
    let mut tasks = Vec::new();
    for uid in ids {
        let (router, cookie) = (app.router.clone(), owner.cookie.clone());
        tasks.push(tokio::spawn(async move {
            common::http(&router, "PUT", &format!("/api/admin/people/{uid}/suspended"), Some(json!({ "suspended": false })), None, Some(&cookie)).await.0
        }));
    }
    let results: Vec<(StatusCode, Value)> = futures::future::join_all(tasks).await.into_iter().map(|r| r.unwrap()).collect();
    let ok = results.iter().filter(|(s, _)| *s == StatusCode::OK).count();
    assert_eq!(ok, 1, "{results:?}");
    assert!(results.iter().any(|(s, b)| *s == StatusCode::CONFLICT && code(b) == "org.seat_limit"), "{results:?}");
    assert_eq!(org_summary(&app, &id).await["seats"]["in_use"], 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn accepting_invitations_never_takes_the_organisation_past_its_limit() {
    let app = host().await;
    let (id, owner) = owned_org(&app, "race_3", "r3@acme.test", 3).await;
    let a = invite(&app, &owner, "a@acme.test").await.1;
    let b = invite(&app, &owner, "b@acme.test").await.1;
    // two accepted at the same moment: each turns its own pending seat into a member, so both get in and
    // the organisation sits exactly at its limit
    let mut tasks = Vec::new();
    for (inv, email) in [(a.clone(), "a@acme.test"), (b.clone(), "b@acme.test")] {
        let router = app.router.clone();
        tasks.push(tokio::spawn(async move {
            common::http(&router, "POST", "/api/auth/register", Some(json!({ "email": email, "password": common::PASSWORD, "invitation": inv["token"] })), None, None).await.0
        }));
    }
    for t in tasks {
        let (status, body) = t.await.unwrap();
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let s = org_summary(&app, &id).await;
    assert_eq!((s["seats"]["active_members"].clone(), s["seats"]["in_use"].clone()), (json!(3), json!(3)), "{s}");

    // an organisation that is already over its limit (the limit lowered out of band) lets nobody else in:
    // the accept is refused, the invitation stays open and no account is left behind
    let (id2, owner2) = owned_org(&app, "race_4", "r4@acme.test", 4).await;
    let x = invite(&app, &owner2, "x@acme.test").await.1;
    let y = invite(&app, &owner2, "y@acme.test").await.1;
    set_org(&app, &id2, "seat_limit = 2").await;
    let (status, body, _) = accept(&app, x["token"].as_str().unwrap(), "x@acme.test").await;
    assert_eq!((status, code(&body)), (StatusCode::CONFLICT, "org.seat_limit"), "{body}");
    assert!(body["detail"].as_str().unwrap().contains("2 seats"), "{body}");
    assert!(models_user::user_by_email(&app.state.control, "x@acme.test").await.unwrap().is_none(), "no half-made account");
    let s = org_summary(&app, &id2).await;
    assert_eq!((s["seats"]["active_members"].clone(), s["seats"]["pending_invitations"].clone()), (json!(1), json!(2)), "{s}");
    // room again (the limit raised): both still work
    set_org(&app, &id2, "seat_limit = 3").await;
    assert_eq!(accept(&app, x["token"].as_str().unwrap(), "x@acme.test").await.0, StatusCode::OK);
    assert_eq!(accept(&app, y["token"].as_str().unwrap(), "y@acme.test").await.0, StatusCode::OK);
}

#[tokio::test]
async fn signing_up_into_a_full_organisation_is_refused() {
    // an install where sign-up joins the install's one organisation (EUNOMIA_SIGNUP_ORG=join) still honours a seat limit
    let app = host().await;
    let (_, list) = op(&app, "GET", "/api/operator/v1/orgs", None).await;
    let default = list["organisations"].as_array().unwrap().iter().find(|r| r["plan"].is_null()).unwrap()["id"].as_str().unwrap().to_string();
    let (status, body) = op(&app, "PATCH", &format!("/api/operator/v1/orgs/{default}"), Some(json!({ "seat_limit": 1 }))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let refused = models_user::register_user_with(&app.state, "joiner@example.com", common::PASSWORD, false).await.unwrap_err();
    assert_eq!(refused.code, eunomia_backend::error::ErrorCode::OrgSeatLimit, "{}", refused.message);
    assert!(refused.message.contains("1 seat "), "{}", refused.message);
    assert!(models_user::user_by_email(&app.state.control, "joiner@example.com").await.unwrap().is_none(), "no half-made account");
    op(&app, "PATCH", &format!("/api/operator/v1/orgs/{default}"), Some(json!({ "seat_limit": 2 }))).await;
    let joined = models_user::register_user_with(&app.state, "joiner@example.com", common::PASSWORD, false).await.unwrap();
    assert_eq!(org_summary(&app, &default).await["seats"]["active_members"], 2);
    drop(joined);
}

// ------------------------------------------------------------------ suspension

#[tokio::test]
async fn a_suspended_organisation_is_refused_over_rest_and_mcp_and_another_is_untouched() {
    let app = host().await;
    let (a_id, a_owner) = owned_org(&app, "sus_a", "oa@acme.test", 5).await;
    let a_member = join(&app, &a_owner, "ma@acme.test").await;
    let (b_id, b_owner) = owned_org(&app, "sus_b", "ob@acme.test", 5).await;
    let b_member = join(&app, &b_owner, "mb@acme.test").await;

    // while active, a member writes and reads through both doors
    let (status, w) = memory_write(&app, &a_member, "Ada likes tea").await;
    assert_eq!(status, StatusCode::OK, "{w}");
    assert_eq!(mcp(&app, &a_member).await.0, StatusCode::OK);

    // the billing service suspends it
    let (status, body) = op(&app, "POST", &format!("/api/operator/v1/orgs/{a_id}/suspend"), Some(json!({ "reason": "payment_failed" }))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!((body["status"].clone(), body["suspended_reason"].clone()), (json!("suspended"), json!("payment_failed")), "{body}");

    // the very next request is refused, whatever the door: REST tool, MCP, REST routes, uploads, chat, connectors
    for p in [&a_member, &a_owner] {
        let (status, body) = memory_write(&app, p, "Ada likes coffee").await;
        assert_eq!((status, code(&body)), (StatusCode::FORBIDDEN, "org.suspended"), "{} {body}", p.email);
        let (status, body) = mcp(&app, p).await;
        assert_eq!((status, code(&body)), (StatusCode::FORBIDDEN, "org.suspended"), "{} mcp {body}", p.email);
        for (m, path) in [
            ("GET", "/api/vaults"),
            ("GET", "/api/entities/graph"),
            ("GET", "/api/documents"),
            ("GET", "/api/chat/threads"),
            ("GET", "/api/connectors"),
            ("POST", "/api/auth/tokens"),
            ("GET", "/api/org/seats"),
            ("POST", "/api/org/invitations"),
        ] {
            let (status, body) = call(&app, &p.cookie, m, path, None).await;
            assert_eq!((status, code(&body)), (StatusCode::FORBIDDEN, "org.suspended"), "{} {m} {path}: {body}", p.email);
        }
    }
    let upload = common::http(&app.router, "POST", "/api/documents?filename=a.txt", None, Some(&a_member.bearer), None).await.0;
    assert_eq!((upload.0, code(&upload.1)), (StatusCode::FORBIDDEN, "org.suspended"), "{}", upload.1);

    // still allowed: who you are, the plan status (with where to pay), your own password, your own data
    let (status, me) = call(&app, &a_member.cookie, "GET", "/api/auth/me", None).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    let plan = plan_of(&app, &a_member).await;
    assert_eq!((plan["status"].clone(), plan["manage_url"].clone(), plan["plan"].clone()), (json!("suspended"), json!(BILLING), json!("team")), "{plan}");
    assert!(!plan.to_string().contains("payment_failed"), "the reason is the operator's: {plan}");
    let (status, body) = call(&app, &a_member.cookie, "POST", "/api/account/password", Some(json!({ "current_password": common::PASSWORD, "new_password": "a-new-password-1" }))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = call(&app, &a_member.cookie, "POST", "/api/export/subject", None).await;
    assert!(status != StatusCode::FORBIDDEN || code(&body) != "org.suspended", "{status} {body}");
    let (status, body) = call(&app, &a_member.cookie, "GET", "/api/export/subject", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = call(&app, &a_member.cookie, "POST", "/api/account/erasure", Some(json!({}))).await;
    assert!(code(&body) != "org.suspended", "{status} {body}");
    // an owner (and an admin) sees the People list; a member does not (and is told why, not that it is suspended)
    let (status, people) = call(&app, &a_owner.cookie, "GET", "/api/admin/people", None).await;
    assert_eq!(status, StatusCode::OK, "{people}");
    assert_eq!(people["people"].as_array().unwrap().len(), 2);
    let (status, body) = call(&app, &a_member.cookie, "GET", "/api/admin/people", None).await;
    assert_eq!((status, code(&body)), (StatusCode::FORBIDDEN, "auth.forbidden"), "{body}");
    // signing in still works; so does signing out
    let ((status, body), cookie) = common::http(&app.router, "POST", "/api/auth/login", Some(json!({ "email": "ma@acme.test", "password": "a-new-password-1" })), None, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _) = call(&app, cookie.as_deref().unwrap().split(';').next().unwrap(), "POST", "/api/auth/logout", None).await;
    assert_eq!(status, StatusCode::OK);

    // the other organisation does not notice, and its plan is its own
    let (status, w) = memory_write(&app, &b_member, "Bea likes tea").await;
    assert_eq!(status, StatusCode::OK, "{w}");
    assert_eq!(mcp(&app, &b_member).await.0, StatusCode::OK);
    assert_eq!(plan_of(&app, &b_owner).await["status"], "active");
    assert_eq!(org_summary(&app, &b_id).await["status"], "active");
    // an owner cannot reach another organisation's plan: the status route has no organisation parameter, and
    // the operator routes are not theirs
    assert_eq!(common::http(&app.router, "GET", &format!("/api/operator/v1/orgs/{b_id}"), None, Some(&a_owner.bearer), Some(&a_owner.cookie)).await.0 .0, StatusCode::UNAUTHORIZED);
    assert_eq!(common::http(&app.router, "PATCH", &format!("/api/operator/v1/orgs/{b_id}"), Some(json!({ "seat_limit": 1 })), Some(&a_owner.bearer), None).await.0 .0, StatusCode::UNAUTHORIZED);
    assert_eq!(org_summary(&app, &b_id).await["seat_limit"], 5);

    // resumed: the same sessions and tokens work again at once
    let (status, body) = op(&app, "POST", &format!("/api/operator/v1/orgs/{a_id}/resume"), None).await;
    assert_eq!((status, body["status"].clone()), (StatusCode::OK, json!("active")), "{body}");
    assert_eq!(memory_write(&app, &a_member, "Ada likes coffee").await.0, StatusCode::OK);
    assert_eq!(mcp(&app, &a_member).await.0, StatusCode::OK);
    assert_eq!(plan_of(&app, &a_owner).await["status"], "active");
    // suspending twice keeps the first time; the ledger has every call
    for reason in ["payment_failed", "chargeback"] {
        assert_eq!(op(&app, "POST", &format!("/api/operator/v1/orgs/{a_id}/suspend"), Some(json!({ "reason": reason }))).await.0, StatusCode::OK);
    }
    let first = org_summary(&app, &a_id).await["suspended_at"].clone();
    op(&app, "POST", &format!("/api/operator/v1/orgs/{a_id}/suspend"), Some(json!({ "reason": "again" }))).await;
    assert_eq!(org_summary(&app, &a_id).await["suspended_at"], first);
    let (status, body) = op(&app, "POST", &format!("/api/operator/v1/orgs/{a_id}/suspend"), Some(json!({ "reason": "" }))).await;
    assert_eq!((status, code(&body)), (StatusCode::BAD_REQUEST, "validation.invalid"), "{body}");
    let (status, body) = op(&app, "POST", "/api/operator/v1/orgs/0123456789abcdef0123456789abcdef/suspend", Some(json!({ "reason": "x" }))).await;
    assert_eq!((status, code(&body)), (StatusCode::NOT_FOUND, "operator.org_not_found"), "{body}");
}

#[tokio::test]
async fn a_suspension_made_on_another_replica_takes_effect_within_a_few_seconds() {
    let app = host().await;
    let (id, owner) = owned_org(&app, "sus_c", "oc@acme.test", 3).await;
    assert_eq!(memory_write(&app, &owner, "Ada likes tea").await.0, StatusCode::OK);
    // another replica writes the row; this process's cache still says active
    set_org(&app, &id, "status = 'suspended', suspended_at = time::now()").await;
    let start = Instant::now();
    loop {
        let (status, body) = memory_write(&app, &owner, "Ada likes coffee").await;
        if status == StatusCode::FORBIDDEN {
            assert_eq!(code(&body), "org.suspended");
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(6), "still served {:?} after the suspension", start.elapsed());
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(start.elapsed() <= eunomia_backend::plan::TTL + Duration::from_secs(2), "{:?}", start.elapsed());
    // and the resume the same way
    set_org(&app, &id, "status = 'active', suspended_at = NONE").await;
    let start = Instant::now();
    while memory_write(&app, &owner, "Ada likes cocoa").await.0 == StatusCode::FORBIDDEN {
        assert!(start.elapsed() < Duration::from_secs(6), "not resumed after {:?}", start.elapsed());
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// A suspended organisation's members reach nothing but their own account routes: every route in the API
/// description, and `/mcp`, called as one of its members. Which routes are refused is not listed here: it is
/// the gate's class for the route, so a route added later is covered (or fails this test).
#[tokio::test]
async fn every_route_refuses_a_suspended_organisations_member_unless_it_is_an_account_route() {
    use axum::http::Method;
    use eunomia_backend::gate::{explicit_class, Class};
    let app = host().await;
    let (id, owner) = owned_org(&app, "walk_1", "w1@acme.test", 5).await;
    let member = join(&app, &owner, "wm@acme.test").await;
    assert_eq!(op(&app, "POST", &format!("/api/operator/v1/orgs/{id}/suspend"), Some(json!({ "reason": "walk" }))).await.0, StatusCode::OK);

    let spec = serde_json::to_value(eunomia_backend::openapi::spec()).unwrap();
    let mut routes: Vec<(String, String)> = vec![("POST".into(), "/mcp".into()), ("GET".into(), "/mcp".into()), ("DELETE".into(), "/mcp".into())];
    for (path, item) in spec["paths"].as_object().unwrap() {
        for m in ["get", "post", "put", "patch", "delete"] {
            if item.get(m).is_some() {
                let mut concrete = String::new();
                let mut in_param = false;
                for c in path.chars() {
                    match c {
                        '{' => { in_param = true; concrete.push('x'); }
                        '}' => in_param = false,
                        c if !in_param => concrete.push(c),
                        _ => {}
                    }
                }
                routes.push((m.to_uppercase(), concrete));
            }
        }
    }
    assert!(routes.len() > 100, "{}", routes.len());
    let (mut refused, mut skipped, mut allowed) = (0, 0, 0);
    for (m, path) in &routes {
        let method: Method = m.parse().unwrap();
        let class = if path == "/mcp" { Some(Class::Mcp) } else { explicit_class(&method, path) };
        if path.starts_with("/api") {
            assert!(class.is_some(), "{m} {path} has no class in gate::explicit_class");
        }
        match class {
            Some(Class::Mcp | Class::Any | Class::Vault(_) | Class::Account(_)) => {}
            // no credential involved, or not the member's content (login, webhooks, OAuth metadata, the operator API)
            _ => {
                skipped += 1;
                continue;
            }
        }
        if eunomia_backend::plan::allowed_while_suspended(&method, path) {
            allowed += 1;
            continue;
        }
        let ((status, body), _) = common::http(&app.router, m, path, None, Some(&member.bearer), Some(&member.cookie)).await;
        assert_eq!((status, code(&body)), (StatusCode::FORBIDDEN, "org.suspended"), "{m} {path}: {body}");
        refused += 1;
    }
    assert!(refused > 100, "refused {refused}, allowed {allowed}, skipped {skipped}");
    assert!((5..=12).contains(&allowed), "the allow-list is small: {allowed}");
}

#[tokio::test]
async fn background_work_for_a_suspended_organisation_pauses_and_picks_up_again() {
    let app = host().await;
    let (a_id, a_owner) = owned_org(&app, "job_a", "ja@acme.test", 3).await;
    let (_, b_owner) = owned_org(&app, "job_b", "jb@acme.test", 3).await;
    let user = |p: &P| {
        let control = app.state.control.clone();
        let email = p.email.clone();
        async move { models_user::user_by_email(&control, &email).await.unwrap().unwrap().id }
    };
    let (a, b) = (user(&a_owner).await, user(&b_owner).await);
    let lease = Duration::from_secs(30);
    let claim_kinds = || async {
        let mut kinds: Vec<String> = jobs::claim(&app.state.control, "w1", 10, lease, 100).await.unwrap().into_iter().map(|j| j.kind).collect();
        kinds.sort();
        kinds
    };
    assert_eq!(op(&app, "POST", &format!("/api/operator/v1/orgs/{a_id}/suspend"), Some(json!({ "reason": "jobs" }))).await.0, StatusCode::OK);
    for (kind, owner, key) in [("sync", &a, "a-sync"), ("embed", &a, "a-embed"), ("sync", &b, "b-sync"), ("subject_export", &a, "a-export"), ("migrate_tenant", &a, "a-migrate")] {
        assert!(jobs::enqueue(&app.state.control, NewJob::new(kind, owner.clone(), key)).await.unwrap());
    }
    // what costs model spend waits; B's work and the work that is a person's own right or a schema migration run
    assert_eq!(claim_kinds().await, vec!["migrate_tenant", "subject_export", "sync"]);
    assert!(claim_kinds().await.is_empty(), "the paused jobs stay queued, not claimed");
    let queued: Vec<Value> = app.control().test_raw().query("SELECT kind, status, attempts FROM job WHERE owner = $o ORDER BY kind").bind(("o", a.clone())).await.unwrap().take(0).unwrap();
    let waiting: Vec<&Value> = queued.iter().filter(|j| j["status"] == "ready").collect();
    assert_eq!(waiting.len(), 2, "{queued:?}");
    assert!(waiting.iter().all(|j| j["attempts"] == 0), "{queued:?}");
    // resumed: the queue picks up exactly where it was
    assert_eq!(op(&app, "POST", &format!("/api/operator/v1/orgs/{a_id}/resume"), None).await.0, StatusCode::OK);
    assert_eq!(claim_kinds().await, vec!["embed", "sync"]);
}

#[tokio::test]
async fn a_running_job_is_handed_back_when_its_organisation_is_suspended() {
    let app = host().await;
    let (a_id, a_owner) = owned_org(&app, "job_c", "jc@acme.test", 3).await;
    let a = models_user::user_by_email(&app.state.control, &a_owner.email).await.unwrap().unwrap().id;
    let (started, finished) = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicU32::new(0)));
    let (s, f) = (started.clone(), finished.clone());
    let registry = Registry::new().register("probe", move |_state, _job| {
        let (s, f) = (s.clone(), f.clone());
        async move {
            s.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(1500)).await;
            f.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    });
    let cfg = WorkerConfig {
        id: "w-pause".into(),
        concurrency: 2,
        poll: Duration::from_millis(20),
        lease: Duration::from_millis(600),
        owner_cap: 100,
        grace: Duration::from_millis(200),
        retry_base: Duration::from_millis(10),
        tick: Duration::from_secs(30),
    };
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let worker = tokio::spawn(worker::run(app.state.clone(), registry, cfg, stopped));
    jobs::enqueue(&app.state.control, NewJob::new("probe", a.clone(), "probe-1")).await.unwrap();
    let wait = |what: &'static str, mut f: Box<dyn FnMut() -> bool + Send>| async move {
        let start = Instant::now();
        while !f() {
            assert!(start.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    let st = started.clone();
    wait("the job to start", Box::new(move || st.load(Ordering::SeqCst) == 1)).await;
    // suspended while it runs: it is cancelled and handed back without burning an attempt
    assert_eq!(op(&app, "POST", &format!("/api/operator/v1/orgs/{a_id}/suspend"), Some(json!({ "reason": "mid-run" }))).await.0, StatusCode::OK);
    let start = Instant::now();
    let row = loop {
        let rows: Vec<Value> = app.control().test_raw().query("SELECT status, attempts, last_error_code FROM job WHERE kind = 'probe'").await.unwrap().take(0).unwrap();
        if rows[0]["status"] == "ready" {
            break rows[0].clone();
        }
        assert!(start.elapsed() < Duration::from_secs(8), "never handed back: {rows:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(row["attempts"], 0, "{row}");
    assert!(row["last_error_code"].is_null(), "not a failure: {row}");
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!((started.load(Ordering::SeqCst), finished.load(Ordering::SeqCst)), (1, 0), "nothing runs while it is suspended");
    // resumed: it runs again from the start and finishes
    assert_eq!(op(&app, "POST", &format!("/api/operator/v1/orgs/{a_id}/resume"), None).await.0, StatusCode::OK);
    let fin = finished.clone();
    wait("the job to finish", Box::new(move || fin.load(Ordering::SeqCst) == 1)).await;
    let rows: Vec<Value> = app.control().test_raw().query("SELECT status FROM job WHERE kind = 'probe'").await.unwrap().take(0).unwrap();
    let start = Instant::now();
    if rows[0]["status"] != "done" {
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    let rows: Vec<Value> = app.control().test_raw().query("SELECT status FROM job WHERE kind = 'probe'").await.unwrap().take(0).unwrap();
    assert_eq!(rows[0]["status"], "done", "{rows:?} after {:?}", start.elapsed());
    stop.send(true).unwrap();
    let _ = worker.await;
}

#[test]
fn the_kinds_that_never_pause_are_the_same_in_the_claim_and_in_code() {
    let sql = include_str!("../src/store/jobs.rs");
    let literal = format!("[{}]", eunomia_backend::jobs::PAUSE_EXEMPT.iter().map(|k| format!("'{k}'")).collect::<Vec<_>>().join(", "));
    assert!(sql.matches(&literal).count() >= 2, "store/jobs.rs CLAIM must name exactly {literal}");
}

// ------------------------------------------------------------------ the token's boundaries

#[tokio::test]
async fn the_operator_token_opens_only_the_operator_routes_and_nothing_else_opens_them() {
    let app = host().await;
    let (id, owner) = owned_org(&app, "bound_1", "b1@acme.test", 3).await;
    let tester = person(&app, "tester@example.com", None).await;
    // the token on every kind of member route: not a credential there
    for (m, path) in [
        ("GET", "/api/auth/me"),
        ("GET", "/api/org/plan"),
        ("GET", "/api/vaults"),
        ("GET", "/api/admin/people"),
        ("POST", "/api/tools/recall"),
        ("POST", "/mcp"),
        ("GET", "/api/debug/metrics"),
    ] {
        let (status, body) = common::http(&app.router, m, path, None, Some(TOKEN), None).await.0;
        assert_eq!((status, code(&body)), (StatusCode::UNAUTHORIZED, "auth.unauthorized"), "{m} {path}: {body}");
    }
    // and no member credential on an operator route: a session, an API token (all scopes), the server operator's own
    for p in [&owner, &tester] {
        for (m, path) in [("GET", "/api/operator/v1/capacity".to_string()), ("GET", "/api/operator/v1/orgs".to_string()), ("GET", format!("/api/operator/v1/orgs/{id}")), ("POST", format!("/api/operator/v1/orgs/{id}/suspend"))] {
            for (bearer, cookie) in [(Some(p.bearer.as_str()), None), (None, Some(p.cookie.as_str())), (Some(p.bearer.as_str()), Some(p.cookie.as_str()))] {
                let (status, body) = common::http(&app.router, m, &path, Some(json!({ "reason": "x" })), bearer, cookie).await.0;
                assert_eq!((status, code(&body)), (StatusCode::UNAUTHORIZED, "auth.unauthorized"), "{} {m} {path}: {body}", p.email);
            }
        }
    }
    // an OAuth-looking token and a near miss of the real one are refused the same way
    for wrong in ["eun_oat_abcdefabcdefabcdefabcdefabcdefabcdef", &TOKEN[..TOKEN.len() - 1], &format!("{TOKEN}x"), ""] {
        let (status, body) = common::http(&app.router, "GET", "/api/operator/v1/capacity", None, Some(wrong), None).await.0;
        assert_eq!((status, code(&body)), (StatusCode::UNAUTHORIZED, "auth.unauthorized"), "{wrong:?}: {body}");
    }
    assert_eq!(common::http(&app.router, "GET", "/api/operator/v1/capacity", None, None, None).await.0 .0, StatusCode::UNAUTHORIZED);
    assert_eq!(org_summary(&app, &id).await["status"], "active", "nothing was suspended by the refused calls");
    // lower-case "bearer" is still the scheme; the right token works
    assert_eq!(op(&app, "GET", "/api/operator/v1/capacity", None).await.0, StatusCode::OK);
}

#[tokio::test]
async fn every_operator_call_is_on_the_instance_ledger_and_the_token_is_nowhere() {
    let app = host().await;
    let made = create_org(&app, "led_1", "l1@acme.test", 3).await;
    let id = made["organisation_id"].as_str().unwrap().to_string();
    op(&app, "GET", "/api/operator/v1/orgs", None).await;
    op(&app, "GET", &format!("/api/operator/v1/orgs/{id}"), None).await;
    op(&app, "PATCH", &format!("/api/operator/v1/orgs/{id}"), Some(json!({ "plan": "enterprise" }))).await;
    op(&app, "POST", &format!("/api/operator/v1/orgs/{id}/suspend"), Some(json!({ "reason": "r" }))).await;
    op(&app, "POST", &format!("/api/operator/v1/orgs/{id}/resume"), None).await;
    op(&app, "POST", &format!("/api/operator/v1/orgs/{id}/owner-claim/resend"), None).await;
    op(&app, "GET", "/api/operator/v1/capacity", None).await;
    // a refused call is on it too, with its code
    op(&app, "POST", "/api/operator/v1/orgs", Some(json!({ "name": "X", "owner_email": "op@corp.test", "plan": "team", "seat_limit": 1, "external_ref": "led_2" }))).await;
    // a wrong token is a failed sign-in, with a hash of what was presented
    common::http(&app.router, "GET", "/api/operator/v1/capacity", None, Some("a-wrong-token-that-someone-guessed-0123456789"), None).await;
    for action in [
        "operator.org_create", "operator.org_list", "operator.org_get", "operator.org_update", "operator.org_suspend", "operator.org_resume", "operator.owner_claim_resend", "operator.capacity",
    ] {
        let rows = ledger(&app, action).await;
        assert!(!rows.is_empty(), "no {action} row");
        for r in &rows {
            assert_eq!((r["actor_kind"].clone(), r["actor_id"].clone(), r["user"].clone()), (json!("operator"), json!("operator-api"), json!("")), "{r}");
        }
    }
    let creates = ledger(&app, "operator.org_create").await;
    assert_eq!(creates.len(), 2);
    assert_eq!((creates[0]["outcome"].clone(), creates[0]["target"].clone()), (json!("ok"), json!("ref:led_1")), "{:?}", creates[0]);
    assert_eq!(creates[1]["outcome"], "operator.owner_email_reserved", "{:?}", creates[1]);
    assert!(creates[0]["detail"].as_str().unwrap().contains("plan=team seats=3"), "{:?}", creates[0]);
    let suspends = ledger(&app, "operator.org_suspend").await;
    assert_eq!((suspends[0]["target"].clone(), suspends[0]["detail"].clone()), (json!(format!("org:{id}")), json!("reason=r")), "{:?}", suspends[0]);
    // a call that never reaches a handler (no body, an unknown path, a method the route does not have) is on it too
    let before = ledger(&app, "operator.call").await.len();
    assert_eq!(before, 0, "a call a handler answered has no second row");
    let (s1, _) = op(&app, "POST", "/api/operator/v1/orgs", None).await;
    let (s2, _) = op(&app, "GET", "/api/operator/v1/nope", None).await;
    let (s3, _) = op(&app, "DELETE", "/api/operator/v1/orgs", None).await;
    assert!(s1.is_client_error() && s2 == StatusCode::NOT_FOUND && s3 == StatusCode::METHOD_NOT_ALLOWED, "{s1} {s2} {s3}");
    let calls = ledger(&app, "operator.call").await;
    assert_eq!(calls.len(), 3, "{calls:?}");
    assert_eq!(calls[1]["target"], "GET /api/operator/v1/nope", "{calls:?}");
    assert_eq!(calls[1]["outcome"], "http.404", "{calls:?}");
    assert!(calls.iter().all(|c| c["actor_kind"] == "operator" && c["actor_id"] == "operator-api"), "{calls:?}");
    common::assert_chained(app.control(), &["operator.org_create", "operator.org_suspend", "operator.org_resume", "operator.call"]).await;
    // nothing in the control database holds the operator token, or a claim token
    let dump: Vec<Value> = app.control().test_raw().query("SELECT * FROM audit_event").await.unwrap().take(0).unwrap();
    let dump = serde_json::to_string(&dump).unwrap();
    assert!(!dump.contains(TOKEN) && !dump.contains("a-wrong-token-that-someone-guessed"), "a credential is in the audit rows");
    assert!(!dump.contains(&token_of(made["claim_link"].as_str().unwrap())), "a claim token is in the audit rows");
    assert!(dump.contains("auth.failed"), "the wrong token left a failed sign-in row");
    let claims: Vec<Value> = app.control().test_raw().query("SELECT * FROM org_invitation").await.unwrap().take(0).unwrap();
    assert!(!serde_json::to_string(&claims).unwrap().contains(&token_of(made["claim_link"].as_str().unwrap())), "only the hash is stored");
}

#[tokio::test]
async fn the_operator_api_has_its_own_rate_limit() {
    host_env();
    let app = host().await;
    let router = eunomia_backend::app_with(app.state.clone(), RateConfig { operator_per_min: 3, auth_per_min: 1000, ..RateConfig::default() });
    let mut statuses = Vec::new();
    for _ in 0..5 {
        statuses.push(common::http(&router, "GET", "/api/operator/v1/capacity", None, Some(TOKEN), None).await.0 .0);
    }
    assert_eq!(statuses, vec![StatusCode::OK, StatusCode::OK, StatusCode::OK, StatusCode::TOO_MANY_REQUESTS, StatusCode::TOO_MANY_REQUESTS]);
    // members are not affected by the billing service's calls
    let me = person(&app, "tester@example.com", None).await;
    assert_eq!(common::http(&router, "GET", "/api/auth/me", None, None, Some(&me.cookie)).await.0 .0, StatusCode::OK);
}

#[tokio::test]
async fn the_operator_api_never_reads_an_organisations_own_database() {
    let app = host().await;
    let (id, owner) = owned_org(&app, "canary_1", "k1@acme.test", 3).await;
    let canary = "CANARY-memory-text-7c1f9e";
    let (status, w) = memory_write(&app, &owner, canary).await;
    assert_eq!(status, StatusCode::OK, "{w}");
    // the content is really there (the owner reads it) ...
    let (status, body) = common::http(&app.router, "POST", "/api/tools/recall", Some(json!({ "query": "Ada" })), Some(&owner.bearer), None).await.0;
    assert_eq!(status, StatusCode::OK);
    assert!(body.to_string().contains(canary), "{body}");
    // ... and nothing the operator API says contains it
    let mut seen = vec![
        op(&app, "GET", "/api/operator/v1/orgs", None).await.1,
        op(&app, "GET", &format!("/api/operator/v1/orgs/{id}"), None).await.1,
        op(&app, "GET", "/api/operator/v1/capacity", None).await.1,
        op(&app, "PATCH", &format!("/api/operator/v1/orgs/{id}"), Some(json!({ "name": "Canary Co" }))).await.1,
        op(&app, "POST", &format!("/api/operator/v1/orgs/{id}/suspend"), Some(json!({ "reason": "x" }))).await.1,
        op(&app, "POST", &format!("/api/operator/v1/orgs/{id}/resume"), None).await.1,
    ];
    seen.push(create_org(&app, "canary_1", "k1@acme.test", 3).await);
    assert!(!serde_json::to_string(&seen).unwrap().contains(canary));
    // by construction too: the operator module opens no organisation database but to ask whether it is ready
    let source = include_str!("../src/operator.rs");
    assert_eq!(source.matches("for_org(").count(), 1, "operator.rs may only ask the pool whether provisioning finished");
    for forbidden in ["state.org(", "OrgDb", "OrgState", "Stmt<Tenant>", "store::entities", "store::vaults", "store::documents", "store::cache"] {
        assert!(!source.contains(forbidden), "operator.rs touches tenant data through {forbidden}");
    }
}

// ------------------------------------------------------------------ a self-hosted install is unchanged

#[tokio::test]
async fn an_install_with_none_of_the_settings_behaves_as_it_always_did() {
    let app = host_with(HostedConfig::default()).await;
    let me = person(&app, "tester@example.com", None).await;
    // a member of an unmanaged organisation: writes, reads, no limits, nothing suspended
    assert_eq!(memory_write(&app, &me, "Ada likes tea").await.0, StatusCode::OK);
    assert_eq!(mcp(&app, &me).await.0, StatusCode::OK);
    let plan = plan_of(&app, &me).await;
    // every plan field is null; the `ai` block (model use) says AI is on with no allowance and no key of its own
    assert!(plan.as_object().unwrap().iter().all(|(k, v)| k == "ai" || v.is_null()), "{plan}");
    assert_eq!(plan["ai"], json!({ "enabled": true, "own_key": false, "usage": { "state": null, "percent": null } }), "{plan}");
    // an organisation invitation still makes an admin or member, with its old statements
    let joined = join(&app, &me, "friend@example.com").await;
    assert_eq!(memory_write(&app, &joined, "Bea likes tea").await.0, StatusCode::OK);
    // the org record carries no plan fields
    let rows: Vec<Value> = app.control().test_raw().query("SELECT plan, seat_limit, status, suspended_at, trial_ends_at, external_ref, seat_guard FROM org").await.unwrap().take(0).unwrap();
    assert!(rows.iter().all(|r| r.as_object().is_none_or(|o| o.values().all(Value::is_null))), "{rows:?}");
}

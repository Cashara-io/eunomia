//! An organisation's owner and admins run their own organisation, and nothing outside it
//! (`org_admin.rs`, `routers/org_admin.rs`, docs/deployment.md "Who may administer what").
//!
//! The whole binary is a shared host (`EUNOMIA_SIGNUP_ORG=personal`, `SIGNUP=closed`), like
//! `org_invitations.rs`. The world: the install's first account (`tester`, the operator, in an org of its own)
//! and two customer organisations. Alpha's has an owner (`alpha`), an admin (`ann`) and two members (`mia`,
//! `max`); Beta's has an owner (`beta`), an admin (`bob`) and a member (`bea`). Every person has a browser
//! session. The cross-organisation canaries live in `isolation.rs`; here are the rules.

mod common;

use axum::http::StatusCode;
use eunomia_backend::mfa;
use eunomia_backend::models_user;
use eunomia_backend::rid::RecordIdExt;
use serde_json::{json, Value};

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

#[derive(Clone)]
struct P {
    email: String,
    /// `user:...`
    id: String,
    cookie: String,
}

struct World {
    app: common::TestApp,
    operator: P,
    alpha: P,
    ann: P,
    mia: P,
    max: P,
    beta: P,
    bob: P,
    bea: P,
}

async fn login(app: &common::TestApp, email: &str) -> String {
    let ((status, body), cookie) = common::http(&app.router, "POST", "/api/auth/login", Some(json!({ "email": email, "password": common::PASSWORD })), None, None).await;
    assert_eq!(status, StatusCode::OK, "{email}: {body}");
    cookie.expect("a session cookie").split(';').next().unwrap().to_string()
}

async fn id_of(app: &common::TestApp, email: &str) -> String {
    models_user::user_by_email(&app.state.control, email).await.unwrap().unwrap().id.to_string()
}

async fn call(app: &common::TestApp, cookie: &str, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    common::http(&app.router, method, path, body, None, Some(cookie)).await.0
}

async fn bearer(app: &common::TestApp, token: &str, method: &str, path: &str) -> (StatusCode, Value) {
    common::http(&app.router, method, path, None, Some(token), None).await.0
}

/// The owner invites `email` with `role` and the invited person signs up through the link.
async fn join(app: &common::TestApp, owner: &P, email: &str, role: &str) -> P {
    let (status, created) = call(app, &owner.cookie, "POST", "/api/org/invitations", Some(json!({ "email": email, "role": role }))).await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let token = created["token"].as_str().unwrap().to_string();
    let ((status, body), cookie) = common::http(&app.router, "POST", "/api/auth/register", Some(json!({ "email": email, "password": common::PASSWORD, "invitation": token })), None, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["org_role"], role, "{body}");
    P { email: email.into(), id: id_of(app, email).await, cookie: cookie.unwrap().split(';').next().unwrap().to_string() }
}

async fn world() -> World {
    host_env();
    let app = common::TestApp::new().await;
    let operator = P { email: app.user.email.clone(), id: app.user.id.to_string(), cookie: app.session_cookie().await };
    let mut owners = Vec::new();
    for email in ["alpha@example.com", "beta@example.com"] {
        common::register_personal(&app.state, email).await;
        owners.push(P { email: email.into(), id: id_of(&app, email).await, cookie: login(&app, email).await });
    }
    let (alpha, beta) = (owners[0].clone(), owners[1].clone());
    let ann = join(&app, &alpha, "ann@example.com", "admin").await;
    let mia = join(&app, &alpha, "mia@example.com", "member").await;
    let max = join(&app, &alpha, "max@example.com", "member").await;
    let bob = join(&app, &beta, "bob@example.com", "admin").await;
    let bea = join(&app, &beta, "bea@example.com", "member").await;
    World { app, operator, alpha, ann, mia, max, beta, bob, bea }
}

async fn people(app: &common::TestApp, p: &P) -> Value {
    let (status, body) = call(app, &p.cookie, "GET", "/api/admin/people", None).await;
    assert_eq!(status, StatusCode::OK, "{}: {body}", p.email);
    body
}

fn row<'a>(people: &'a Value, email: &str) -> &'a Value {
    people["people"].as_array().unwrap().iter().find(|r| r["email"] == email).unwrap_or_else(|| panic!("{email} is not listed: {people}"))
}

async fn events(app: &common::TestApp, action: &str) -> Vec<Value> {
    let mut res = app
        .control()
        .test_raw()
        .query("SELECT user, actor_kind, actor_id, action, target, outcome, detail FROM audit_event WHERE action = $a ORDER BY created_at")
        .bind(("a", action.to_string()))
        .await
        .unwrap();
    res.take(0).unwrap()
}

/// The routes that act on one member, with the harmless body each takes.
fn member_routes(id: &str) -> Vec<(&'static str, String, Option<Value>)> {
    vec![
        ("PUT", format!("/api/admin/people/{id}/suspended"), Some(json!({ "suspended": true }))),
        ("PUT", format!("/api/admin/people/{id}/suspended"), Some(json!({ "suspended": false }))),
        ("PUT", format!("/api/admin/people/{id}/role"), Some(json!({ "role": "admin" }))),
        ("PUT", format!("/api/admin/people/{id}/role"), Some(json!({ "role": "member" }))),
        ("DELETE", format!("/api/admin/people/{id}/sessions"), None),
        ("POST", format!("/api/admin/people/{id}/revoke-access"), None),
        ("GET", format!("/api/admin/people/{id}/tokens"), None),
        ("DELETE", format!("/api/admin/people/{id}/tokens/api_token:x"), None),
        ("GET", format!("/api/admin/people/{id}/apps"), None),
        ("DELETE", format!("/api/admin/people/{id}/apps/oauth_grant:x"), None),
        ("PUT", format!("/api/admin/people/{id}/admin"), Some(json!({ "admin": true }))),
    ]
}

/// A response without what differs per request.
fn plain(mut v: Value) -> Value {
    if let Some(o) = v.as_object_mut() {
        o.remove("trace_id");
    }
    v
}

fn code(v: &Value) -> Option<&str> {
    v["code"].as_str()
}

#[tokio::test]
async fn an_owner_and_an_admin_see_their_own_members_with_role_status_mfa_and_activity() {
    let w = world().await;
    for viewer in [&w.alpha, &w.ann] {
        let list = people(&w.app, viewer).await;
        let emails: Vec<&str> = list["people"].as_array().unwrap().iter().map(|r| r["email"].as_str().unwrap()).collect();
        assert_eq!(emails.len(), 4, "{viewer_email}: {emails:?}", viewer_email = viewer.email);
        for e in ["alpha@example.com", "ann@example.com", "mia@example.com", "max@example.com"] {
            assert!(emails.contains(&e), "{e} in {emails:?}");
        }
        assert!(!list.to_string().contains("beta@example.com") && !list.to_string().contains("tester@example.com"), "{list}");
        let mia = row(&list, "mia@example.com");
        assert_eq!((mia["org_role"].as_str(), mia["mfa"].as_str(), mia["suspended_at"].is_null()), (Some("member"), Some("off"), true), "{mia}");
        assert!(mia["last_active"].is_string(), "she signed in when she joined: {mia}");
        assert_eq!(row(&list, "ann@example.com")["org_role"], "admin");
        assert_eq!(row(&list, "alpha@example.com")["org_role"], "owner");
    }
    // what each may do to each row, as the buttons see it
    let by_owner = people(&w.app, &w.alpha).await;
    let by_admin = people(&w.app, &w.ann).await;
    for (list, email, suspend, role) in [
        (&by_owner, "mia@example.com", true, true),
        (&by_owner, "ann@example.com", true, true),
        (&by_owner, "alpha@example.com", false, false),
        (&by_admin, "mia@example.com", true, false),
        (&by_admin, "ann@example.com", false, false),
        (&by_admin, "alpha@example.com", false, false),
    ] {
        let r = row(list, email);
        assert_eq!((r["can_suspend"].as_bool(), r["can_set_role"].as_bool()), (Some(suspend), Some(role)), "{email}: {r}");
    }
    // the org's name and policy
    let (status, org) = call(&w.app, &w.ann.cookie, "GET", "/api/admin/org", None).await;
    assert_eq!(status, StatusCode::OK, "{org}");
    assert_eq!((org["your_role"].as_str(), org["can_rename"].as_bool(), org["require_mfa"].as_bool()), (Some("admin"), Some(false), Some(false)), "{org}");
    assert_eq!(org["name"], "alpha@example.com", "named after its founder until renamed");
}

#[tokio::test]
async fn a_plain_member_is_refused_everything() {
    let w = world().await;
    let mut calls: Vec<(&str, String, Option<Value>)> = vec![
        ("GET", "/api/admin/people".into(), None),
        ("GET", "/api/admin/audit".into(), None),
        ("GET", "/api/admin/audit/verify".into(), None),
        ("GET", "/api/admin/audit/export".into(), None),
        ("GET", "/api/admin/access-reviews".into(), None),
        ("POST", "/api/admin/access-reviews".into(), None),
        ("GET", "/api/admin/org".into(), None),
        ("PUT", "/api/admin/org/name".into(), Some(json!({ "name": "Mine now" }))),
        ("PUT", "/api/admin/org/security".into(), Some(json!({ "require_mfa": true }))),
        ("GET", "/api/org/invitations".into(), None),
        ("GET", "/api/org/seats".into(), None),
        ("GET", "/api/admin/security".into(), None),
    ];
    // acting on a member of their own org, an admin, the owner, themselves, and someone who does not exist
    for target in [&w.max.id, &w.ann.id, &w.alpha.id, &w.mia.id, &"user:nobody".to_string()] {
        calls.extend(member_routes(target));
    }
    for (method, path, body) in calls {
        let (status, body) = call(&w.app, &w.mia.cookie, method, &path, body).await;
        assert_eq!((status, code(&body)), (StatusCode::FORBIDDEN, Some("auth.forbidden")), "{method} {path}: {body}");
    }
    // nothing changed
    let list = people(&w.app, &w.alpha).await;
    assert!(list["people"].as_array().unwrap().iter().all(|r| r["suspended_at"].is_null()));
    assert_eq!(row(&list, "max@example.com")["org_role"], "member");
    assert_eq!(call(&w.app, &w.alpha.cookie, "GET", "/api/admin/org", None).await.1["name"], "alpha@example.com");
}

#[tokio::test]
async fn an_owner_and_an_admin_of_one_org_cannot_reach_another_orgs_members_and_cannot_tell_if_they_exist() {
    let w = world().await;
    let ghost = "user:thereisnoonehere".to_string();
    for attacker in [&w.alpha, &w.ann] {
        for victim in [&w.beta, &w.bob, &w.bea, &w.operator] {
            for ((method, path, body), (_, ghost_path, _)) in member_routes(&victim.id).into_iter().zip(member_routes(&ghost)) {
                let (status, answer) = call(&w.app, &attacker.cookie, method, &path, body.clone()).await;
                let (g_status, g_answer) = call(&w.app, &attacker.cookie, method, &ghost_path, body).await;
                assert_eq!((status, plain(answer.clone())), (g_status, plain(g_answer.clone())), "{method} {path}: a real account elsewhere must answer exactly like one that does not exist");
                // the instance-admin flag route is for instance admins: a refusal, not a "not found"
                let expected = if path.ends_with("/admin") { (StatusCode::FORBIDDEN, "auth.forbidden") } else { (StatusCode::NOT_FOUND, "auth.not_found") };
                assert_eq!((status, code(&answer)), (expected.0, Some(expected.1)), "{} -> {method} {path}: {answer}", attacker.email);
                assert!(!answer.to_string().contains(&victim.email) && !answer.to_string().contains(&victim.id), "{answer}");
            }
        }
    }
    // Beta is exactly as it was: nobody suspended, roles kept, sessions alive, name and policy unchanged
    let list = people(&w.app, &w.beta).await;
    assert!(list["people"].as_array().unwrap().iter().all(|r| r["suspended_at"].is_null()), "{list}");
    assert_eq!((row(&list, "bob@example.com")["org_role"].as_str(), row(&list, "bea@example.com")["org_role"].as_str()), (Some("admin"), Some("member")));
    for p in [&w.beta, &w.bob, &w.bea, &w.operator] {
        assert_eq!(call(&w.app, &p.cookie, "GET", "/api/auth/me", None).await.0, StatusCode::OK, "{} is still signed in", p.email);
    }
    let (_, org) = call(&w.app, &w.beta.cookie, "GET", "/api/admin/org", None).await;
    assert_eq!((org["name"].as_str(), org["require_mfa"].as_bool()), (Some("beta@example.com"), Some(false)), "{org}");
}

#[tokio::test]
async fn another_orgs_audit_log_policy_and_name_are_not_reachable() {
    let w = world().await;
    // Beta does things worth logging
    let (status, _) = call(&w.app, &w.beta.cookie, "PUT", "/api/admin/org/name", Some(json!({ "name": "Beta Holdings" }))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(call(&w.app, &w.beta.cookie, "PUT", &format!("/api/admin/people/{}/suspended", w.bea.id), Some(json!({ "suspended": true }))).await.0, StatusCode::OK);
    // an unknown sign-in belongs to no organisation: for the operators only
    let _ = common::http(&w.app.router, "POST", "/api/auth/login", Some(json!({ "email": "ghost@nowhere.test", "password": "nope-nope-nope" })), None, None).await;

    for attacker in [&w.alpha, &w.ann] {
        // the log: only Alpha's own events
        let (status, page) = call(&w.app, &attacker.cookie, "GET", "/api/admin/audit?category=all&limit=200", None).await;
        assert_eq!(status, StatusCode::OK, "{page}");
        let text = page.to_string();
        for beta_only in ["beta@example.com", "bob@example.com", "bea@example.com", "Beta Holdings", "ghost@nowhere.test"] {
            assert!(!text.contains(beta_only), "{} reads {beta_only} in {text}", attacker.email);
        }
        assert!(text.contains("alpha@example.com"), "its own events are there: {text}");
        // verify and export: its own ledger only, and not the instance's
        let (status, verify) = call(&w.app, &attacker.cookie, "GET", "/api/admin/audit/verify", None).await;
        assert_eq!(status, StatusCode::OK, "{verify}");
        assert_eq!(verify["ledgers"].as_array().unwrap().len(), 1, "no instance ledger: {verify}");
        let (status, export) = call(&w.app, &attacker.cookie, "GET", "/api/admin/audit/export", None).await;
        assert_eq!(status, StatusCode::OK, "{export}");
        let beta_org = eunomia_backend::models_user::user_by_email(&w.app.state.control, "beta@example.com").await.unwrap().unwrap().org.key();
        assert!(!export.to_string().contains(&beta_org) && !export.to_string().contains(&w.beta.id), "{export}");
        let (status, refused) = call(&w.app, &attacker.cookie, "GET", "/api/admin/audit/export?ledger=instance", None).await;
        assert_eq!((status, code(&refused)), (StatusCode::FORBIDDEN, Some("auth.forbidden")), "{refused}");
        // the policy and the name are the caller's own, wherever they are asked from
        let (_, org) = call(&w.app, &attacker.cookie, "GET", "/api/admin/org", None).await;
        assert_eq!(org["name"], "alpha@example.com");
    }
    // the operator, who sees the whole host, does see the unknown sign-in
    let (_, page) = call(&w.app, &w.operator.cookie, "GET", "/api/admin/audit?category=all&limit=200", None).await;
    assert!(page.to_string().contains("ghost@nowhere.test"), "{page}");

    // Alpha turns its own policy on and renames: Beta's name and policy are untouched
    let (status, _) = call(&w.app, &w.alpha.cookie, "PUT", "/api/admin/org/name", Some(json!({ "name": "Alpha Pty" }))).await;
    assert_eq!(status, StatusCode::OK);
    let (_, org) = call(&w.app, &w.beta.cookie, "GET", "/api/admin/org", None).await;
    assert_eq!((org["name"].as_str(), org["require_mfa"].as_bool()), (Some("Beta Holdings"), Some(false)), "{org}");
    // Alpha's access reviews list holds none of Beta's
    let (status, started) = call(&w.app, &w.alpha.cookie, "POST", "/api/admin/access-reviews", None).await;
    assert_eq!(status, StatusCode::OK, "{started}");
    assert!(!started.to_string().contains("beta@example.com") && !started.to_string().contains("bob@example.com"), "{started}");
    let (_, theirs) = call(&w.app, &w.beta.cookie, "GET", "/api/admin/access-reviews", None).await;
    assert_eq!(theirs, json!([]), "Beta never sees Alpha's review");
}

#[tokio::test]
async fn an_admin_cannot_grant_admin_and_only_the_owner_changes_roles() {
    let w = world().await;
    let role = |id: &str| format!("/api/admin/people/{id}/role");
    // an admin cannot make a member an admin, or take admin from one
    let (status, body) = call(&w.app, &w.ann.cookie, "PUT", &role(&w.mia.id), Some(json!({ "role": "admin" }))).await;
    assert_eq!((status, code(&body)), (StatusCode::FORBIDDEN, Some("auth.forbidden")), "{body}");
    // the owner may; the list follows
    let (status, list) = call(&w.app, &w.alpha.cookie, "PUT", &role(&w.mia.id), Some(json!({ "role": "admin" }))).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(row(&list, "mia@example.com")["org_role"], "admin");
    // an admin cannot take admin from another admin
    let (status, body) = call(&w.app, &w.ann.cookie, "PUT", &role(&w.mia.id), Some(json!({ "role": "member" }))).await;
    assert_eq!((status, code(&body)), (StatusCode::FORBIDDEN, Some("auth.forbidden")), "{body}");
    // a role other than member or admin is not accepted; an owner is never made this way
    for bad in ["owner", "root", ""] {
        let (status, _) = call(&w.app, &w.alpha.cookie, "PUT", &role(&w.max.id), Some(json!({ "role": bad }))).await;
        assert!(status.is_client_error(), "{bad} -> {status}");
    }
    assert_eq!(row(&people(&w.app, &w.alpha).await, "max@example.com")["org_role"], "member");
    // the owner can demote her again; the new admin's powers go at once
    assert_eq!(call(&w.app, &w.mia.cookie, "GET", "/api/admin/people", None).await.0, StatusCode::OK, "an admin now");
    let (status, list) = call(&w.app, &w.alpha.cookie, "PUT", &role(&w.mia.id), Some(json!({ "role": "member" }))).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let (status, body) = call(&w.app, &w.mia.cookie, "GET", "/api/admin/people", None).await;
    assert_eq!((status, code(&body)), (StatusCode::FORBIDDEN, Some("auth.forbidden")), "demoted, no powers left: {body}");
    // the same role again changes nothing and is not an error
    assert_eq!(call(&w.app, &w.alpha.cookie, "PUT", &role(&w.mia.id), Some(json!({ "role": "member" }))).await.0, StatusCode::OK);
    // invitations follow the same rule (an admin invites members only)
    let (status, body) = call(&w.app, &w.ann.cookie, "POST", "/api/org/invitations", Some(json!({ "email": "new@example.com", "role": "admin" }))).await;
    assert_eq!((status, code(&body)), (StatusCode::FORBIDDEN, Some("auth.forbidden")), "{body}");
    // and the role change is audited with the actor
    let rows = events(&w.app, "org.role").await;
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(rows.iter().all(|r| r["user"].to_string().contains(w.alpha.id.trim_start_matches("user:")) && r["actor_kind"] == "user"), "{rows:?}");
}

#[tokio::test]
async fn the_owner_cannot_be_demoted_suspended_signed_out_or_removed() {
    let w = world().await;
    let owner = &w.alpha.id;
    // an admin: refused, as protected, for every act
    for (method, path, body) in member_routes(owner).into_iter().filter(|(_, p, _)| !p.ends_with("/admin") && !p.contains("/tokens/") && !p.contains("/apps/")) {
        let (status, answer) = call(&w.app, &w.ann.cookie, method, &path, body).await;
        assert_eq!((status, code(&answer)), (StatusCode::FORBIDDEN, Some("org.protected_member")), "{method} {path}: {answer}");
    }
    // the owner on herself: not here (their own Settings do that)
    for (method, path, body) in member_routes(owner).into_iter().filter(|(_, p, _)| p.contains("/suspended") || p.contains("/role") || p.contains("/revoke-access") || p.contains("/sessions")) {
        let (status, answer) = call(&w.app, &w.alpha.cookie, method, &path, body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{method} {path}: {answer}");
    }
    // another member's attempt, even one who is somehow an admin of another org, reaches nothing
    let list = people(&w.app, &w.alpha).await;
    assert_eq!(row(&list, "alpha@example.com")["org_role"], "owner");
    assert!(row(&list, "alpha@example.com")["suspended_at"].is_null());
    assert_eq!(call(&w.app, &w.alpha.cookie, "GET", "/api/auth/me", None).await.0, StatusCode::OK);
    // there is no route that removes a member (erasing an account stays with the person and the operator)
    for method in ["DELETE", "POST"] {
        let (status, _) = call(&w.app, &w.alpha.cookie, method, &format!("/api/admin/people/{}", w.mia.id), None).await;
        assert!(status == StatusCode::NOT_FOUND || status == StatusCode::METHOD_NOT_ALLOWED, "{method} -> {status}");
    }
    let (status, body) = call(&w.app, &w.alpha.cookie, "POST", &format!("/api/admin/people/{}/erasure", w.mia.id), Some(json!({}))).await;
    assert_eq!((status, code(&body)), (StatusCode::FORBIDDEN, Some("auth.forbidden")), "an organisation's owner does not erase another person's account: {body}");
}

#[tokio::test]
async fn an_admin_cannot_act_on_another_admin_but_the_owner_can() {
    let w = world().await;
    // alpha makes mia an admin too
    call(&w.app, &w.alpha.cookie, "PUT", &format!("/api/admin/people/{}/role", w.mia.id), Some(json!({ "role": "admin" }))).await;
    for (method, path, body) in member_routes(&w.mia.id).into_iter().filter(|(_, p, _)| !p.ends_with("/admin") && !p.contains("/tokens/") && !p.contains("/apps/")) {
        let (status, answer) = call(&w.app, &w.ann.cookie, method, &path, body).await;
        assert_eq!((status, code(&answer)), (StatusCode::FORBIDDEN, Some("auth.forbidden")), "ann -> {method} {path}: {answer}");
    }
    // the owner suspends and restores the admin
    let (status, list) = call(&w.app, &w.alpha.cookie, "PUT", &format!("/api/admin/people/{}/suspended", w.mia.id), Some(json!({ "suspended": true }))).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert!(row(&list, "mia@example.com")["suspended_at"].is_string());
    let (status, list) = call(&w.app, &w.alpha.cookie, "PUT", &format!("/api/admin/people/{}/suspended", w.mia.id), Some(json!({ "suspended": false }))).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    // an admin acts on an ordinary member
    let (status, list) = call(&w.app, &w.ann.cookie, "PUT", &format!("/api/admin/people/{}/suspended", w.max.id), Some(json!({ "suspended": true }))).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert!(row(&list, "max@example.com")["suspended_at"].is_string());
}

#[tokio::test]
async fn a_suspended_admin_loses_the_powers_at_once() {
    let w = world().await;
    let ann_token = models_user::create_api_token_with(&w.app.state.control, &models_user::user_by_email(&w.app.state.control, "ann@example.com").await.unwrap().unwrap().id, "ci", &eunomia_backend::scopes::ALL.iter().map(|s| s.to_string()).collect::<Vec<_>>(), None, None).await.unwrap().token;
    assert_eq!(bearer(&w.app, &ann_token, "GET", "/api/admin/people").await.0, StatusCode::OK);
    assert_eq!(call(&w.app, &w.ann.cookie, "GET", "/api/admin/people", None).await.0, StatusCode::OK);

    let (status, _) = call(&w.app, &w.alpha.cookie, "PUT", &format!("/api/admin/people/{}/suspended", w.ann.id), Some(json!({ "suspended": true }))).await;
    assert_eq!(status, StatusCode::OK);
    // the very next request: session and token are both refused, on every admin route
    for path in ["/api/admin/people", "/api/admin/audit", "/api/admin/org", "/api/admin/access-reviews"] {
        let (status, _) = call(&w.app, &w.ann.cookie, "GET", path, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}: the session ended");
        let (status, body) = bearer(&w.app, &ann_token, "GET", path).await;
        assert_eq!((status, code(&body)), (StatusCode::FORBIDDEN, Some("auth.suspended")), "{path}: {body}");
    }
    let (status, _) = call(&w.app, &w.ann.cookie, "PUT", &format!("/api/admin/people/{}/suspended", w.mia.id), Some(json!({ "suspended": true }))).await;
    assert_ne!(status, StatusCode::OK);
    assert!(row(&people(&w.app, &w.alpha).await, "mia@example.com")["suspended_at"].is_null(), "and nothing she tried took effect");
    // she cannot sign in again until restored; restored, she has her role back
    let ((status, _), _) = common::http(&w.app.router, "POST", "/api/auth/login", Some(json!({ "email": "ann@example.com", "password": common::PASSWORD })), None, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    call(&w.app, &w.alpha.cookie, "PUT", &format!("/api/admin/people/{}/suspended", w.ann.id), Some(json!({ "suspended": false }))).await;
    let again = login(&w.app, "ann@example.com").await;
    assert_eq!(call(&w.app, &again, "GET", "/api/admin/people", None).await.0, StatusCode::OK);
}

#[tokio::test]
async fn an_operator_address_the_first_account_and_an_instance_admin_cannot_be_acted_on_by_an_org_admin() {
    let w = world().await;
    // an operator address (EUNOMIA_ADMIN_EMAILS) that ended up as a member of Alpha, the first account, and an
    // account carrying the instance-admin flag
    let op = common::register_personal(&w.app.state, "op@corp.test").await;
    let flagged = common::register_personal(&w.app.state, "flagged@example.com").await;
    let alpha_org = models_user::user_by_email(&w.app.state.control, "alpha@example.com").await.unwrap().unwrap().org;
    for (u, role) in [(&op.id, "member"), (&flagged.id, "member"), (&w.app.user.id, "member")] {
        w.app
            .control()
            .test_raw()
            .query("DELETE membership WHERE user = $u; CREATE membership SET user = $u, org = $org, role = $role, slot = string::concat(<string>$org, '/', <string>$u);")
            .bind(("u", u.clone()))
            .bind(("org", alpha_org.record()))
            .bind(("role", role))
            .await
            .unwrap()
            .check()
            .unwrap();
    }
    w.app.control().test_raw().query("UPDATE $u SET admin = true").bind(("u", flagged.id.clone())).await.unwrap().check().unwrap();
    w.app.state.credentials.clear();
    let list = people(&w.app, &w.alpha).await;
    for email in ["op@corp.test", "flagged@example.com", "tester@example.com"] {
        let r = row(&list, email);
        assert_eq!((r["can_suspend"].as_bool(), r["can_set_role"].as_bool(), r["can_revoke"].as_bool()), (Some(false), Some(false), Some(false)), "{email}: {r}");
        let id = r["id"].as_str().unwrap().to_string();
        for actor in [&w.alpha, &w.ann] {
            for (method, path, body) in member_routes(&id).into_iter().filter(|(_, p, _)| !p.ends_with("/admin") && !p.contains("/tokens/") && !p.contains("/apps/")) {
                let (status, answer) = call(&w.app, &actor.cookie, method, &path, body).await;
                assert_eq!((status, code(&answer)), (StatusCode::FORBIDDEN, Some("org.protected_member")), "{} -> {email}: {method} {path}: {answer}", actor.email);
            }
            // making them an instance admin or taking that away is the instance's, not the org's
            let (status, answer) = call(&w.app, &actor.cookie, "PUT", &format!("/api/admin/people/{id}/admin"), Some(json!({ "admin": false }))).await;
            assert_eq!((status, code(&answer)), (StatusCode::FORBIDDEN, Some("auth.forbidden")), "{answer}");
        }
    }
    // nothing about them changed
    let list = people(&w.app, &w.alpha).await;
    assert!(["op@corp.test", "flagged@example.com", "tester@example.com"].iter().all(|e| row(&list, e)["suspended_at"].is_null()));
    // and the address can still never be invited into an organisation
    let (status, _) = call(&w.app, &w.alpha.cookie, "POST", "/api/org/invitations", Some(json!({ "email": "op@corp.test", "role": "member" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn the_operators_powers_stay_closed_to_an_organisations_owner_and_admins() {
    let w = world().await;
    let someone = &w.mia.id;
    for who in [&w.alpha, &w.ann] {
        for (method, path, body) in [
            ("GET", "/api/admin/security".to_string(), None),
            ("PUT", "/api/admin/security".to_string(), Some(json!({ "require_mfa": true }))),
            ("GET", "/api/debug/metrics".to_string(), None),
            ("POST", format!("/api/admin/people/{someone}/erasure"), Some(json!({}))),
            ("PUT", format!("/api/admin/people/{someone}/legal-hold"), Some(json!({ "hold": true }))),
            ("PUT", format!("/api/admin/people/{someone}/admin"), Some(json!({ "admin": true }))),
            ("PUT", format!("/api/admin/people/{someone}/workspace-owner"), Some(json!({}))),
            ("GET", "/api/admin/erasures".to_string(), None),
        ] {
            let (status, answer) = call(&w.app, &who.cookie, method, &path, body).await;
            assert_eq!((status, code(&answer)), (StatusCode::FORBIDDEN, Some("auth.forbidden")), "{} -> {method} {path}: {answer}", who.email);
        }
    }
    // the same for the operator is not refused (so the refusals above are about the caller, not the route)
    assert_eq!(call(&w.app, &w.operator.cookie, "GET", "/api/admin/security", None).await.0, StatusCode::OK);
    assert_eq!(call(&w.app, &w.operator.cookie, "GET", "/api/debug/metrics", None).await.0, StatusCode::OK);
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

async fn enrol(app: &common::TestApp, cookie: &str) {
    let (status, e) = call(app, cookie, "POST", "/api/auth/mfa/enrol", Some(json!({ "password": common::PASSWORD }))).await;
    assert_eq!(status, StatusCode::OK, "{e}");
    let code = mfa::code_at(e["secret"].as_str().unwrap(), now()).unwrap();
    let (status, done) = call(app, cookie, "POST", "/api/auth/mfa/enrol/confirm", Some(json!({ "code": code }))).await;
    assert_eq!(status, StatusCode::OK, "{done}");
}

async fn sign_in(app: &common::TestApp, email: &str) -> Value {
    let ((status, body), _) = common::http(&app.router, "POST", "/api/auth/login", Some(json!({ "email": email, "password": common::PASSWORD })), None, None).await;
    assert_eq!(status, StatusCode::OK, "{email}: {body}");
    body
}

#[tokio::test]
async fn the_org_mfa_policy_binds_only_that_orgs_members_and_combines_with_the_instance_switch() {
    let w = world().await;
    let org_policy = |on: bool| Some(json!({ "require_mfa": on }));
    // it needs the owner's own account protected first, as the instance switch does
    let (status, body) = call(&w.app, &w.alpha.cookie, "PUT", "/api/admin/org/security", org_policy(true)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    enrol(&w.app, &w.alpha.cookie).await;
    // nothing is required of anyone yet
    for e in ["mia@example.com", "bea@example.com"] {
        assert!(sign_in(&w.app, e).await.get("mfa_required").is_none(), "{e}");
    }
    let mia_session = login(&w.app, "mia@example.com").await;
    let bea_session = login(&w.app, "bea@example.com").await;

    // a member cannot switch it; an admin of the same org can (after enrolling too)
    assert_eq!(call(&w.app, &w.mia.cookie, "PUT", "/api/admin/org/security", org_policy(true)).await.0, StatusCode::FORBIDDEN);
    let (status, org) = call(&w.app, &w.alpha.cookie, "PUT", "/api/admin/org/security", org_policy(true)).await;
    assert_eq!(status, StatusCode::OK, "{org}");
    assert_eq!((org["require_mfa"].as_bool(), org["server_requires_mfa"].as_bool(), org["without_mfa"].as_i64()), (Some(true), Some(false), Some(3)), "ann, mia, max have none yet: {org}");
    assert_eq!(org["updated_by"], "alpha@example.com");

    // Alpha's members: enrolment prompt at the next sign-in, and a session that never passed a second factor is refused
    for e in ["mia@example.com", "ann@example.com"] {
        assert_eq!(sign_in(&w.app, e).await["mfa_required"], "enrol", "{e}");
    }
    assert_eq!(call(&w.app, &mia_session, "GET", "/api/auth/me", None).await.0, StatusCode::UNAUTHORIZED, "an existing session without a second factor ends");
    let (_, status_body) = call(&w.app, &w.alpha.cookie, "GET", "/api/auth/mfa", None).await;
    assert_eq!(status_body["required"], true);
    // a person who joins while it is on enrols at sign-up
    let (_, created) = call(&w.app, &w.alpha.cookie, "POST", "/api/org/invitations", Some(json!({ "email": "late@example.com", "role": "member" }))).await;
    let ((status, body), _) = common::http(&w.app.router, "POST", "/api/auth/register", Some(json!({ "email": "late@example.com", "password": common::PASSWORD, "invitation": created["token"] })), None, None).await;
    assert_eq!((status, body["mfa_required"].as_str()), (StatusCode::OK, Some("enrol")), "{body}");

    // Beta's members are untouched: Alpha's policy is Alpha's
    for e in ["bea@example.com", "bob@example.com", "beta@example.com"] {
        assert!(sign_in(&w.app, e).await.get("mfa_required").is_none(), "{e}");
    }
    assert_eq!(call(&w.app, &bea_session, "GET", "/api/auth/me", None).await.0, StatusCode::OK);
    let (_, bea_status) = call(&w.app, &w.bea.cookie, "GET", "/api/auth/mfa", None).await;
    assert_eq!(bea_status["required"], false);
    assert_eq!(call(&w.app, &w.beta.cookie, "GET", "/api/admin/org", None).await.1["require_mfa"], false);
    // a member who is required cannot turn it off (not even after enrolling); the operator's own org is untouched too
    assert!(sign_in(&w.app, "tester@example.com").await.get("mfa_required").is_none());

    // combined with the instance switch: either one requires it
    mfa::set_require_mfa(&w.app.state.control, true, &w.app.user.id).await.unwrap();
    w.app.state.credentials.clear();
    assert_eq!(sign_in(&w.app, "bea@example.com").await["mfa_required"], "enrol", "the instance switch binds Beta");
    let (status, org) = call(&w.app, &w.alpha.cookie, "PUT", "/api/admin/org/security", org_policy(false)).await;
    assert_eq!(status, StatusCode::OK, "{org}");
    assert_eq!((org["require_mfa"].as_bool(), org["server_requires_mfa"].as_bool()), (Some(false), Some(true)), "{org}");
    assert_eq!(sign_in(&w.app, "mia@example.com").await["mfa_required"], "enrol", "the org turned its own off; the instance still requires it");
    mfa::set_require_mfa(&w.app.state.control, false, &w.app.user.id).await.unwrap();
    w.app.state.credentials.clear();
    assert!(sign_in(&w.app, "mia@example.com").await.get("mfa_required").is_none(), "both off");
    assert!(sign_in(&w.app, "bea@example.com").await.get("mfa_required").is_none());
    // and on again, then Beta's own owner sets Beta's: Alpha's off/on state does not follow
    let (status, _) = call(&w.app, &w.alpha.cookie, "PUT", "/api/admin/org/security", org_policy(true)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(sign_in(&w.app, "mia@example.com").await["mfa_required"], "enrol");
    // the policy changes are in the org's own log, attributed
    let on = events(&w.app, "org.require_mfa").await;
    assert_eq!(on.len(), 3, "{on:?}");
    assert!(on.iter().all(|r| r["user"].to_string().contains(w.alpha.id.trim_start_matches("user:")) && r["actor_kind"] == "user"), "{on:?}");
}

#[tokio::test]
async fn revoking_access_ends_sessions_tokens_and_apps_and_suspension_ends_them_too() {
    let w = world().await;
    let control = w.app.control();
    let mia_id: surrealdb::types::RecordId = eunomia_backend::rid::parse(&w.mia.id).unwrap();
    let token = models_user::create_api_token_with(control, &mia_id, "laptop", &eunomia_backend::scopes::ALL.iter().map(|s| s.to_string()).collect::<Vec<_>>(), None, None).await.unwrap().token;
    let grant: Vec<surrealdb::types::RecordId> = control
        .test_raw()
        .query("LET $g = (CREATE oauth_grant SET owner = $u, client_id = 'client-x', client_name = 'Claude', scope = ['memory:read'], resource = 'http://localhost:8001/mcp' RETURN VALUE id)[0]; CREATE oauth_token SET kind = 'access', token_hash = 'h1', family = $g, expires_at = time::now() + 15m; RETURN [$g];")
        .bind(("u", mia_id.clone()))
        .await
        .unwrap()
        .take(2)
        .unwrap();
    let grant_id = grant[0].to_string();
    assert_eq!(bearer(&w.app, &token, "GET", "/api/auth/me").await.0, StatusCode::OK);

    // the admin sees them: tokens, apps
    let (status, tokens) = call(&w.app, &w.ann.cookie, "GET", &format!("/api/admin/people/{}/tokens", w.mia.id), None).await;
    assert_eq!((status, tokens.as_array().map(Vec::len)), (StatusCode::OK, Some(1)), "{tokens}");
    let (status, apps) = call(&w.app, &w.ann.cookie, "GET", &format!("/api/admin/people/{}/apps", w.mia.id), None).await;
    assert_eq!((status, apps.as_array().map(Vec::len), apps[0]["name"].as_str()), (StatusCode::OK, Some(1), Some("Claude")), "{apps}");
    // another account's grant, named under mia, is not found
    let (status, body) = call(&w.app, &w.ann.cookie, "DELETE", &format!("/api/admin/people/{}/apps/{}", w.max.id, grant_id), None).await;
    assert_eq!((status, code(&body)), (StatusCode::NOT_FOUND, Some("resource.not_found")), "{body}");
    assert_eq!(call(&w.app, &w.ann.cookie, "GET", &format!("/api/admin/people/{}/apps", w.mia.id), None).await.1.as_array().unwrap().len(), 1);
    // one app
    let (status, left) = call(&w.app, &w.ann.cookie, "DELETE", &format!("/api/admin/people/{}/apps/{}", w.mia.id, grant_id), None).await;
    assert_eq!((status, left), (StatusCode::OK, json!([])));
    // sessions only
    let extra = login(&w.app, "mia@example.com").await;
    let (status, ended) = call(&w.app, &w.ann.cookie, "DELETE", &format!("/api/admin/people/{}/sessions", w.mia.id), None).await;
    assert_eq!((status, ended["sessions"].as_i64().unwrap() >= 2), (StatusCode::OK, true), "{ended}");
    for c in [&w.mia.cookie, &extra] {
        assert_eq!(call(&w.app, c, "GET", "/api/auth/me", None).await.0, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(bearer(&w.app, &token, "GET", "/api/auth/me").await.0, StatusCode::OK, "tokens survive a sign-out");
    // everything at once
    let again = login(&w.app, "mia@example.com").await;
    let (status, out) = call(&w.app, &w.ann.cookie, "POST", &format!("/api/admin/people/{}/revoke-access", w.mia.id), None).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!((out["tokens"].as_i64(), out["apps"].as_i64()), (Some(1), Some(0)), "{out}");
    assert_eq!(bearer(&w.app, &token, "GET", "/api/auth/me").await.0, StatusCode::UNAUTHORIZED, "the token is revoked");
    assert_eq!(call(&w.app, &again, "GET", "/api/auth/me", None).await.0, StatusCode::UNAUTHORIZED);
    // she can sign in again: revoking is not suspending
    let fresh = login(&w.app, "mia@example.com").await;
    assert_eq!(call(&w.app, &fresh, "GET", "/api/auth/me", None).await.0, StatusCode::OK);
}

#[tokio::test]
async fn every_admin_action_is_audited_with_the_actor_and_shows_in_the_orgs_own_log() {
    let w = world().await;
    let mia_id = eunomia_backend::rid::parse(&w.mia.id).unwrap();
    let token = models_user::create_api_token_with(w.app.control(), &mia_id, "laptop", &["memory:read".to_string()], None, None).await.unwrap();
    w.app.control().test_raw().query("CREATE oauth_grant SET owner = $u, client_id = 'client-x', client_name = 'Claude', scope = ['memory:read'], resource = 'http://x'").bind(("u", mia_id.clone())).await.unwrap().check().unwrap();
    let grant: Vec<Value> = call(&w.app, &w.ann.cookie, "GET", &format!("/api/admin/people/{}/apps", w.mia.id), None).await.1.as_array().unwrap().clone();

    let t = |p: &str| format!("/api/admin/people/{}/{p}", w.mia.id);
    let steps: Vec<(&P, &str, String, Option<Value>, &str)> = vec![
        (&w.alpha, "PUT", t("role"), Some(json!({ "role": "admin" })), "org.role"),
        (&w.alpha, "PUT", t("role"), Some(json!({ "role": "member" })), "org.role"),
        (&w.ann, "PUT", t("suspended"), Some(json!({ "suspended": true })), "user.suspend"),
        (&w.ann, "PUT", t("suspended"), Some(json!({ "suspended": false })), "user.restore"),
        (&w.ann, "DELETE", t("sessions"), None, "admin.sessions_end"),
        (&w.ann, "DELETE", format!("/api/admin/people/{}/tokens/{}", w.mia.id, token.id.to_string()), None, "admin.token_revoke"),
        (&w.ann, "DELETE", format!("/api/admin/people/{}/apps/{}", w.mia.id, grant[0]["id"].as_str().unwrap()), None, "admin.app_revoke"),
        (&w.ann, "POST", t("revoke-access"), None, "admin.access_revoke"),
        (&w.alpha, "PUT", "/api/admin/org/name".into(), Some(json!({ "name": "Alpha Pty" })), "org.rename"),
        (&w.ann, "GET", "/api/admin/audit/export".into(), None, "audit.export"),
        (&w.ann, "POST", "/api/admin/access-reviews".into(), None, "access_review.start"),
    ];
    for (actor, method, path, body, action) in &steps {
        let (status, out) = call(&w.app, &actor.cookie, method, path, body.clone()).await;
        assert_eq!(status, StatusCode::OK, "{} {method} {path}: {out}", actor.email);
        let rows = events(&w.app, action).await;
        let last = rows.last().unwrap_or_else(|| panic!("no {action} event"));
        assert!(last["user"].to_string().contains(actor.id.trim_start_matches("user:")), "{action} is filed under {}: {last}", actor.email);
        assert_eq!((last["actor_kind"].as_str(), last["outcome"].as_str()), (Some("user"), Some("ok")), "{action}: {last}");
    }
    // the review's decision and completion, by an admin
    let (_, reviews) = call(&w.app, &w.ann.cookie, "GET", "/api/admin/access-reviews", None).await;
    let review = reviews[0]["id"].as_str().unwrap().to_string();
    for p in [&w.alpha, &w.mia, &w.max] {
        if p.email == "alpha@example.com" || p.email == "mia@example.com" || p.email == "max@example.com" {
            let (status, out) = call(&w.app, &w.ann.cookie, "PUT", &format!("/api/admin/access-reviews/{review}/decisions/{}", p.id), Some(json!({ "decision": "keep" }))).await;
            assert_eq!(status, StatusCode::OK, "{out}");
        }
    }
    let (status, out) = call(&w.app, &w.alpha.cookie, "PUT", &format!("/api/admin/access-reviews/{review}/decisions/{}", w.ann.id), Some(json!({ "decision": "keep" }))).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let (status, out) = call(&w.app, &w.ann.cookie, "POST", &format!("/api/admin/access-reviews/{review}/complete"), None).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    for action in ["access_review.decide", "access_review.complete"] {
        assert!(!events(&w.app, action).await.is_empty(), "{action}");
    }
    // the snapshot says each person's role in the org
    assert!(out.to_string().contains("\"alpha@example.com\""), "{out}");

    // the org's own log shows them, by name, and Beta's log shows none of them
    let (_, page) = call(&w.app, &w.alpha.cookie, "GET", "/api/admin/audit?category=admin&limit=200", None).await;
    let text = page.to_string();
    for sentence in [
        "Made mia@example.com an admin of the organisation",
        "Suspended mia@example.com",
        "Restored access for mia@example.com",
        "Signed mia@example.com out everywhere",
        "Ended every session, API token and connected app of mia@example.com",
        "Renamed the organisation to",
        "Disconnected the app",
    ] {
        assert!(text.contains(sentence), "{sentence}: {text}");
    }
    assert!(page["results"].as_array().unwrap().iter().all(|e| e["category"] == "admin" || !e["actor_email"].as_str().unwrap_or("").is_empty()));
    let (_, theirs) = call(&w.app, &w.beta.cookie, "GET", "/api/admin/audit?category=all&limit=200", None).await;
    assert!(!theirs.to_string().contains("mia@example.com") && !theirs.to_string().contains("Alpha Pty"), "{theirs}");
    // and the whole thing is in the hash chain and verifies, for Alpha's ledger
    let (status, verify) = call(&w.app, &w.alpha.cookie, "GET", "/api/admin/audit/verify", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(verify["ledgers"][0]["ok"], true, "{verify}");
    common::assert_chained(w.app.control(), &["org.role", "user.suspend", "admin.sessions_end", "admin.access_revoke", "org.rename"]).await;
}

#[tokio::test]
async fn only_the_owner_renames_the_org_and_the_name_is_checked() {
    let w = world().await;
    for who in [&w.ann, &w.mia] {
        let (status, body) = call(&w.app, &who.cookie, "PUT", "/api/admin/org/name", Some(json!({ "name": "Hijacked" }))).await;
        assert_eq!((status, code(&body)), (StatusCode::FORBIDDEN, Some("auth.forbidden")), "{}: {body}", who.email);
    }
    for bad in ["", "   ", "ops@acme.test", &"x".repeat(81), "Bell\u{7}"] {
        let (status, body) = call(&w.app, &w.alpha.cookie, "PUT", "/api/admin/org/name", Some(json!({ "name": bad }))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad:?}: {body}");
    }
    let (status, org) = call(&w.app, &w.alpha.cookie, "PUT", "/api/admin/org/name", Some(json!({ "name": "  Alpha   Pty Ltd " }))).await;
    assert_eq!((status, org["name"].as_str(), org["can_rename"].as_bool()), (StatusCode::OK, Some("Alpha Pty Ltd"), Some(true)), "{org}");
    // a token cannot rename (a browser session is needed), and the name is what invitees are shown
    let alpha_id = eunomia_backend::rid::parse(&w.alpha.id).unwrap();
    let token = models_user::create_api_token_with(w.app.control(), &alpha_id, "t", &eunomia_backend::scopes::ALL.iter().map(|s| s.to_string()).collect::<Vec<_>>(), None, None).await.unwrap().token;
    let (status, _) = common::http(&w.app.router, "PUT", "/api/admin/org/name", Some(json!({ "name": "By token" })), Some(&token), None).await.0;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (_, created) = call(&w.app, &w.alpha.cookie, "POST", "/api/org/invitations", Some(json!({ "email": "new@example.com", "role": "member" }))).await;
    let ((status, seen), _) = common::http(&w.app.router, "POST", "/api/invitations/lookup", Some(json!({ "token": created["token"] })), None, None).await;
    assert_eq!((status, seen["org_name"].as_str(), seen["personal"].as_bool()), (StatusCode::OK, Some("Alpha Pty Ltd"), Some(false)), "{seen}");
}

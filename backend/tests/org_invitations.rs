//! Organisation invitations on a host that runs many orgs (`invitations.rs`, `routers/org.rs`).
//!
//! The whole binary runs as a shared host: `EUNOMIA_SIGNUP_ORG=personal` (a plain signup gets an org of
//! its own) and `SIGNUP=closed` (nobody may sign up without an invitation). The env is set once, before
//! any test touches the app, to the same values for every test.

mod common;

use axum::http::StatusCode;
use eunomia_backend::invitations::{self, OrgInviteRole};
use eunomia_backend::models_user::{self, User};
use eunomia_backend::rid::RecordIdExt;
use eunomia_backend::state::AppState;
use serde_json::{json, Value};

fn host_env() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // SAFETY: set once, before any test reads them, to the same values for every test in this binary
        unsafe {
            std::env::set_var("EUNOMIA_SIGNUP_ORG", "personal");
            std::env::set_var("SIGNUP", "closed");
            // an instance operator's address: never invitable, never created through an invitation
            std::env::set_var("EUNOMIA_ADMIN_EMAILS", "op@corp.test");
            // logins and signups in one test go past the default 20 per minute per address
            std::env::set_var("RATE_LIMIT_AUTH_PER_MIN", "1000");
        }
    });
}

struct Host {
    app: common::TestApp,
    /// owner of org A (not an instance admin: an org of their own on a shared host)
    alpha: User,
    alpha_cookie: String,
}

async fn host() -> Host {
    host_env();
    let app = common::TestApp::new().await;
    let alpha = common::register_personal(&app.state, "alpha@example.com").await;
    assert_ne!(alpha.org, app.user.org);
    let alpha_cookie = login(&app, "alpha@example.com").await;
    Host { app, alpha, alpha_cookie }
}

async fn login(app: &common::TestApp, email: &str) -> String {
    let ((status, body), cookie) =
        common::http(&app.router, "POST", "/api/auth/login", Some(json!({ "email": email, "password": common::PASSWORD })), None, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    cookie.expect("a session cookie").split(';').next().unwrap().to_string()
}

async fn as_cookie(app: &common::TestApp, cookie: &str, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    common::http(&app.router, method, path, body, None, Some(cookie)).await.0
}

async fn invite(h: &Host, email: &str, role: &str) -> Value {
    let (status, body) = as_cookie(&h.app, &h.alpha_cookie, "POST", "/api/org/invitations", Some(json!({ "email": email, "role": role }))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

async fn register(app: &common::TestApp, email: &str, invitation: Option<&str>) -> ((StatusCode, Value), Option<String>) {
    let mut body = json!({ "email": email, "password": common::PASSWORD });
    if let Some(t) = invitation {
        body["invitation"] = json!(t);
    }
    common::http(&app.router, "POST", "/api/auth/register", Some(body), None, None).await
}

async fn lookup(app: &common::TestApp, token: &str) -> (StatusCode, Value) {
    common::http(&app.router, "POST", "/api/invitations/lookup", Some(json!({ "token": token })), None, None).await.0
}

async fn audit_rows(state: &AppState, action: &str) -> Vec<Value> {
    let mut res = state
        .control
        .test_raw()
        .query("SELECT user, actor_kind, action, target, outcome, detail FROM audit_event WHERE action = $a ORDER BY created_at")
        .bind(("a", action.to_string()))
        .await
        .unwrap();
    res.take(0).unwrap()
}

async fn memberships(state: &AppState, user: &surrealdb::types::RecordId) -> Vec<Value> {
    let mut res = state.control.test_raw().query("SELECT org, role FROM membership WHERE user = $u").bind(("u", user.clone())).await.unwrap();
    res.take(0).unwrap()
}

/// Every control row, as text: what a leak of the token would show up in.
async fn control_dump(state: &AppState) -> String {
    let mut all = String::new();
    for t in eunomia_backend::store::CONTROL_TABLES {
        let rows: Vec<Value> = state.control.test_raw().query(format!("SELECT * FROM {t}")).await.unwrap().take(0).unwrap();
        all.push_str(&serde_json::to_string(&rows).unwrap());
    }
    all
}

#[tokio::test]
async fn an_invited_person_signs_up_into_the_inviting_org_even_on_a_closed_personal_host() {
    let h = host().await;
    let created = invite(&h, " Pat@Example.com ", "member").await;
    let token = created["token"].as_str().unwrap().to_string();
    assert_eq!(created["invitation"]["email"], "pat@example.com", "stored normalized");
    assert!(created["link"].as_str().unwrap().ends_with(&format!("/invite#{token}")), "the token rides in the fragment: {created}");

    // the list and the seats never carry the token
    let (status, list) = as_cookie(&h.app, &h.alpha_cookie, "GET", "/api/org/invitations", None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert!(!list.to_string().contains(&token));
    let (_, seats) = as_cookie(&h.app, &h.alpha_cookie, "GET", "/api/org/seats", None).await;
    assert_eq!(seats, json!({ "active_members": 1, "suspended_members": 0, "pending_invitations": 1 }));

    // the page can show what the link is for
    let (status, seen) = lookup(&h.app, &token).await;
    assert_eq!(status, StatusCode::OK, "{seen}");
    assert_eq!((seen["email"].as_str(), seen["status"].as_str()), (Some("pat@example.com"), Some("open")));
    assert!(seen.get("invited_by").is_none(), "no unverified inviter address on the page: {seen}");
    // alpha's org is a personal workspace, named after alpha's address: the name is not shown
    assert_eq!((seen["personal"].as_bool(), seen["org_name"].as_str()), (Some(true), Some("")), "{seen}");
    assert!(!seen.to_string().contains("alpha@example.com"), "{seen}");

    // without the invitation, signup is closed; with someone else's email, the invitation does not count
    let ((status, body), _) = register(&h.app, "pat@example.com", None).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::FORBIDDEN, Some("auth.forbidden")), "{body}");
    let ((status, body), _) = register(&h.app, "mallory@example.com", Some(&token)).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::FORBIDDEN, Some("invitation.email_mismatch")), "{body}");
    assert!(models_user::user_by_email(&h.app.state.control, "mallory@example.com").await.unwrap().is_none());

    // the invited email (any case) signs up and lands in org A as a member, not in an org of its own
    let ((status, me), cookie) = register(&h.app, "PAT@example.com", Some(&token)).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!((me["org_role"].as_str(), me["can_manage_org"].as_bool()), (Some("member"), Some(false)), "{me}");
    let pat = models_user::user_by_email(&h.app.state.control, "pat@example.com").await.unwrap().unwrap();
    assert_eq!(pat.org, h.alpha.org);
    assert_eq!(memberships(&h.app.state, &pat.id).await.len(), 1, "one membership, no personal org");
    // with a working session and a personal space in org A
    let cookie = cookie.unwrap().split(';').next().unwrap().to_string();
    let (status, vaults) = as_cookie(&h.app, &cookie, "GET", "/api/vaults", None).await;
    assert_eq!(status, StatusCode::OK, "{vaults}");
    assert!(vaults.to_string().contains("personal"), "{vaults}");

    // single use
    let ((status, body), _) = register(&h.app, "pat@example.com", Some(&token)).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::NOT_FOUND, Some("invitation.not_found")), "{body}");
    let (status, _) = lookup(&h.app, &token).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, seats) = as_cookie(&h.app, &h.alpha_cookie, "GET", "/api/org/seats", None).await;
    assert_eq!(seats, json!({ "active_members": 2, "suspended_members": 0, "pending_invitations": 0 }));

    // the ledger: create and accept, scoped to org A's people, and the token is in no control row
    let created = audit_rows(&h.app.state, "org.invite").await;
    assert_eq!(created.len(), 1);
    assert_eq!(created[0]["user"], json!(h.alpha.id.to_string()));
    assert_eq!(created[0]["detail"], "email=pat@example.com role=member");
    // the wrong-email attempt above left the generic "not accepted" row; the signup left the accepted one
    let rows = audit_rows(&h.app.state, "org.invite_accept").await;
    assert_eq!(rows.iter().filter(|r| r["outcome"] == invitations::NOT_ACCEPTED).count(), 1, "{rows:?}");
    let accepted: Vec<&Value> = rows.iter().filter(|r| r["outcome"] == "ok").collect();
    assert_eq!(accepted.len(), 1, "{rows:?}");
    assert_eq!(accepted[0]["user"], json!(pat.id.to_string()));
    let dump = control_dump(&h.app.state).await;
    assert!(!dump.contains(&token), "the invitation token is stored somewhere");
}

#[tokio::test]
async fn an_invited_admin_can_invite_and_a_member_cannot() {
    let h = host().await;
    let token = invite(&h, "ada@example.com", "admin").await["token"].as_str().unwrap().to_string();
    let ((status, me), _) = register(&h.app, "ada@example.com", Some(&token)).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!((me["org_role"].as_str(), me["can_manage_org"].as_bool(), me["is_admin"].as_bool()), (Some("admin"), Some(true), Some(false)), "{me}");
    let ada_cookie = login(&h.app, "ada@example.com").await;
    let (status, body) = as_cookie(&h.app, &ada_cookie, "POST", "/api/org/invitations", Some(json!({ "email": "mo@example.com" }))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["invitation"]["role"], "member", "member is the default");
    // an org admin is not an instance admin
    let (status, _) = as_cookie(&h.app, &ada_cookie, "GET", "/api/admin/people", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let mo_token = body["token"].as_str().unwrap().to_string();
    let ((status, _), _) = register(&h.app, "mo@example.com", Some(&mo_token)).await;
    assert_eq!(status, StatusCode::OK);
    let mo_cookie = login(&h.app, "mo@example.com").await;
    for (method, path, body) in [
        ("GET", "/api/org/invitations", None),
        ("POST", "/api/org/invitations", Some(json!({ "email": "x@example.com" }))),
        ("GET", "/api/org/seats", None),
    ] {
        let (status, body) = as_cookie(&h.app, &mo_cookie, method, path, body).await;
        assert_eq!((status, body["code"].as_str()), (StatusCode::FORBIDDEN, Some("auth.forbidden")), "{method} {path}: {body}");
    }
    // an invitation can never make an owner
    let (status, _) = as_cookie(&h.app, &h.alpha_cookie, "POST", "/api/org/invitations", Some(json!({ "email": "o@example.com", "role": "owner" }))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn creating_resending_and_revoking_need_a_browser_session() {
    let h = host().await;
    let alpha_token = models_user::create_api_token_with(&h.app.state.control, &h.alpha.id, "t", &eunomia_backend::scopes::ALL.iter().map(|s| s.to_string()).collect::<Vec<_>>(), None, None)
        .await
        .unwrap()
        .token;
    let ((status, body), _) = common::http(&h.app.router, "POST", "/api/org/invitations", Some(json!({ "email": "pat@example.com" })), Some(&alpha_token), None).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::FORBIDDEN, Some("auth.scope")), "{body}");
    // reading works with a token
    let ((status, _), _) = common::http(&h.app.router, "GET", "/api/org/seats", None, Some(&alpha_token), None).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn resend_makes_a_new_link_and_revoke_withdraws_it() {
    let h = host().await;
    let first = invite(&h, "pat@example.com", "member").await;
    let id = first["invitation"]["id"].as_str().unwrap().to_string();
    let old = first["token"].as_str().unwrap().to_string();

    // a second open invitation for the same email is refused: resend it instead
    let (status, body) = as_cookie(&h.app, &h.alpha_cookie, "POST", "/api/org/invitations", Some(json!({ "email": "PAT@example.com" }))).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::CONFLICT, Some("invitation.exists")), "{body}");

    let (status, resent) = as_cookie(&h.app, &h.alpha_cookie, "POST", &format!("/api/org/invitations/{id}/resend"), None).await;
    assert_eq!(status, StatusCode::OK, "{resent}");
    let new = resent["token"].as_str().unwrap().to_string();
    assert_ne!(old, new);
    assert_eq!(lookup(&h.app, &old).await.0, StatusCode::NOT_FOUND, "the old link stops working");
    assert_eq!(lookup(&h.app, &new).await.0, StatusCode::OK);

    let (status, body) = as_cookie(&h.app, &h.alpha_cookie, "DELETE", &format!("/api/org/invitations/{id}"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(lookup(&h.app, &new).await.0, StatusCode::NOT_FOUND, "a withdrawn link stops working");
    let ((status, body), _) = register(&h.app, "pat@example.com", Some(&new)).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::NOT_FOUND, Some("invitation.not_found")), "{body}");
    let (_, list) = as_cookie(&h.app, &h.alpha_cookie, "GET", "/api/org/invitations", None).await;
    assert_eq!(list, json!([]));
    // withdrawn: twice is not found, and resending is not possible
    let (status, _) = as_cookie(&h.app, &h.alpha_cookie, "DELETE", &format!("/api/org/invitations/{id}"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = as_cookie(&h.app, &h.alpha_cookie, "POST", &format!("/api/org/invitations/{id}/resend"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // and the email can be invited again
    invite(&h, "pat@example.com", "member").await;

    assert_eq!(audit_rows(&h.app.state, "org.invite_resend").await.len(), 1);
    let revoked = audit_rows(&h.app.state, "org.invite_revoke").await;
    assert_eq!((revoked.len(), revoked[0]["detail"].as_str()), (1, Some("email=pat@example.com role=member")));
    let dump = control_dump(&h.app.state).await;
    assert!(!dump.contains(&old) && !dump.contains(&new), "a token is stored somewhere");
}

#[tokio::test]
async fn an_expired_invitation_is_refused_audited_and_can_be_resent() {
    let h = host().await;
    let created = invite(&h, "pat@example.com", "member").await;
    let token = created["token"].as_str().unwrap().to_string();
    let id = created["invitation"]["id"].as_str().unwrap().to_string();
    h.app.state.control.test_raw().query("UPDATE org_invitation SET expires_at = time::now() - 1s").await.unwrap().check().unwrap();

    let (status, seen) = lookup(&h.app, &token).await;
    assert_eq!((status, seen["status"].as_str()), (StatusCode::OK, Some("expired")), "{seen}");
    let ((status, body), _) = register(&h.app, "pat@example.com", Some(&token)).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::GONE, Some("invitation.expired")), "{body}");
    assert!(body["detail"].as_str().unwrap_or_default().contains("Ask an admin of"), "{body}");
    assert!(models_user::user_by_email(&h.app.state.control, "pat@example.com").await.unwrap().is_none(), "no account was made");
    // the refusal is in the inviting org's log
    let refused = audit_rows(&h.app.state, "org.invite_expired").await;
    assert_eq!(refused.len(), 1);
    assert_eq!((refused[0]["outcome"].as_str(), refused[0]["actor_kind"].as_str()), (Some("invitation.expired"), Some("anonymous")));
    assert_eq!(refused[0]["user"], json!(h.alpha.id.to_string()));

    // listed as expired, not counted as a seat, and a resend makes it work again
    let (_, list) = as_cookie(&h.app, &h.alpha_cookie, "GET", "/api/org/invitations", None).await;
    assert_eq!(list[0]["expired"], true, "{list}");
    let (_, seats) = as_cookie(&h.app, &h.alpha_cookie, "GET", "/api/org/seats", None).await;
    assert_eq!(seats["pending_invitations"], 0);
    let (status, resent) = as_cookie(&h.app, &h.alpha_cookie, "POST", &format!("/api/org/invitations/{id}/resend"), None).await;
    assert_eq!(status, StatusCode::OK, "{resent}");
    let ((status, body), _) = register(&h.app, "pat@example.com", Some(resent["token"].as_str().unwrap())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn two_accepts_of_one_token_make_one_membership() {
    let h = host().await;
    let token = invite(&h, "pat@example.com", "member").await["token"].as_str().unwrap().to_string();
    // two signups through the same link at once: one account, one membership
    let (a, b) = tokio::join!(register(&h.app, "pat@example.com", Some(&token)), register(&h.app, "pat@example.com", Some(&token)));
    let ok = [a.0 .0, b.0 .0].iter().filter(|s| **s == StatusCode::OK).count();
    assert_eq!(ok, 1, "{a:?} {b:?}");
    let pat = models_user::user_by_email(&h.app.state.control, "pat@example.com").await.unwrap().unwrap();
    assert_eq!(memberships(&h.app.state, &pat.id).await.len(), 1);

    // the claim itself, raced eight ways for one account with no membership yet
    let sam_token = invite(&h, "sam@example.com", "member").await["token"].as_str().unwrap().to_string();
    let mut res = h.app.state.control.test_raw().query("CREATE user SET email = 'sam@example.com', email_lc = 'sam@example.com' RETURN VALUE id").await.unwrap();
    let sam: surrealdb::types::RecordId = res.take::<Vec<surrealdb::types::RecordId>>(0).unwrap().remove(0);
    let pending = invitations::by_hash(&h.app.state.control, &invitations::token_hash(&sam_token)).await.unwrap().expect("open invitation");
    let control = h.app.state.control.clone();
    let tries = (0..8).map(|_| {
        let (control, sam, pending) = (control.clone(), sam.clone(), pending.clone());
        tokio::spawn(async move { invitations::claim(&control, &sam, &pending, true).await })
    });
    let results: Vec<_> = futures::future::join_all(tries).await.into_iter().map(|r| r.unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1, "{results:?}");
    assert!(results.iter().filter_map(|r| r.as_ref().err()).all(|e| e.code.as_str() == "invitation.not_found"), "{results:?}");
    let rows = memberships(&h.app.state, &sam).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["org"], json!(h.alpha.org.record().to_string()));
}

#[tokio::test]
async fn an_account_in_another_org_cannot_accept_and_inviting_it_reveals_nothing() {
    let h = host().await;
    // beta has an org of their own; nobody@ has no account. Inviting either looks the same.
    let beta = common::register_personal(&h.app.state, "beta@example.com").await;
    let to_beta = invite(&h, "beta@example.com", "member").await;
    let to_nobody = invite(&h, "nobody@example.com", "member").await;
    let shape = |v: &Value| {
        let mut keys: Vec<String> = v["invitation"].as_object().unwrap().keys().cloned().collect();
        keys.sort();
        (keys, v["invitation"]["expired"].clone(), v["invitation"]["role"].clone())
    };
    assert_eq!(shape(&to_beta), shape(&to_nobody));
    let (s1, l1) = lookup(&h.app, to_beta["token"].as_str().unwrap()).await;
    let (s2, l2) = lookup(&h.app, to_nobody["token"].as_str().unwrap()).await;
    assert_eq!((s1, l1.as_object().unwrap().len()), (s2, l2.as_object().unwrap().len()));

    let beta_cookie = login(&h.app, "beta@example.com").await;
    let (status, body) = as_cookie(&h.app, &beta_cookie, "POST", "/api/invitations/accept", Some(json!({ "token": to_beta["token"] }))).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::CONFLICT, Some("invitation.other_org")), "{body}");
    assert!(body["detail"].as_str().unwrap().contains("only be in one"), "{body}");
    // nothing changed: beta is still only in their own org, and the invitation is still open
    let rows = memberships(&h.app.state, &beta.id).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["org"], json!(beta.org.record().to_string()));
    assert_eq!(lookup(&h.app, to_beta["token"].as_str().unwrap()).await.0, StatusCode::OK);
    // the inviting org's ledger says only that it was not accepted: nothing about an account elsewhere
    let refused = audit_rows(&h.app.state, "org.invite_accept").await;
    assert_eq!((refused.len(), refused[0]["outcome"].as_str()), (1, Some(invitations::NOT_ACCEPTED)));
    assert_eq!(refused[0]["user"], json!(h.alpha.id.to_string()));
    let all: Vec<Value> = h.app.state.control.test_raw().query("SELECT * FROM audit_event").await.unwrap().take(0).unwrap();
    assert!(!serde_json::to_string(&all).unwrap().contains("other_org"), "the precise reason is in no ledger row");

    // a signed-in account with another email is refused, whatever org it is in
    let (status, body) = as_cookie(&h.app, &beta_cookie, "POST", "/api/invitations/accept", Some(json!({ "token": to_nobody["token"] }))).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::FORBIDDEN, Some("invitation.email_mismatch")), "{body}");
}

#[tokio::test]
async fn a_member_already_in_the_org_cannot_be_invited_and_a_signed_in_member_accepts_as_a_no_op() {
    let h = host().await;
    let token = invite(&h, "pat@example.com", "member").await["token"].as_str().unwrap().to_string();
    let ((status, _), _) = register(&h.app, "pat@example.com", Some(&token)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = as_cookie(&h.app, &h.alpha_cookie, "POST", "/api/org/invitations", Some(json!({ "email": "pat@example.com" }))).await;
    assert_eq!((status, body["code"].as_str(), body["detail"].as_str()), (StatusCode::CONFLICT, Some("invitation.exists"), Some("That person is already in this organisation.")));

    // an invitation that raced the signup: the member accepts it signed in, and it is used up
    let pending = invitations::create(&h.app.state, &h.alpha, "quinn@example.com", OrgInviteRole::Member).await.unwrap();
    let quinn = models_user::register_invited(&h.app.state, "quinn@example.com", common::PASSWORD, &invitations::by_hash(&h.app.state.control, &invitations::token_hash(&pending.token)).await.unwrap().unwrap())
        .await
        .unwrap();
    assert_eq!(quinn.org, h.alpha.org);
    let again = invite(&h, "quinn2@example.com", "member").await;
    h.app.state.control.test_raw().query("UPDATE org_invitation SET email = 'quinn@example.com' WHERE email = 'quinn2@example.com'").await.unwrap().check().unwrap();
    let quinn_cookie = login(&h.app, "quinn@example.com").await;
    let (status, body) = as_cookie(&h.app, &quinn_cookie, "POST", "/api/invitations/accept", Some(json!({ "token": again["token"] }))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["already_member"], true);
    assert_eq!(memberships(&h.app.state, &quinn.id).await.len(), 1);
    assert_eq!(lookup(&h.app, again["token"].as_str().unwrap()).await.0, StatusCode::NOT_FOUND);
    assert_eq!(lookup(&h.app, &pending.token).await.0, StatusCode::NOT_FOUND, "the first one was used by the signup");

    // an admin invitation that raced: accepting it makes the member an admin, and says so truthfully
    let promote = invite(&h, "quinn3@example.com", "admin").await;
    h.app.state.control.test_raw().query("UPDATE org_invitation SET email = 'quinn@example.com' WHERE email = 'quinn3@example.com'").await.unwrap().check().unwrap();
    let (status, body) = as_cookie(&h.app, &quinn_cookie, "POST", "/api/invitations/accept", Some(json!({ "token": promote["token"] }))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!((body["role"].as_str(), body["already_member"].as_bool()), (Some("admin"), Some(true)), "{body}");
    let (_, me) = as_cookie(&h.app, &quinn_cookie, "GET", "/api/auth/me", None).await;
    assert_eq!(me["org_role"], "admin", "{me}");
    // an owner accepting a member invitation stays the owner, and the answer says owner
    let to_owner = invite(&h, "alpha2@example.com", "member").await;
    h.app.state.control.test_raw().query("UPDATE org_invitation SET email = 'alpha@example.com' WHERE email = 'alpha2@example.com'").await.unwrap().check().unwrap();
    let (status, body) = as_cookie(&h.app, &h.alpha_cookie, "POST", "/api/invitations/accept", Some(json!({ "token": to_owner["token"] }))).await;
    assert_eq!((status, body["role"].as_str()), (StatusCode::OK, Some("owner")), "{body}");
}

#[tokio::test]
async fn only_the_owner_manages_admin_invitations() {
    let h = host().await;
    let token = invite(&h, "ada@example.com", "admin").await["token"].as_str().unwrap().to_string();
    assert_eq!(register(&h.app, "ada@example.com", Some(&token)).await.0 .0, StatusCode::OK);
    let ada = login(&h.app, "ada@example.com").await;
    // an org admin cannot invite an admin
    let (status, body) = as_cookie(&h.app, &ada, "POST", "/api/org/invitations", Some(json!({ "email": "eve@example.com", "role": "admin" }))).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::FORBIDDEN, Some("auth.forbidden")), "{body}");
    // nor resend or withdraw the owner's admin invitation
    let owners = invite(&h, "bea@example.com", "admin").await;
    let id = owners["invitation"]["id"].as_str().unwrap();
    for (method, path) in [("POST", format!("/api/org/invitations/{id}/resend")), ("DELETE", format!("/api/org/invitations/{id}"))] {
        let (status, body) = as_cookie(&h.app, &ada, method, &path, None).await;
        assert_eq!((status, body["code"].as_str()), (StatusCode::FORBIDDEN, Some("auth.forbidden")), "{method} {path}: {body}");
    }
    assert_eq!(lookup(&h.app, owners["token"].as_str().unwrap()).await.0, StatusCode::OK, "the owner's link still works");
    // members are fine for an org admin, from start to finish
    let mem = as_cookie(&h.app, &ada, "POST", "/api/org/invitations", Some(json!({ "email": "mo@example.com", "role": "member" }))).await.1;
    let mid = mem["invitation"]["id"].as_str().unwrap();
    assert_eq!(as_cookie(&h.app, &ada, "POST", &format!("/api/org/invitations/{mid}/resend"), None).await.0, StatusCode::OK);
    assert_eq!(as_cookie(&h.app, &ada, "DELETE", &format!("/api/org/invitations/{mid}"), None).await.0, StatusCode::OK);
    // and the owner can do all of it
    assert_eq!(as_cookie(&h.app, &h.alpha_cookie, "POST", &format!("/api/org/invitations/{id}/resend"), None).await.0, StatusCode::OK);
    assert_eq!(as_cookie(&h.app, &h.alpha_cookie, "DELETE", &format!("/api/org/invitations/{id}"), None).await.0, StatusCode::OK);
}

#[tokio::test]
async fn an_operator_address_is_never_invited_or_created_through_an_invitation() {
    let h = host().await;
    // creating: the same answer as any other address that can't be invited, naming no reason
    for email in ["op@corp.test", " OP@Corp.Test "] {
        let (status, body) = as_cookie(&h.app, &h.alpha_cookie, "POST", "/api/org/invitations", Some(json!({ "email": email }))).await;
        assert_eq!((status, body["code"].as_str()), (StatusCode::BAD_REQUEST, Some("validation.invalid")), "{email}: {body}");
        let detail = body["detail"].as_str().unwrap().to_lowercase();
        assert!(detail.contains("can't be invited") && !detail.contains("admin") && !detail.contains("operator"), "{detail}");
    }
    // accepting: an invitation for the address that got in some other way (planted here) creates nothing
    let token = "planted-operator-invitation-token-0123456789abcdef";
    h.app.state.control.test_raw()
        .query("CREATE org_invitation SET org = $org, email = 'op@corp.test', role = 'member', token_hash = $h, invited_by = $by, expires_at = time::now() + 7d, slot = string::concat(<string>$org, '/op@corp.test')")
        .bind(("org", h.alpha.org.record()))
        .bind(("h", invitations::token_hash(token)))
        .bind(("by", h.alpha.id.clone()))
        .await.unwrap().check().unwrap();
    assert_eq!(lookup(&h.app, token).await.0, StatusCode::NOT_FOUND);
    let ((status, body), cookie) = register(&h.app, "op@corp.test", Some(token)).await;
    assert!(status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND, "{status} {body}");
    assert!(cookie.is_none());
    assert!(models_user::user_by_email(&h.app.state.control, "op@corp.test").await.unwrap().is_none());
    let p = invitations::by_hash(&h.app.state.control, &invitations::token_hash(token)).await.unwrap().unwrap();
    assert!(invitations::check_usable(&h.app.state.control, p).await.is_err(), "refused below the route too");
}

#[tokio::test]
async fn look_alike_addresses_cannot_be_invited_or_accept_an_invitation() {
    let h = host().await;
    for email in ["\u{212A}en@example.com", "\u{212B}sa@example.com"] {
        let (status, body) = as_cookie(&h.app, &h.alpha_cookie, "POST", "/api/org/invitations", Some(json!({ "email": email }))).await;
        assert_eq!((status, body["code"].as_str()), (StatusCode::BAD_REQUEST, Some("validation.invalid")), "{email}: {body}");
    }
    // an invitation for the plain address cannot be taken with a look-alike of it
    let token = invite(&h, "ken@example.com", "member").await["token"].as_str().unwrap().to_string();
    let ((status, body), _) = register(&h.app, "\u{212A}en@example.com", Some(&token)).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::BAD_REQUEST, Some("validation.invalid")), "{body}");
    assert!(models_user::user_by_email(&h.app.state.control, "ken@example.com").await.unwrap().is_none());
    assert_eq!(lookup(&h.app, &token).await.0, StatusCode::OK, "still open for the real address");
    let token = invite(&h, "\u{e5}sa@example.com", "member").await["token"].as_str().unwrap().to_string();
    let ((status, body), _) = register(&h.app, "\u{212B}sa@example.com", Some(&token)).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::BAD_REQUEST, Some("validation.invalid")), "{body}");
}

#[tokio::test]
async fn a_signup_that_fails_after_the_claim_leaves_the_invitation_open() {
    let h = host().await;
    let token = invite(&h, "pat@example.com", "member").await["token"].as_str().unwrap().to_string();
    *models_user::FAIL_SETUP_FOR.lock().unwrap() = Some("pat@example.com".into());
    let ((status, _), _) = register(&h.app, "pat@example.com", Some(&token)).await;
    *models_user::FAIL_SETUP_FOR.lock().unwrap() = None;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(models_user::user_by_email(&h.app.state.control, "pat@example.com").await.unwrap().is_none());
    assert_eq!(lookup(&h.app, &token).await.0, StatusCode::OK, "the invitation is open again");
    let ((status, body), _) = register(&h.app, "pat@example.com", Some(&token)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn creation_and_acceptance_are_rate_limited() {
    let h = host().await;
    // creation: one bucket per org (and per admin)
    for i in 0..eunomia_backend::ratelimit::INVITES_PER_MIN {
        invite(&h, &format!("p{i}@example.com"), "member").await;
    }
    let (status, body) = as_cookie(&h.app, &h.alpha_cookie, "POST", "/api/org/invitations", Some(json!({ "email": "one-more@example.com" }))).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::TOO_MANY_REQUESTS, Some("rate.limited")), "{body}");

    // links that do not check out: after INVITE_TOKEN_FAILS misses, even a good link is refused for a while
    let h2 = host().await;
    let good = invite(&h2, "pat@example.com", "member").await["token"].as_str().unwrap().to_string();
    for i in 0..eunomia_backend::ratelimit::INVITE_TOKEN_FAILS {
        assert_eq!(lookup(&h2.app, &format!("guess-{i}")).await.0, StatusCode::NOT_FOUND);
    }
    let (status, body) = lookup(&h2.app, &good).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::TOO_MANY_REQUESTS, Some("rate.limited")), "{body}");

    // and a signed-in account gets INVITE_ACCEPTS_PER_MIN attempts a minute, good link or not
    let h3 = host().await;
    let link = invite(&h3, "pat@example.com", "member").await["token"].as_str().unwrap().to_string();
    let beta = common::register_personal(&h3.app.state, "beta@example.com").await;
    let beta_cookie = login(&h3.app, &beta.email).await;
    for _ in 0..eunomia_backend::ratelimit::INVITE_ACCEPTS_PER_MIN {
        let (status, _) = as_cookie(&h3.app, &beta_cookie, "POST", "/api/invitations/accept", Some(json!({ "token": link }))).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "email mismatch, not a miss");
    }
    let (status, body) = as_cookie(&h3.app, &beta_cookie, "POST", "/api/invitations/accept", Some(json!({ "token": link }))).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::TOO_MANY_REQUESTS, Some("rate.limited")), "{body}");
}

/// A link a resend has just replaced cannot win: the claim only succeeds under the hash it started with.
#[tokio::test]
async fn a_link_replaced_by_a_resend_cannot_claim() {
    let h = host().await;
    let first = invite(&h, "pat@example.com", "member").await;
    let old = invitations::by_hash(&h.app.state.control, &invitations::token_hash(first["token"].as_str().unwrap())).await.unwrap().expect("open");
    // an acceptance with the old link got this far; meanwhile the admin resends
    let id = first["invitation"]["id"].as_str().unwrap();
    assert_eq!(as_cookie(&h.app, &h.alpha_cookie, "POST", &format!("/api/org/invitations/{id}/resend"), None).await.0, StatusCode::OK);
    let mut res = h.app.state.control.test_raw().query("CREATE user SET email = 'pat@example.com', email_lc = 'pat@example.com' RETURN VALUE id").await.unwrap();
    let pat: surrealdb::types::RecordId = res.take::<Vec<surrealdb::types::RecordId>>(0).unwrap().remove(0);
    let err = invitations::claim(&h.app.state.control, &pat, &old, true).await.unwrap_err();
    assert_eq!(err.code.as_str(), "invitation.not_found");
    assert!(memberships(&h.app.state, &pat).await.is_empty());
}

/// Every refusal at acceptance writes the same generic row to the inviting org's ledger, whatever the reason.
#[tokio::test]
async fn every_refusal_at_acceptance_reads_the_same_to_the_inviter() {
    let h = host().await;
    let to_pat = invite(&h, "pat@example.com", "member").await;
    let to_beta = invite(&h, "beta@example.com", "member").await;
    // another email, by password signup
    let ((status, _), _) = register(&h.app, "mallory@example.com", Some(to_pat["token"].as_str().unwrap())).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // an account in another organisation, signed in
    common::register_personal(&h.app.state, "beta@example.com").await;
    let beta = login(&h.app, "beta@example.com").await;
    assert_eq!(as_cookie(&h.app, &beta, "POST", "/api/invitations/accept", Some(json!({ "token": to_beta["token"] }))).await.0, StatusCode::CONFLICT);
    let rows = audit_rows(&h.app.state, "org.invite_accept").await;
    assert_eq!(rows.len(), 2, "{rows:?}");
    for r in &rows {
        assert_eq!((r["outcome"].as_str(), r["actor_kind"].as_str()), (Some(invitations::NOT_ACCEPTED), Some("anonymous")), "{r}");
        assert_eq!(r["user"], json!(h.alpha.id.to_string()));
        assert!(r["detail"].as_str().unwrap().starts_with("email=") && !r["detail"].as_str().unwrap().contains(' '), "only the invited address: {r}");
    }
}

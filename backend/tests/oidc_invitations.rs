//! Single sign-on through an organisation invitation link. Only the invitation whose token the person's
//! own sign-in carried (posted from the invitation page into the signed flow cookie) is ever used, and
//! only for the invited email; a sign-in without a link follows the host's normal sign-up rules, whatever
//! invitations exist for the address.
//!
//! The sign-up env (`SIGNUP`, `EUNOMIA_SIGNUP_ORG`, `EUNOMIA_ADMIN_EMAILS`) differs per test, so every
//! test holds `SERIAL` and sets it first.

mod common;
#[path = "common/oidc_keys.rs"]
mod keys;
#[path = "common/oidc_idp.rs"]
mod idp;

use idp::*;

use eunomia_backend::invitations::{self, OrgInviteRole};
use eunomia_backend::models_user::{self, User};

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn host(signup: &str, signup_org: &str) {
    // SAFETY: every test in this binary holds SERIAL while it sets and uses these
    unsafe {
        std::env::set_var("SIGNUP", signup);
        std::env::set_var("EUNOMIA_SIGNUP_ORG", signup_org);
        std::env::set_var("EUNOMIA_ADMIN_EMAILS", "op@corp.test");
        std::env::set_var("SIGNUP_ALLOWLIST", "");
        std::env::set_var("COOKIE_SECURE", "auto");
        std::env::set_var("RATE_LIMIT_AUTH_PER_MIN", "1000");
    }
}

fn set_env(k: &str, v: &str) {
    // SAFETY: as in `host`
    unsafe { std::env::set_var(k, v) };
}

/// An install whose first account is the operator's, plus the owners of org A and org B.
async fn world(mode: AuthMode, absent_verified: bool) -> (App, User, User) {
    let app = app_with(mode, absent_verified).await;
    // the install's first account (with operators listed, a first SSO account would have to be one of them)
    models_user::register_user_with(&app.state, "first@example.test", PASSWORD, true).await.unwrap();
    let a = models_user::register_user_with(&app.state, "owner-a@example.test", PASSWORD, true).await.unwrap();
    let b = models_user::register_user_with(&app.state, "owner-b@example.test", PASSWORD, true).await.unwrap();
    assert_ne!(a.org, b.org);
    (app, a, b)
}

fn as_pat(app: &App) {
    app.idp.behave(Behavior { sub: "pat-sub".into(), email: Some("pat@example.test".into()), ..Default::default() });
}

async fn org_of(app: &App, email: &str) -> Option<eunomia_backend::pool::OrgId> {
    models_user::user_by_email(&app.state.control, email).await.unwrap().map(|u| u.org)
}

async fn open(app: &App, token: &str) -> bool {
    match invitations::by_hash(&app.state.control, &invitations::token_hash(token)).await.unwrap() {
        Some(p) => invitations::check_usable(&app.state.control, p).await.is_ok(),
        None => false,
    }
}

#[tokio::test]
async fn two_organisations_invite_one_address_and_the_link_decides() {
    let _g = SERIAL.lock().await;
    host("closed", "personal");
    let (app, a, b) = world(AuthMode::Both, false).await;
    let from_a = invitations::create(&app.state, &a, "pat@example.test", OrgInviteRole::Member).await.unwrap();
    let from_b = invitations::create(&app.state, &b, "pat@example.test", OrgInviteRole::Admin).await.unwrap();
    // B resends after A: B's is the newest, which must not matter
    let b_id = from_b.invitation.id.clone();
    let from_b = invitations::resend(&app.state, &b, &b_id).await.unwrap();

    // the provider never sees the token: it is not in the URL the browser is sent to
    let start = app.router.clone();
    let (status, headers, _) = raw(&start, app.invited_start(&from_a.token, Some(FRONTEND_ORIGIN))).await;
    assert_eq!(status, axum::http::StatusCode::SEE_OTHER);
    let to_provider = headers[axum::http::header::LOCATION].to_str().unwrap();
    assert!(!to_provider.contains(&from_a.token) && !to_provider.contains(&invitations::token_hash(&from_a.token)), "{to_provider}");

    as_pat(&app);
    let landed = app.sign_in_invited(&from_a.token).await;
    expect_signed_in(&landed);
    assert_eq!(landed.location, "/onboarding", "a new member goes to onboarding, not back to the used link");
    assert_eq!(org_of(&app, "pat@example.test").await, Some(a.org), "the link's org, not the newest inviter");
    assert!(!open(&app, &from_a.token).await, "A's invitation is used up");
    assert!(open(&app, &from_b.token).await, "B's is untouched");
    let rows: Vec<Value> = app.state.control.test_raw().query("SELECT role FROM membership WHERE user.email = 'pat@example.test'").await.unwrap().take(0).unwrap();
    assert_eq!(rows, vec![json!({"role": "member"})], "A's role, not B's");
}

#[tokio::test]
async fn without_a_link_a_personal_host_gives_an_org_of_its_own_whatever_is_pending() {
    let _g = SERIAL.lock().await;
    host("open", "personal");
    let (app, a, b) = world(AuthMode::Both, false).await;
    let from_a = invitations::create(&app.state, &a, "pat@example.test", OrgInviteRole::Member).await.unwrap();
    let from_b = invitations::create(&app.state, &b, "pat@example.test", OrgInviteRole::Member).await.unwrap();
    as_pat(&app);
    expect_signed_in(&app.sign_in().await);
    let org = org_of(&app, "pat@example.test").await.unwrap();
    assert!(org != a.org && org != b.org, "an org of its own, as before invitations existed");
    assert!(open(&app, &from_a.token).await && open(&app, &from_b.token).await, "no invitation was used by email");
}

#[tokio::test]
async fn without_a_link_a_closed_host_refuses_whatever_is_pending() {
    let _g = SERIAL.lock().await;
    host("closed", "personal");
    let (app, a, _) = world(AuthMode::Both, false).await;
    let from_a = invitations::create(&app.state, &a, "pat@example.test", OrgInviteRole::Member).await.unwrap();
    as_pat(&app);
    expect_refused(&app.sign_in().await, "signup_closed");
    assert!(org_of(&app, "pat@example.test").await.is_none());
    assert!(open(&app, &from_a.token).await);
}

#[tokio::test]
async fn a_link_only_works_for_its_own_email_and_while_it_is_open() {
    let _g = SERIAL.lock().await;
    host("closed", "personal");
    let (app, a, _) = world(AuthMode::Both, false).await;
    // someone else's link
    let for_sam = invitations::create(&app.state, &a, "sam@example.test", OrgInviteRole::Member).await.unwrap();
    as_pat(&app);
    expect_refused(&app.sign_in_invited(&for_sam.token).await, "invitation_email");
    assert!(org_of(&app, "pat@example.test").await.is_none());
    assert!(open(&app, &for_sam.token).await);
    // the inviting org's ledger says only that it was not accepted
    let rows: Vec<Value> = app.state.control.test_raw().query("SELECT user, outcome FROM audit_event WHERE action = 'org.invite_accept'").await.unwrap().take(0).unwrap();
    assert_eq!(rows, vec![json!({"user": a.id.to_string(), "outcome": invitations::NOT_ACCEPTED})]);
    // withdrawn
    let for_pat = invitations::create(&app.state, &a, "pat@example.test", OrgInviteRole::Member).await.unwrap();
    invitations::revoke(&app.state, &a, &for_pat.invitation.id).await.unwrap();
    expect_refused(&app.sign_in_invited(&for_pat.token).await, "invitation");
    // expired: refused, and the inviting org's ledger has it
    let again = invitations::create(&app.state, &a, "pat@example.test", OrgInviteRole::Member).await.unwrap();
    app.state.control.test_raw().query("UPDATE org_invitation SET expires_at = time::now() - 1s WHERE email = 'pat@example.test'").await.unwrap().check().unwrap();
    expect_refused(&app.sign_in_invited(&again.token).await, "invitation_expired");
    let rows: Vec<Value> = app.state.control.test_raw().query("SELECT user FROM audit_event WHERE action = 'org.invite_expired'").await.unwrap().take(0).unwrap();
    assert_eq!(rows, vec![json!({"user": a.id.to_string()})]);
    // a made-up token
    expect_refused(&app.sign_in_invited("not-a-real-invitation-token").await, "invitation");
    assert!(org_of(&app, "pat@example.test").await.is_none());
}

#[tokio::test]
async fn the_start_only_takes_an_invitation_from_this_site() {
    let _g = SERIAL.lock().await;
    host("closed", "personal");
    let (app, a, _) = world(AuthMode::Both, false).await;
    let link = invitations::create(&app.state, &a, "pat@example.test", OrgInviteRole::Member).await.unwrap();
    for origin in [None, Some("https://evil.example")] {
        let (status, headers, _) = raw(&app.router, app.invited_start(&link.token, origin)).await;
        assert_eq!(status, axum::http::StatusCode::FORBIDDEN, "{origin:?}");
        assert!(headers.get(axum::http::header::SET_COOKIE).is_none());
    }
}

#[tokio::test]
async fn an_operator_address_is_never_created_through_an_invitation_by_sso() {
    let _g = SERIAL.lock().await;
    host("closed", "personal");
    let (app, a, _) = world(AuthMode::Both, false).await;
    assert!(invitations::create(&app.state, &a, "op@corp.test", OrgInviteRole::Member).await.is_err(), "not invitable");
    // one that got in some other way (planted)
    let token = "planted-operator-invitation-token-0123456789abcdef";
    app.state.control.test_raw()
        .query("CREATE org_invitation SET org = $org, email = 'op@corp.test', role = 'member', token_hash = $h, invited_by = $by, expires_at = time::now() + 7d, slot = string::concat(<string>$org, '/op@corp.test')")
        .bind(("org", a.org.record()))
        .bind(("h", invitations::token_hash(token)))
        .bind(("by", a.id.clone()))
        .await.unwrap().check().unwrap();
    app.idp.behave(Behavior { sub: "op-sub".into(), email: Some("op@corp.test".into()), ..Default::default() });
    expect_refused(&app.sign_in_invited(token).await, "invitation");
    assert!(org_of(&app, "op@corp.test").await.is_none());
}

#[tokio::test]
async fn look_alike_letters_cannot_take_an_invitation_by_sso() {
    let _g = SERIAL.lock().await;
    host("closed", "personal");
    let (app, a, _) = world(AuthMode::Both, false).await;
    let link = invitations::create(&app.state, &a, "ken@example.test", OrgInviteRole::Member).await.unwrap();
    for (sub, email) in [("k-sub", "\u{212A}en@example.test"), ("a-sub", "\u{212B}ke@example.test")] {
        app.idp.behave(Behavior { sub: sub.into(), email: Some(email.into()), ..Default::default() });
        expect_refused(&app.sign_in_invited(&link.token).await, "no_email");
    }
    assert!(org_of(&app, "ken@example.test").await.is_none());
    assert!(open(&app, &link.token).await);
}

#[tokio::test]
async fn an_unverified_email_never_uses_an_invitation_and_a_missing_claim_follows_the_setting() {
    let _g = SERIAL.lock().await;
    host("closed", "personal");
    // the provider says false: refused, setting or not
    for absent_verified in [false, true] {
        let (app, a, _) = world(AuthMode::Both, absent_verified).await;
        let link = invitations::create(&app.state, &a, "pat@example.test", OrgInviteRole::Member).await.unwrap();
        app.idp.behave(Behavior { sub: "pat-sub".into(), email: Some("pat@example.test".into()), email_verified: json!(false), ..Default::default() });
        expect_refused(&app.sign_in_invited(&link.token).await, "invitation_unverified");
        assert!(org_of(&app, "pat@example.test").await.is_none(), "absent_verified={absent_verified}");
    }
    // the provider sends no claim (Entra ID): refused by default, accepted with OIDC_EMAIL_VERIFIED_WHEN_ABSENT
    for (absent_verified, joins) in [(false, false), (true, true)] {
        let (app, a, _) = world(AuthMode::Both, absent_verified).await;
        let link = invitations::create(&app.state, &a, "pat@example.test", OrgInviteRole::Member).await.unwrap();
        app.idp.behave(Behavior { sub: "pat-sub".into(), email: Some("pat@example.test".into()), email_verified: Value::Null, ..Default::default() });
        let landed = app.sign_in_invited(&link.token).await;
        if joins {
            expect_signed_in(&landed);
            assert_eq!(org_of(&app, "pat@example.test").await, Some(a.org));
        } else {
            expect_refused(&landed, "invitation_unverified");
            assert!(org_of(&app, "pat@example.test").await.is_none());
        }
    }
}

#[tokio::test]
async fn an_existing_account_signing_in_through_a_link_keeps_its_org_and_the_link_stays_open() {
    let _g = SERIAL.lock().await;
    host("closed", "personal");
    let (app, a, b) = world(AuthMode::Both, false).await;
    // B's owner was invited to A. Their account has a password, so single sign-on asks before linking
    // (and would link to the account in B either way); nothing joins A
    let link = invitations::create(&app.state, &a, "owner-b@example.test", OrgInviteRole::Member).await.unwrap();
    app.idp.behave(Behavior { sub: "b-sub".into(), email: Some("owner-b@example.test".into()), ..Default::default() });
    let landed = app.sign_in_invited(&link.token).await;
    assert_eq!((landed.location.as_str(), landed.session.is_none()), ("/login/link", true));
    let ((status, _), _) = http(&app.router, "POST", "/api/auth/oidc/link", Some(json!({"password": PASSWORD})), None, Some(&landed.link.unwrap())).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(org_of(&app, "owner-b@example.test").await, Some(b.org));
    assert!(open(&app, &link.token).await, "accepting is a separate, confirmed step on the invitation page");
}

/// The invitation-only setting never makes a missing claim count for anything but the invitation that
/// came with the sign-in: not for linking (to a password or an SSO-only account), not for an operator
/// address, not for the allowlist.
#[tokio::test]
async fn the_unverified_invitation_setting_trusts_nothing_else() {
    let _g = SERIAL.lock().await;
    host("closed", "personal");
    let missing = |sub: &str, email: &str| Behavior { sub: sub.into(), email: Some(email.into()), email_verified: Value::Null, ..Default::default() };
    let (app, _, _) = world(AuthMode::Both, true).await;
    // an existing password account: no link, and no take-over page either
    models_user::register_user_with(&app.state, "pw@example.test", PASSWORD, true).await.unwrap();
    app.idp.behave(missing("pw-sub", "pw@example.test"));
    let landed = app.sign_in().await;
    assert!(landed.link.is_none());
    expect_refused(&landed, "email_unverified");
    // an existing account made by single sign-on: no link to a second identity
    app.idp.behave(Behavior { sub: "sso-1".into(), email: Some("sso@example.test".into()), ..Default::default() });
    host("open", "personal");
    expect_signed_in(&app.sign_in().await);
    app.idp.behave(missing("sso-2", "sso@example.test"));
    expect_refused(&app.sign_in().await, "email_unverified");
    // an operator address with no account
    host("closed", "personal");
    app.idp.behave(missing("op-sub", "op@corp.test"));
    expect_refused(&app.sign_in().await, "signup_closed");
    assert!(org_of(&app, "op@corp.test").await.is_none());
    // the SIGNUP=invite allowlist
    host("invite", "personal");
    set_env("SIGNUP_ALLOWLIST", "listed@example.test");
    app.idp.behave(missing("listed-sub", "listed@example.test"));
    expect_refused(&app.sign_in().await, "signup_closed");
    assert!(org_of(&app, "listed@example.test").await.is_none());
    // the control: the same allowlisted address with the claim sent as true signs up
    app.idp.behave(Behavior { sub: "listed-sub".into(), email: Some("listed@example.test".into()), ..Default::default() });
    expect_signed_in(&app.sign_in().await);
}

/// The inviter registers the invitee's address with a password of their own (the token lets them),
/// mints a token and keeps the account in their organisation. The real person arrives through a link by
/// single sign-on: asked first, they start fresh, which sets the squatted account aside and makes them a new
/// account through the invitation, in the inviting organisation; the squatter's credentials stop working.
#[tokio::test]
async fn a_squatted_invitation_ends_with_the_real_person_in_a_fresh_account() {
    let _g = SERIAL.lock().await;
    host("closed", "personal");
    let (app, a, _) = world(AuthMode::Both, false).await;
    let squat_link = invitations::create(&app.state, &a, "pat@example.test", OrgInviteRole::Member).await.unwrap();
    let ((status, _), _) = http(&app.router, "POST", "/api/auth/register", Some(json!({"email": "pat@example.test", "password": "squatter-password", "invitation": squat_link.token})), None, None).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let squatted = models_user::user_by_email(&app.state.control, "pat@example.test").await.unwrap().unwrap();
    let token = models_user::create_api_token_with(&app.state.control, &squatted.id, "squatter", &eunomia_backend::scopes::ALL.iter().map(|s| s.to_string()).collect::<Vec<_>>(), None, None).await.unwrap().token;

    // a fresh invitation for the real person (the first one was used up by the squatter)
    let link = invitations::create(&app.state, &a, "pat2@example.test", OrgInviteRole::Member).await.unwrap();
    app.state.control.test_raw().query("UPDATE org_invitation SET email = 'pat@example.test', slot = string::concat(<string>org, '/pat@example.test') WHERE email = 'pat2@example.test'").await.unwrap().check().unwrap();
    as_pat(&app);
    let landed = app.sign_in_invited(&link.token).await;
    assert_eq!(landed.location, "/login/link", "asked first, not linked");
    assert!(landed.session.is_none());
    let ((status, done), set) = http(&app.router, "POST", "/api/auth/oidc/link", Some(json!({"start_fresh": true})), None, Some(&landed.link.unwrap())).await;
    assert_eq!((status, done["started_fresh"].as_bool()), (axum::http::StatusCode::OK, Some(true)), "{done}");
    assert!(set.is_some());

    // a fresh account, in the inviting organisation, through the invitation
    let fresh = models_user::user_by_email(&app.state.control, "pat@example.test").await.unwrap().unwrap();
    assert_ne!(fresh.id, squatted.id);
    assert_eq!(fresh.org, a.org);
    assert!(!open(&app, &link.token).await, "the invitation was used by the fresh account");
    // and the squatter's credentials are dead
    let login = http(&app.router, "POST", "/api/auth/login", Some(json!({"email": "pat@example.test", "password": "squatter-password"})), None, None).await.0 .0;
    assert_eq!(login, axum::http::StatusCode::UNAUTHORIZED, "the squatter's password");
    assert_ne!(http(&app.router, "GET", "/api/vaults", None, Some(&token), None).await.0 .0, axum::http::StatusCode::OK, "and their token");
    assert!(models_user::is_suspended(&app.state.control, &squatted.id).await.unwrap());
}

/// Served over https, the sign-in cookie is `__Host-` (Path=/, Secure): one another host planted under the
/// plain name is not read.
#[tokio::test]
async fn on_https_the_sign_in_cookie_is_host_locked() {
    let _g = SERIAL.lock().await;
    host("open", "personal");
    set_env("COOKIE_SECURE", "true");
    let app = app_with(AuthMode::Both, false).await;
    let (status, headers, _) = raw(&app.router, axum::http::Request::get("/api/auth/oidc/start?next=/").body(axum::body::Body::empty()).unwrap()).await;
    assert_eq!(status, axum::http::StatusCode::SEE_OTHER);
    let set = headers.get(axum::http::header::SET_COOKIE).unwrap().to_str().unwrap().to_string();
    assert!(set.starts_with("__Host-eunomia_oidc=") && set.contains("; Path=/;") && set.ends_with("; Secure"), "{set}");
    // the same signed value under the plain name (as a sibling host could set it) is not a flow
    let value = set.split(';').next().unwrap().strip_prefix("__Host-eunomia_oidc=").unwrap().to_string();
    let req = axum::http::Request::get("/api/auth/oidc/callback?code=x&state=y").header(axum::http::header::COOKIE, format!("eunomia_oidc={value}")).body(axum::body::Body::empty()).unwrap();
    let (_, headers, _) = raw(&app.router, req).await;
    assert!(headers[axum::http::header::LOCATION].to_str().unwrap().contains("oidc_error=state"));
    set_env("COOKIE_SECURE", "auto");
}

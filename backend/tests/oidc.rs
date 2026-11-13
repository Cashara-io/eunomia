//! Sign-in through an OpenID Connect provider, end to end: the real router and an in-memory SurrealDB against
//! an in-test mock provider (axum on 127.0.0.1:0) that serves discovery, a JWKS, `/authorize` and `/token`
//! and signs ID tokens with an ES256 test key. Nothing leaves the machine.

mod common;
#[path = "common/oidc_keys.rs"]
mod keys;

#[path = "common/oidc_idp.rs"]
mod idp;

use idp::*;
// -- tests -------------------------------------------------------------------------------------

#[tokio::test]
async fn first_sign_in_creates_the_admin_with_a_personal_vault_and_a_session() {
    let app = app(AuthMode::Oidc).await;
    let landed = app.sign_in_with("/consent?client_id=x", Tamper::default()).await;
    let cookie = expect_signed_in(&landed);
    // a new account goes through onboarding, then on to where it was headed
    assert_eq!(landed.path(), "/onboarding?next=%2Fconsent%3Fclient_id%3Dx");
    assert_eq!(app.user_count().await, 1);

    let (status, me) = app.get_as(&cookie, "/api/auth/me").await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["email"], "alice@example.test");
    assert_eq!(me["has_password"], false);
    assert_eq!(me["oidc_provider"], "Mock IdP");

    let (_, vaults) = app.get_as(&cookie, "/api/vaults").await;
    let list = vaults.as_array().or(vaults["results"].as_array()).expect("vault list");
    assert_eq!(list.len(), 1, "{vaults}");
    assert_eq!(list[0]["kind"], "personal", "{vaults}");

    // the first user of an install is its admin
    let user = models_user::user_by_email(&app.state.control, "alice@example.test").await.unwrap().unwrap();
    assert!(eunomia_backend::authz::is_admin(&app.state.control, &user).await.unwrap());

    // the ledger has the signup and the login
    let mut res = app.state.control.test_raw().query("SELECT action, outcome FROM audit_event WHERE action IN ['auth.register', 'auth.oidc_login'] ORDER BY created_at").await.unwrap();
    let rows: Vec<Value> = res.take(0).unwrap();
    assert_eq!(rows.iter().map(|r| r["action"].as_str().unwrap()).collect::<Vec<_>>(), ["auth.register", "auth.oidc_login"]);
}

#[tokio::test]
async fn the_second_sign_in_maps_to_the_same_account() {
    let app = app(AuthMode::Both).await;
    let first = expect_signed_in(&app.sign_in().await);
    let (_, me1) = app.get_as(&first, "/api/auth/me").await;

    let second = app.sign_in_with("/vaults", Tamper::default()).await;
    let cookie = expect_signed_in(&second);
    assert_eq!(second.path(), "/vaults", "an existing account goes straight to next");
    let (_, me2) = app.get_as(&cookie, "/api/auth/me").await;
    assert_eq!(me1["id"], me2["id"]);
    assert_eq!(app.user_count().await, 1);

    // a different person is a different account, and not the admin
    app.idp.behave(Behavior { sub: "bob-sub".into(), email: Some("Bob@Example.test".into()), ..Default::default() });
    let bob = expect_signed_in(&app.sign_in().await);
    let (_, me3) = app.get_as(&bob, "/api/auth/me").await;
    assert_ne!(me3["id"], me1["id"]);
    assert_eq!(me3["email"], "bob@example.test");
    assert_eq!(app.user_count().await, 2);
    let bob_user = models_user::user_by_email(&app.state.control, "bob@example.test").await.unwrap().unwrap();
    assert!(!eunomia_backend::authz::is_admin(&app.state.control, &bob_user).await.unwrap());
}

#[tokio::test]
async fn an_oidc_account_cannot_log_in_with_a_password() {
    let app = app(AuthMode::Both).await;
    expect_signed_in(&app.sign_in().await);
    for guess in [PASSWORD, "", "x", "no password"] {
        let (status, body) = http(&app.router, "POST", "/api/auth/login", Some(json!({"email": "alice@example.test", "password": guess})), None, None).await.0;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    }
}

#[tokio::test]
async fn a_token_that_does_not_validate_is_refused() {
    let app = app(AuthMode::Both).await;
    let cases: Vec<(&str, Behavior, &str)> = vec![
        ("wrong audience", Behavior { aud: json!("someone-else"), ..Default::default() }, "token"),
        ("audience list without us", Behavior { aud: json!(["a", "b"]), ..Default::default() }, "token"),
        ("several audiences, no azp", Behavior { aud: json!([CLIENT_ID, "other"]), ..Default::default() }, "token"),
        ("azp is someone else", Behavior { azp: Some("other".into()), ..Default::default() }, "token"),
        ("wrong issuer", Behavior { iss: Some("https://evil.example".into()), ..Default::default() }, "token"),
        ("expired", Behavior { exp_in: -3600, ..Default::default() }, "token"),
        ("signed by another key", Behavior { key: "b", ..Default::default() }, "token"),
        ("unknown kid", Behavior { kid: Some("nope".into()), ..Default::default() }, "token"),
        ("nonce mismatch", Behavior { nonce: Some("replayed-nonce".into()), ..Default::default() }, "token"),
        ("alg none", Behavior { alg: "none", ..Default::default() }, "token"),
        ("alg HS256", Behavior { alg: "HS256", ..Default::default() }, "token"),
    ];
    for (name, b, code) in cases {
        app.idp.behave(b);
        let landed = app.sign_in().await;
        assert_eq!(landed.error().as_deref(), Some(code), "{name}: {}", landed.location);
        assert!(landed.session.is_none(), "{name}");
    }
    assert_eq!(app.user_count().await, 0, "nothing was created by any refused token");
    // and the failures are in the ledger, without the provider's token
    let mut res = app.state.control.test_raw().query("SELECT detail FROM audit_event WHERE action = 'auth.oidc_login' AND outcome != 'ok'").await.unwrap();
    let rows: Vec<Value> = res.take(0).unwrap();
    assert_eq!(rows.len(), 11);
    // a good token still works afterwards
    app.idp.behave(Behavior::default());
    expect_signed_in(&app.sign_in().await);
}

#[tokio::test]
async fn state_cookie_and_provider_errors_are_refused() {
    let app = app(AuthMode::Both).await;
    expect_refused(&app.sign_in_with("/", Tamper { state: Some("forged"), ..Default::default() }).await, "state");
    // a callback from another browser has no flow cookie (login CSRF)
    expect_refused(&app.sign_in_with("/", Tamper { drop_cookie: true, ..Default::default() }).await, "state");
    expect_refused(&app.sign_in_with("/", Tamper { error: Some("access_denied"), ..Default::default() }).await, "denied");
    // a callback with a code the provider never issued
    let (_, headers, _) = raw(&app.router, Request::get("/api/auth/oidc/start").body(Body::empty()).unwrap()).await;
    let flow = cookie_pairs(&headers).remove(0);
    let url = reqwest::Url::parse(headers[header::LOCATION].to_str().unwrap()).unwrap();
    let state = url.query_pairs().find(|(k, _)| k == "state").unwrap().1.to_string();
    let uri = format!("/api/auth/oidc/callback?{}", serde_urlencoded::to_string([("code", "made-up"), ("state", state.as_str())]).unwrap());
    let (status, headers, _) = raw(&app.router, Request::get(uri).header(header::COOKIE, flow).body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(headers[header::LOCATION].to_str().unwrap().contains("oidc_error=provider"));
    // a response naming another issuer (RFC 9207)
    let uri = format!("/api/auth/oidc/callback?{}", serde_urlencoded::to_string([("code", "x"), ("state", state.as_str()), ("iss", "https://evil.example")]).unwrap());
    let (_, headers, _) = raw(&app.router, Request::get(uri).header(header::COOKIE, cookie_pairs(&raw(&app.router, Request::get("/api/auth/oidc/start").body(Body::empty()).unwrap()).await.1).remove(0)).body(Body::empty()).unwrap()).await;
    assert!(headers[header::LOCATION].to_str().unwrap().contains("oidc_error="));
    assert_eq!(app.user_count().await, 0);
}

#[tokio::test]
async fn a_code_cannot_be_replayed_and_a_stolen_flow_cookie_is_single_use_at_the_provider() {
    let app = app(AuthMode::Both).await;
    // run the flow by hand so the same callback can be sent twice
    let (_, headers, _) = raw(&app.router, Request::get("/api/auth/oidc/start").body(Body::empty()).unwrap()).await;
    let flow = cookie_pairs(&headers).remove(0);
    let callback = at_provider(headers[header::LOCATION].to_str().unwrap()).await;
    let path = callback.strip_prefix("http://localhost:8001").unwrap().to_string();
    let go = || raw(&app.router, Request::get(path.clone()).header(header::COOKIE, flow.clone()).body(Body::empty()).unwrap());
    let (_, h1, _) = go().await;
    assert!(!h1[header::LOCATION].to_str().unwrap().contains("oidc_error"));
    let (_, h2, _) = go().await;
    assert!(h2[header::LOCATION].to_str().unwrap().contains("oidc_error=provider"), "the provider refuses a reused code");
    assert_eq!(app.user_count().await, 1);
}

#[tokio::test]
async fn an_unverified_email_does_not_take_over_a_password_account() {
    let app = app(AuthMode::Both).await;
    let victim = common::register(&app.state, "alice@example.test").await;
    app.idp.behave(Behavior { email_verified: json!(false), ..Default::default() });
    expect_refused(&app.sign_in().await, "email_unverified");
    // "email_verified" as a string "false" is no better; a missing claim neither
    app.idp.behave(Behavior { email_verified: json!("false"), ..Default::default() });
    expect_refused(&app.sign_in().await, "email_unverified");
    app.idp.behave(Behavior { email_verified: Value::Null, ..Default::default() });
    expect_refused(&app.sign_in().await, "email_unverified");
    let issuer = app.idp.inner.lock().unwrap().issuer.clone();
    assert!(models_user::user_by_identity(&app.state.control, &issuer, "alice-sub").await.unwrap().is_none());
    assert_eq!(app.user_count().await, 1);

    // an unverified email with no existing account may still make a new one (the provider vouches for the person, not the address)
    app.idp.behave(Behavior { sub: "carol-sub".into(), email: Some("carol@example.test".into()), email_verified: json!(false), ..Default::default() });
    expect_signed_in(&app.sign_in().await);

    // verified: it links to the password account, which keeps its password
    let cookie_before = {
        let (_, set) = http(&app.router, "POST", "/api/auth/login", Some(json!({"email": "alice@example.test", "password": PASSWORD})), None, None).await;
        set.unwrap().split(';').next().unwrap().to_string()
    };
    assert_eq!(app.get_as(&cookie_before, "/api/auth/me").await.0, StatusCode::OK);
    app.idp.behave(Behavior { email_verified: json!(true), ..Default::default() });
    let cookie = expect_signed_in(&app.sign_in().await);
    let (_, me) = app.get_as(&cookie, "/api/auth/me").await;
    assert_eq!(me["id"], victim.id.to_string());
    assert_eq!(me["email"], "alice@example.test");
    assert_eq!(me["has_password"], true);
    assert_eq!(me["oidc_provider"], "Mock IdP");
    assert_eq!(app.user_count().await, 2);
    // linking signed the account's earlier browser sessions out
    assert_eq!(app.get_as(&cookie_before, "/api/auth/me").await.0, StatusCode::UNAUTHORIZED);
    // the password still works
    let (status, _) = http(&app.router, "POST", "/api/auth/login", Some(json!({"email": "alice@example.test", "password": PASSWORD})), None, None).await.0;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn provider_only_mode_turns_the_password_routes_off() {
    let app = app(AuthMode::Oidc).await;
    for (path, body) in [("/api/auth/login", json!({"email": "a@b.co", "password": PASSWORD})), ("/api/auth/register", json!({"email": "a@b.co", "password": PASSWORD}))] {
        let (status, problem) = http(&app.router, "POST", path, Some(body), None, None).await.0;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
        assert_eq!(problem["code"], "auth.password_disabled", "{problem}");
    }
    assert_eq!(app.user_count().await, 0);
    let (_, boot) = http(&app.router, "GET", "/api/auth/bootstrap", None, None, None).await.0;
    assert_eq!((boot["auth_mode"].as_str(), boot["oidc_provider"].as_str(), boot["has_users"].as_bool()), (Some("oidc"), Some("Mock IdP"), Some(false)));
    // the provider still works
    expect_signed_in(&app.sign_in().await);
    let (_, boot) = http(&app.router, "GET", "/api/auth/bootstrap", None, None, None).await.0;
    assert_eq!(boot["has_users"], true);
}

#[tokio::test]
async fn both_mode_keeps_passwords_and_password_mode_has_no_oidc_routes() {
    let both = app(AuthMode::Both).await;
    let (status, _) = http(&both.router, "POST", "/api/auth/register", Some(json!({"email": "pw@example.test", "password": PASSWORD})), None, None).await.0;
    assert_eq!(status, StatusCode::OK);
    let (_, boot) = http(&both.router, "GET", "/api/auth/bootstrap", None, None, None).await.0;
    assert_eq!(boot["auth_mode"], "both");

    // password mode (today's behaviour): the OIDC routes answer 404 and the bootstrap shows no provider
    let state = AppState::build(&test_settings(), engine_config()).await.unwrap();
    let router = eunomia_backend::app(state);
    let (status, problem) = http(&router, "GET", "/api/auth/oidc/start", None, None, None).await.0;
    assert_eq!(status, StatusCode::NOT_FOUND, "{problem}");
    let (_, boot) = http(&router, "GET", "/api/auth/bootstrap", None, None, None).await.0;
    assert_eq!((boot["auth_mode"].as_str(), boot["oidc_provider"].is_null()), (Some("password"), true));
}

#[tokio::test]
async fn next_is_kept_through_the_round_trip_and_never_leaves_the_site() {
    let app = app(AuthMode::Both).await;
    expect_signed_in(&app.sign_in().await);
    for bad in ["//evil.example/x", "https://evil.example", "/\\evil.example", "javascript:alert(1)"] {
        let l = app.sign_in_with(bad, Tamper::default()).await;
        expect_signed_in(&l);
        assert_eq!(l.location, "/", "{bad}");
    }
    let l = app.sign_in_with("/consent?client_id=abc&state=a%20b", Tamper::default()).await;
    assert_eq!(l.location, "/consent?client_id=abc&state=a%20b");
}

#[tokio::test]
async fn signing_keys_rotate_without_a_restart() {
    let app = app(AuthMode::Both).await;
    app.state.oidc.set_refetch_cooldown(std::time::Duration::from_secs(3600));
    expect_signed_in(&app.sign_in().await);
    let hits = app.idp.jwks_hits();
    assert_eq!(app.idp.jwks_hits(), hits);

    // the provider rotates: key-c replaces key-a. Inside the cooldown a token with an unknown kid does not refetch...
    app.idp.serve_keys(vec![("key-c", "c")]);
    app.idp.behave(Behavior { key: "c", kid: Some("key-c".into()), ..Default::default() });
    expect_refused(&app.sign_in().await, "token");
    assert_eq!(app.idp.jwks_hits(), hits, "an unknown kid inside the cooldown cannot make the server fetch");
    // ...and once the cooldown allows it, the new key is picked up
    app.state.oidc.set_refetch_cooldown(std::time::Duration::ZERO);
    expect_signed_in(&app.sign_in().await);
    assert_eq!(app.idp.jwks_hits(), hits + 1);
    // key-a is gone from the set now: its tokens are refused
    app.idp.behave(Behavior::default());
    expect_refused(&app.sign_in().await, "token");
}

#[tokio::test]
async fn the_email_can_come_from_userinfo_when_the_id_token_has_none() {
    let app = app(AuthMode::Both).await;
    app.idp.behave(Behavior { email: None, ..Default::default() });
    expect_refused(&app.sign_in().await, "no_email");
    app.idp.inner.lock().unwrap().userinfo = Some(json!({"sub": "someone-else", "email": "mallory@example.test", "email_verified": true}));
    expect_refused(&app.sign_in().await, "no_email");
    app.idp.inner.lock().unwrap().userinfo = Some(json!({"sub": "alice-sub", "email": "alice@example.test", "email_verified": true}));
    let cookie = expect_signed_in(&app.sign_in().await);
    assert_eq!(app.get_as(&cookie, "/api/auth/me").await.1["email"], "alice@example.test");
}

#[tokio::test]
async fn an_mcp_client_signs_in_through_the_provider_and_gets_a_working_token() {
    let app = app(AuthMode::Oidc).await;
    // 1. the client registers and sends the browser to /oauth/authorize
    const REDIRECT: &str = "http://127.0.0.1:7777/callback";
    const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    let (status, reg) = http(&app.router, "POST", "/oauth/register", Some(json!({"client_name": "Agent", "redirect_uris": [REDIRECT], "token_endpoint_auth_method": "none"})), None, None).await.0;
    assert_eq!(status, StatusCode::CREATED, "{reg}");
    let client_id = reg["client_id"].as_str().unwrap();
    let authz = [
        ("response_type", "code"), ("client_id", client_id), ("redirect_uri", REDIRECT), ("state", "st-1"),
        ("code_challenge", &URL_SAFE_NO_PAD.encode(Sha256::digest(VERIFIER.as_bytes()))), ("code_challenge_method", "S256"),
        ("resource", "http://localhost:8001/mcp"),
    ];
    let q = serde_urlencoded::to_string(authz).unwrap();
    let (status, headers, _) = raw(&app.router, Request::get(format!("/oauth/authorize?{q}")).body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    // 2. not signed in: the login page, remembering the consent URL
    let login = headers[header::LOCATION].to_str().unwrap().to_string();
    let login_url = reqwest::Url::parse(&login).unwrap();
    assert_eq!(login_url.path(), "/login");
    let next = login_url.query_pairs().find(|(k, _)| k == "next").unwrap().1.to_string();
    assert!(next.starts_with("/consent?"), "{next}");
    // 3. the login page's "Continue with" button goes to the provider and back, carrying next
    let landed = app.sign_in_with(&next, Tamper::default()).await;
    let cookie = expect_signed_in(&landed);
    assert!(landed.path().starts_with("/onboarding?next=%2Fconsent"), "{}", landed.location);
    // 4. consent, as the page does it
    let consent_q = next.strip_prefix("/consent?").unwrap();
    let (status, info) = app.get_as(&cookie, &format!("/api/oauth/consent?{consent_q}")).await;
    assert_eq!(status, StatusCode::OK, "{info}");
    let mut params: HashMap<String, String> = serde_urlencoded::from_str(consent_q).unwrap();
    params.insert("approve".into(), "true".into());
    let body: Value = serde_json::to_value(&params).unwrap();
    let body = {
        let mut b = body;
        b["approve"] = json!(true);
        b
    };
    let (status, decided) = http(&app.router, "POST", "/api/oauth/consent", Some(body), None, Some(&cookie)).await.0;
    assert_eq!(status, StatusCode::OK, "{decided}");
    let cb = reqwest::Url::parse(decided["redirect_to"].as_str().unwrap()).unwrap();
    let code = cb.query_pairs().find(|(k, _)| k == "code").unwrap().1.to_string();
    // 5. the code buys a token that works on /mcp as the OIDC user
    let body = serde_urlencoded::to_string([("grant_type", "authorization_code"), ("code", code.as_str()), ("redirect_uri", REDIRECT), ("client_id", client_id), ("code_verifier", VERIFIER), ("resource", "http://localhost:8001/mcp")]).unwrap();
    let (status, _, bytes) = raw(&app.router, Request::post("/oauth/token").header(header::CONTENT_TYPE, "application/x-www-form-urlencoded").body(Body::from(body)).unwrap()).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&bytes));
    let access = serde_json::from_slice::<Value>(&bytes).unwrap()["access_token"].as_str().unwrap().to_string();
    let rpc = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "vault_list", "arguments": {}}});
    let (status, mcp) = http(&app.router, "POST", "/mcp", Some(rpc), Some(&access), None).await.0;
    assert_eq!(status, StatusCode::OK, "{mcp}");
    assert!(mcp.to_string().contains("alice@example.test") || mcp.to_string().contains("personal") || mcp.to_string().contains("Personal"), "{mcp}");
    assert!(app.idp.token_hits() >= 1);
}

#[tokio::test]
async fn a_provider_email_written_with_look_alike_letters_makes_no_account() {
    let app = app(AuthMode::Both).await;
    expect_signed_in(&app.sign_in().await);
    for (sub, email) in [("kelvin-sub", "\u{212A}ate@example.test"), ("angstrom-sub", "\u{212B}sa@example.test")] {
        app.idp.behave(Behavior { sub: sub.into(), email: Some(email.into()), ..Default::default() });
        expect_refused(&app.sign_in().await, "no_email");
    }
    assert_eq!(app.user_count().await, 1);
    assert!(models_user::user_by_email(&app.state.control, "kate@example.test").await.unwrap().is_none());
}

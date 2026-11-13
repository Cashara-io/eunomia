//! Reserved and allowlisted addresses are compared in the form accounts are stored in. An address
//! written with a character that lowercases into another letter (U+212A KELVIN SIGN into `k`, U+212B
//! ANGSTROM SIGN into `\u{e5}`) is refused at signup, so it can never be stored as a listed address.
//! The operator list is set once for the whole binary.

mod common;

use axum::http::StatusCode;
use eunomia_backend::models_user;
use serde_json::json;

fn env() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // SAFETY: set once, before any test reads it, to the same value for every test in this binary
        unsafe {
            std::env::set_var("EUNOMIA_ADMIN_EMAILS", "kate@corp.com, \u{e5}sa@corp.com");
            std::env::set_var("RATE_LIMIT_AUTH_PER_MIN", "1000");
        }
    });
}

async fn register(app: &common::TestApp, email: &str) -> (StatusCode, serde_json::Value) {
    common::http(&app.router, "POST", "/api/auth/register", Some(json!({ "email": email, "password": common::PASSWORD })), None, None).await.0
}

#[tokio::test]
async fn look_alikes_of_operator_addresses_cannot_sign_up() {
    env();
    let app = common::TestApp::new().await;
    for (raw, stored) in [("\u{212A}ate@corp.com", "kate@corp.com"), ("\u{212B}sa@corp.com", "\u{e5}sa@corp.com"), ("KATE@corp.com", "kate@corp.com")] {
        let (status, body) = register(&app, raw).await;
        assert!(status == StatusCode::FORBIDDEN || status == StatusCode::BAD_REQUEST, "{raw:?}: {status} {body}");
        assert!(models_user::user_by_email(&app.state.control, stored).await.unwrap().is_none(), "{raw:?} made an account");
    }
}

#[tokio::test]
async fn look_alikes_are_refused_even_when_nothing_is_reserved() {
    env();
    let app = common::TestApp::new().await;
    for raw in ["\u{212A}en@acme.test", "\u{212B}ke@acme.test"] {
        let (status, body) = register(&app, raw).await;
        assert_eq!((status, body["code"].as_str()), (StatusCode::BAD_REQUEST, Some("validation.invalid")), "{raw:?}: {body}");
    }
    assert!(models_user::user_by_email(&app.state.control, "ken@acme.test").await.unwrap().is_none());
    // the same refusal below the route
    assert!(models_user::register_user(&app.state, "\u{212A}en@acme.test", common::PASSWORD).await.is_err());
    // a plain address and a punycode domain still sign up
    assert_eq!(register(&app, "Ken@Acme.test").await.0, StatusCode::OK);
    assert_eq!(register(&app, "anna@xn--bcher-kva.example").await.0, StatusCode::OK);
}

#[tokio::test]
async fn ordinary_capitals_outside_ascii_still_sign_up() {
    env();
    let app = common::TestApp::new().await;
    assert_eq!(register(&app, "\u{d8}lsen@firma.no").await.0, StatusCode::OK);
    assert!(models_user::user_by_email(&app.state.control, "\u{f8}lsen@firma.no").await.unwrap().is_some());
}

/// Operator status is read from the stored normalized address. A legacy row whose raw email only
/// normalizes to a listed address (its `email_lc` is a `duplicate:` placeholder) is not an operator.
#[tokio::test]
async fn a_legacy_duplicate_of_an_operator_address_is_not_an_operator() {
    env();
    let app = common::TestApp::new().await;
    let make = |email: &'static str, email_lc: &'static str| {
        let db = app.state.control.clone();
        async move {
            let mut res = db.test_raw().query("CREATE user SET email = $e, email_lc = $l RETURN VALUE id").bind(("e", email)).bind(("l", email_lc)).await.unwrap();
            res.take::<Vec<surrealdb::types::RecordId>>(0).unwrap().remove(0)
        }
    };
    let real = make("kate@corp.com", "kate@corp.com").await;
    let legacy = make("\u{212A}ate@corp.com", "duplicate:legacy").await;
    let user = |id: surrealdb::types::RecordId, email: &str| models_user::User { id, email: email.into(), org: app.user.org };
    assert!(eunomia_backend::authz::is_operator(&app.state.control, &user(real.clone(), "kate@corp.com")).await.unwrap());
    assert!(eunomia_backend::authz::is_admin(&app.state.control, &user(real, "kate@corp.com")).await.unwrap());
    let legacy_user = user(legacy, "\u{212A}ate@corp.com");
    assert!(!eunomia_backend::authz::is_operator(&app.state.control, &legacy_user).await.unwrap());
    assert!(!eunomia_backend::authz::is_admin(&app.state.control, &legacy_user).await.unwrap());
}

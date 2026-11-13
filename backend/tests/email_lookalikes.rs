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

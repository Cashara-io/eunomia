//! Workspace admin: space changes land in the security ledger and read back through
//! `/api/admin/audit` as sentences; `/api/admin/people` lists every account. Both refuse non-admins.

mod common;

use axum::http::StatusCode;
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
    for want in [
        "Deleted the space scratch",
        "Created the space scratch",
        "Merged acme-eng and acme-copy into a new space, acme-all",
        "Copied acme-eng into a new space, acme-copy",
        "Renamed acme-engineering to acme-eng",
        "Withdrew the invitation for carol@example.com to acme-engineering",
        "Made bob@example.com an owner of acme-engineering",
        "Joined acme-engineering",
        "Invited bob@example.com to acme-engineering",
        "Created the space acme-engineering",
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
    let bob_token = models_user::create_api_token(&app.state.control, &bob.id, "t").await.unwrap().token;
    for path in ["/api/admin/audit", "/api/admin/people"] {
        let ((status, _), _) = common::http(&app.router, "GET", path, None, Some(&bob_token), None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }
}

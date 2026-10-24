//! `POST /api/entities/cloud/similarity`: pairwise cosine / distance / correlation of cloud points
//! on their real vectors, behind the same vault access as `GET /api/entities/cloud`.

mod common;

use common::{http, TestApp};
use eunomia_backend::rid::RecordIdExt;
use eunomia_backend::vaults::service as vaults;
use serde_json::{json, Value};

async fn write(app: &TestApp, text: &str) {
    let out = app.tool("memory_write", json!({"subject_name": "Ada", "subject_kind": "person", "text": text})).await;
    assert!(out.get("error").is_none(), "{out}");
}

async fn cloud_ids(app: &TestApp) -> Vec<(String, String)> {
    let (status, cloud) = app.http("GET", "/api/entities/cloud", None, true).await;
    assert_eq!(status, 200, "{cloud}");
    cloud["points"].as_array().unwrap().iter().map(|p| (p["id"].as_str().unwrap().to_string(), p["label"].as_str().unwrap().to_string())).collect()
}

fn matrix(v: &Value) -> Vec<Vec<f64>> {
    v.as_array().unwrap().iter().map(|r| r.as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect()).collect()
}

#[tokio::test]
async fn similarity_compares_cloud_points_on_their_vectors() {
    let app = TestApp::new().await;
    write(&app, "Ada wrote the first algorithm for the engine").await;
    write(&app, "the first algorithm for the engine was written by Ada").await;
    write(&app, "quarterly invoice for cloud hosting is overdue").await;
    let points = cloud_ids(&app).await;
    assert_eq!(points.len(), 3);
    let id_of = |needle: &str| points.iter().find(|(_, l)| l.contains(needle)).unwrap().0.clone();
    let (a, b, c) = (id_of("Ada wrote"), id_of("was written"), id_of("invoice"));

    let (status, out) = app.http("POST", "/api/entities/cloud/similarity", Some(json!({"ids": [a, b, c, a]})), true).await;
    assert_eq!(status, 200, "{out}");
    // duplicates collapse, order is kept
    assert_eq!(out["ids"], json!([a, b, c]));
    assert_eq!(out["space"], "lexical");
    let (cos, dist, corr) = (matrix(&out["cosine"]), matrix(&out["distance"]), matrix(&out["correlation"]));
    for m in [&cos, &dist, &corr] {
        assert_eq!(m.len(), 3);
        assert!(m.iter().all(|r| r.len() == 3));
    }
    for i in 0..3 {
        assert!((cos[i][i] - 1.0).abs() < 1e-6 && dist[i][i] == 0.0 && (corr[i][i] - 1.0).abs() < 1e-6);
        for j in 0..3 {
            assert!((cos[i][j] - cos[j][i]).abs() < 1e-6 && (dist[i][j] - dist[j][i]).abs() < 1e-6);
        }
    }
    // the two phrasings of one sentence are closer than either is to the invoice
    assert!(cos[0][1] > cos[0][2] && cos[0][1] > cos[1][2], "{cos:?}");
    assert!(dist[0][1] < dist[0][2], "{dist:?}");

    // one point is a valid (trivial) comparison; none is an empty matrix
    let (status, one) = app.http("POST", "/api/entities/cloud/similarity", Some(json!({"ids": [a]})), true).await;
    assert_eq!(status, 200);
    assert_eq!(one["cosine"], json!([[1.0]]));
    let (status, none) = app.http("POST", "/api/entities/cloud/similarity", Some(json!({"ids": []})), true).await;
    assert_eq!(status, 200);
    assert_eq!(none["ids"], json!([]));
}

#[tokio::test]
async fn similarity_rejects_unknown_ids_and_oversized_requests() {
    let app = TestApp::new().await;
    write(&app, "one fact").await;
    let (status, _) = app.http("POST", "/api/entities/cloud/similarity", Some(json!({"ids": ["memory:doesnotexist"]})), true).await;
    assert_eq!(status, 404);
    let many: Vec<String> = (0..41).map(|i| format!("memory:m{i}")).collect();
    let (status, _) = app.http("POST", "/api/entities/cloud/similarity", Some(json!({"ids": many})), true).await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn similarity_is_vault_scoped_like_the_cloud() {
    let app = TestApp::new().await;
    write(&app, "ALICESECRET the private plan").await;
    let secret = cloud_ids(&app).await.remove(0).0;

    let outsider = common::register(&app.state, "mallory@example.com").await;
    let token = eunomia_backend::models_user::create_api_token(&app.state.control, &outsider.id, "t").await.unwrap().token;
    let post = |body: Value| {
        let router = app.router.clone();
        let token = token.clone();
        async move { http(&router, "POST", "/api/entities/cloud/similarity", Some(body), Some(&token), None).await.0 }
    };

    // another user's vault is refused outright, exactly as for the cloud
    let db = app.db().await;
    let owners_vault = vaults::personal_vault_id(&db, &app.user.id).await.unwrap().to_string();
    let (status, body) = post(json!({"vault_ids": [owners_vault], "ids": [secret]})).await;
    assert_eq!(status, 403, "{body}");
    assert!(!body.to_string().contains("ALICESECRET"));

    // their own (default) vault does not hold that point: not found, not a probe for existence
    let (status, body) = post(json!({"ids": [secret]})).await;
    assert_eq!(status, 404, "{body}");

    // and no token at all is unauthenticated
    let ((status, _), _) = http(&app.router, "POST", "/api/entities/cloud/similarity", Some(json!({"ids": []})), None, None).await;
    assert_eq!(status, 401);
}

#[test]
fn internal_token_issuance_requires_explicit_permissions_destination_and_expiry() {
    let source = include_str!("../src/models_user.rs");
    assert!(!source.contains("pub async fn create_api_token("), "remove the broad name-only convenience issuer");
    let replay = include_str!("../src/replay.rs");
    assert!(replay.contains("create_api_token_with("));
}

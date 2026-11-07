//! Outgoing invitations: owners see who has not answered yet, can withdraw one, and an invitee sees who
//! invited them. Runs the service layer as the system, like `vault_roles`.

mod common;

use eunomia_backend::rid;
use eunomia_backend::vaults::service as vaults;

#[tokio::test]
async fn owners_list_and_revoke_pending_invitations() {
    common::sys(Box::pin(async {
        let state = common::bare_state().await;
        let alice = common::register(&state, "alice@example.com").await;
        let bob = common::register(&state, "bob@example.com").await;
        let carol = common::register(&state, "carol@example.com").await;
        let db = common::org_db(&state, &alice).await;
        let control = state.control.clone();

        let v = vaults::create_vault(&db, &control, &alice.id, "Team", "org").await.unwrap();
        let v = rid::parse(&v.id).unwrap();
        assert!(vaults::list_pending_invites(&db, &control, &alice.id, &v).await.unwrap().is_empty());

        vaults::invite_member(&db, &control, &alice.id, &v, &bob.email, "member").await.unwrap();
        vaults::invite_member(&db, &control, &alice.id, &v, &carol.email, "owner").await.unwrap();

        // The owner sees both; neither is an active member yet.
        let pending = vaults::list_pending_invites(&db, &control, &alice.id, &v).await.unwrap();
        let mut emails: Vec<_> = pending.iter().map(|p| (p.email.as_str(), p.role.as_str())).collect();
        emails.sort();
        assert_eq!(emails, vec![("bob@example.com", "member"), ("carol@example.com", "owner")]);
        let members = vaults::list_members(&db, &control, &alice.id, &v).await.unwrap();
        assert_eq!(members.len(), 1, "pending invitees are not members");

        // The invitee sees who invited them: the space's only owner.
        let mut inv = vaults::list_my_invitations(&db, &bob.id).await.unwrap();
        vaults::add_inviters(&db, &control, &mut inv).await.unwrap();
        assert_eq!(inv[0].invited_by.as_deref(), Some("alice@example.com"));

        // Revoking is removing the pending membership: Carol's invitation is gone and she cannot accept.
        vaults::remove_member(&db, &control, &alice.id, &v, &carol.email).await.unwrap();
        let pending = vaults::list_pending_invites(&db, &control, &alice.id, &v).await.unwrap();
        assert_eq!(pending.iter().map(|p| p.email.as_str()).collect::<Vec<_>>(), vec!["bob@example.com"]);
        assert!(vaults::accept_invitation(&db, &control, &carol.id, &v).await.is_err(), "a revoked invitation cannot be accepted");

        // Once Bob joins, he is no longer pending, and as a plain member he cannot read the list.
        vaults::accept_invitation(&db, &control, &bob.id, &v).await.unwrap();
        assert!(vaults::list_pending_invites(&db, &control, &alice.id, &v).await.unwrap().is_empty());
        assert!(vaults::list_pending_invites(&db, &control, &bob.id, &v).await.is_err(), "members cannot list invitations");
        // Nor can someone outside the space.
        assert!(vaults::list_pending_invites(&db, &control, &carol.id, &v).await.is_err());
    }))
    .await;
}

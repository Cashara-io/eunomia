//! Member roles: promote, demote, hand a vault over, and the invitations an agent can now answer.
//! Runs the service layer as the system, like `authz_matrix`.

mod common;

use eunomia_backend::rid;
use eunomia_backend::vaults::service as vaults;

#[tokio::test]
async fn roles_ownership_and_invitations() {
    common::sys(Box::pin(async {
        let state = common::bare_state().await;
        let alice = common::register(&state, "alice@example.com").await;
        let bob = common::register(&state, "bob@example.com").await;
        let carol = common::register(&state, "carol@example.com").await;
        let db = common::org_db(&state, &alice).await;
        let control = state.control.clone();

        let v = vaults::create_vault(&db, &alice.id, "Handover", "org").await.unwrap();
        let v = rid::parse(&v.id).unwrap();
        for u in [&bob, &carol] {
            vaults::invite_member(&db, &control, &alice.id, &v, &u.email, "member").await.unwrap();
        }

        // Bob sees his invitation, declines Carol's path: only the invitee can answer his own.
        let inv = vaults::list_my_invitations(&db, &bob.id).await.unwrap();
        assert_eq!(inv.len(), 1);
        assert_eq!(inv[0].vault_name, "Handover");
        vaults::accept_invitation(&db, &bob.id, &v).await.unwrap();
        assert!(vaults::list_my_invitations(&db, &bob.id).await.unwrap().is_empty());
        vaults::decline_invitation(&db, &carol.id, &v).await.unwrap();
        assert!(vaults::accept_invitation(&db, &carol.id, &v).await.is_err(), "declined invitation is gone");

        // vault_get: caller's role and the active member count.
        let got = vaults::get_vault(&db, &bob.id, &v).await.unwrap();
        assert_eq!((got.role.as_str(), got.member_count), ("member", 2));
        assert!(vaults::get_vault(&db, &carol.id, &v).await.is_err(), "a non-member cannot read a vault");

        // A member cannot change roles; an owner can.
        assert!(vaults::set_member_role(&db, &control, &bob.id, &v, &alice.email, "member").await.is_err());
        vaults::set_member_role(&db, &control, &alice.id, &v, &bob.email, "owner").await.unwrap();
        assert_eq!(vaults::get_vault(&db, &bob.id, &v).await.unwrap().role, "owner");
        assert!(vaults::set_member_role(&db, &control, &alice.id, &v, &bob.email, "admin").await.is_err(), "unknown role");

        // Two owners: either may be demoted, but never the last one.
        vaults::set_member_role(&db, &control, &bob.id, &v, &alice.email, "member").await.unwrap();
        let err = vaults::set_member_role(&db, &control, &bob.id, &v, &bob.email, "member").await.unwrap_err();
        assert!(err.detail().contains("last owner"), "{}", err.detail());

        // Hand the vault back: Alice owns again, Bob is a member, in one step.
        vaults::set_member_role(&db, &control, &bob.id, &v, &alice.email, "owner").await.unwrap();
        vaults::transfer_ownership(&db, &control, &alice.id, &v, &bob.email).await.unwrap();
        assert_eq!(vaults::get_vault(&db, &alice.id, &v).await.unwrap().role, "member");
        assert_eq!(vaults::get_vault(&db, &bob.id, &v).await.unwrap().role, "owner");
        // Transferring to yourself, or to someone who is not a member, is refused.
        assert!(vaults::transfer_ownership(&db, &control, &bob.id, &v, &bob.email).await.is_err());
        assert!(vaults::transfer_ownership(&db, &control, &bob.id, &v, &carol.email).await.is_err());
        // Alice is no longer an owner, so she cannot hand it on.
        assert!(vaults::transfer_ownership(&db, &control, &alice.id, &v, &bob.email).await.is_err());
    }))
    .await;
}

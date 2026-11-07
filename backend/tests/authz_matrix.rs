//! Who may do what in a vault, role by role, against a real database. Written
//! against the behaviour before `authz::authorize` existed and kept as the
//! guard that routing every check through it changed nothing.

mod common;

use eunomia_backend::pool::{ControlDb, OrgDb};
use eunomia_backend::entities::service as entities;
use eunomia_backend::error::AppResult;
use eunomia_backend::models_user::User;
use eunomia_backend::ontology::{self, metric, property};
use eunomia_backend::vaults::service as vaults;
use eunomia_backend::rid;
use surrealdb::types::RecordId;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Who {
    Owner,
    Member,
    Pending,
    Outsider,
}
use Who::*;

const WHOS: [Who; 4] = [Owner, Member, Pending, Outsider];

struct World {
    db: OrgDb,
    org: eunomia_backend::state::OrgState,
    control: ControlDb,
    owner: User,
    member: User,
    pending: User,
    outsider: User,
    spare: User,
}

impl World {
    fn user(&self, who: Who) -> &User {
        match who {
            Owner => &self.owner,
            Member => &self.member,
            Pending => &self.pending,
            Outsider => &self.outsider,
        }
    }

    /// A fresh org vault owned by `owner`, with `member` active and `pending` invited.
    async fn vault(&self) -> RecordId {
        let v = common::sys(vaults::create_vault(&self.db, &self.control, &self.owner.id, &format!("Team {}", uuid::Uuid::new_v4().simple()), "org")).await.unwrap();
        let v: RecordId = rid::parse(&v.id).unwrap();
        for (u, accept) in [(&self.member, true), (&self.pending, false)] {
            common::sys(vaults::invite_member(&self.db, &self.control, &self.owner.id, &v, &u.email, "member")).await.unwrap();
            if accept {
                common::sys(vaults::accept_invitation(&self.db, &self.control, &u.id, &v)).await.unwrap();
            }
        }
        v
    }
}

/// A `price` (money) on `product`.
fn price() -> ontology::Proposals {
    serde_json::from_value(serde_json::json!({"properties": [{"kind": "product", "name": "price", "datatype": "money", "unit": "AUD"}]})).unwrap()
}

/// `price` with an `avg_price` metric over it.
fn avg_price() -> ontology::Proposals {
    serde_json::from_value(serde_json::json!({
        "properties": [{"kind": "product", "name": "price", "datatype": "money", "unit": "AUD"}],
        "metrics": [{"kind": "product", "name": "avg_price", "expr": {"agg": "avg", "property": "price"}}],
    }))
    .unwrap()
}

async fn world() -> World {
    let state = common::bare_state().await;
    let mut users = Vec::new();
    for name in ["owner", "member", "pending", "outsider", "spare"] {
        users.push(common::register(&state, &format!("{name}@example.com")).await);
    }
    let db = common::org_db(&state, &users[0]).await;
    let org = state.org(&users[0].org).await.unwrap();
    let mut it = users.into_iter();
    World {
        db,
        org,
        control: state.control.clone(),
        owner: it.next().unwrap(),
        member: it.next().unwrap(),
        pending: it.next().unwrap(),
        outsider: it.next().unwrap(),
        spare: it.next().unwrap(),
    }
}

fn allowed<T>(r: AppResult<T>) -> bool {
    match r {
        Ok(_) => true,
        Err(e) => {
            assert_eq!(e.status, axum::http::StatusCode::FORBIDDEN, "denial must be a 403, got {e:?}");
            false
        }
    }
}

type Row = (&'static str, [bool; 4]);

fn check(op: &str, who: Who, got: bool, want: bool) {
    assert_eq!(got, want, "{op} as {who:?}: expected allowed={want}");
}

#[tokio::test]
async fn vault_operations_matrix() {
    common::sys(Box::pin(async {
    let w = world().await;
    // [owner, member, pending, outsider]
    let rows: Vec<Row> = vec![
        ("read memories", [true, true, false, false]),
        ("write memories", [true, true, false, false]),
        ("list members", [true, true, false, false]),
        ("rename", [true, false, false, false]),
        ("invite", [true, false, false, false]),
        ("remove member", [true, false, false, false]),
        ("delete", [true, false, false, false]),
        ("clone", [true, true, false, false]),
        ("merge", [true, true, false, false]),
        ("propose a property", [true, true, false, false]),
        ("decide a property", [true, false, false, false]),
        ("decide a metric", [true, false, false, false]),
        ("run a metric", [true, true, false, false]),
        ("auto-map records", [true, true, false, false]),
    ];
    for (op, want) in rows {
        for (i, who) in WHOS.into_iter().enumerate() {
            let v = w.vault().await;
            let u = w.user(who);
            let got = match op {
                "read memories" => allowed(entities::list_entities(&w.db, &u.id, None, Some(&v), None, 0).await),
                "write memories" => allowed(entities::upsert_entity(&w.db, &u.id, "person", "Bob", None, Some(&v)).await),
                "list members" => allowed(common::sys(vaults::list_members(&w.db, &w.control, &u.id, &v)).await),
                "rename" => allowed(common::sys(vaults::rename_vault(&w.db, &w.control, &u.id, &v, "Renamed")).await),
                "invite" => allowed(common::sys(vaults::invite_member(&w.db, &w.control, &u.id, &v, &w.spare.email, "member")).await),
                "remove member" => allowed(common::sys(vaults::remove_member(&w.db, &w.control, &u.id, &v, &w.pending.email)).await),
                "delete" => allowed(common::sys(vaults::delete_vault(&w.db, &w.control, &u.id, &v)).await),
                "clone" => allowed(common::sys(vaults::clone_vault(&w.db, &w.control, &u.id, &v, None, "org")).await),
                "propose a property" => allowed(ontology::propose(&w.db, &u.id, Some(&v), &price()).await),
                "decide a property" => {
                    ontology::propose(&w.db, &w.owner.id, Some(&v), &price()).await.unwrap();
                    allowed(property::decide_property(&w.db, &u.id, &v, "product", "price", property::PropertyDecision::Approve).await)
                }
                "decide a metric" => {
                    ontology::propose(&w.db, &w.owner.id, Some(&v), &avg_price()).await.unwrap();
                    property::decide_property(&w.db, &w.owner.id, &v, "product", "price", property::PropertyDecision::Approve).await.unwrap();
                    allowed(metric::decide_metric(&w.db, &u.id, &v, "product", "avg_price", metric::MetricDecision::Approve).await)
                }
                "run a metric" => {
                    ontology::propose(&w.db, &w.owner.id, Some(&v), &avg_price()).await.unwrap();
                    let args = metric::EvalArgs { metric: "avg_price".into(), vault_id: Some(rid::RecordIdExt::to_string(&v)), allow_unapproved: true, ..Default::default() };
                    allowed(metric::eval(&w.db, &u.id, &args).await)
                }
                "auto-map records" => {
                    let args = ontology::automap::AutomapArgs { vault_id: Some(rid::RecordIdExt::to_string(&v)), ..Default::default() };
                    allowed(ontology::automap::automap(&w.org, &w.org.settings, &u.id, &args).await)
                }
                "merge" => {
                    let own = common::sys(vaults::default_vault_id(&w.db, &u.id)).await.unwrap();
                    allowed(common::sys(vaults::merge_vaults(&w.db, &w.control, &u.id, &v, &own, None, "org")).await)
                }
                _ => unreachable!(),
            };
            check(op, who, got, want[i]);
        }
    }
    }))
    .await;
}

#[tokio::test]
async fn entity_row_operations_matrix() {
    common::sys(Box::pin(async {
    let w = world().await;
    // Row-level checks treat a non-member as "not found" (None/false), not a 403.
    for who in WHOS {
        let v = w.vault().await;
        let e = entities::upsert_entity(&w.db, &w.owner.id, "person", "Alice", None, Some(&v)).await.unwrap();
        let eid: RecordId = rid::parse(&e.id).unwrap();
        let mem = entities::add_memory(&w.db, &w.owner.id, &eid, "likes tea", None, "world").await.unwrap();
        let mid: RecordId = rid::parse(&mem.id).unwrap();
        let u = w.user(who);
        let member_like = matches!(who, Owner | Member);

        let got = entities::get_entity(&w.db, &w.control, &u.id, &eid).await.unwrap().is_some();
        check("get entity", who, got, member_like);
        let got = entities::update_entity(&w.db, &u.id, &eid, Some("Alicia"), None, None).await.unwrap().is_some();
        check("update entity", who, got, member_like);
        let got = entities::update_memory(&w.db, &u.id, &mid, Some("likes coffee"), None).await.unwrap().is_some();
        check("update memory", who, got, member_like);
        let got = allowed(entities::add_memory(&w.db, &u.id, &eid, "new", None, "world").await);
        check("add memory", who, got, member_like);
        ontology::propose(&w.db, &w.owner.id, Some(&v), &price()).await.unwrap();
        w.db.test_raw().query("UPDATE ontology_property SET status = 'approved' WHERE vault = $v").bind(("v", v.clone())).await.unwrap().check().unwrap();
        let product = entities::upsert_entity(&w.db, &w.owner.id, "product", "Latte", None, Some(&v)).await.unwrap();
        let pid: RecordId = rid::parse(&product.id).unwrap();
        let value = serde_json::json!(4.5);
        let set = property::SetProperty { entity_id: &pid, property: "price", value: &value, unit: None, as_of: None, source_record_id: None };
        let res = property::set_property(&w.db, &u.id, set).await;
        if let Err(e) = &res {
            assert_eq!(e.status, axum::http::StatusCode::NOT_FOUND, "a non-member sees no entity: {e:?}");
        }
        check("set property", who, res.is_ok(), member_like);
        let got = entities::delete_memory(&w.db, &u.id, &mid).await.unwrap();
        check("delete memory", who, got, member_like);
        let got = entities::delete_entity(&w.db, &u.id, &eid).await.unwrap();
        check("delete entity", who, got, member_like);
    }
    }))
    .await;
}

#[tokio::test]
async fn leave_and_invitation_rules() {
    common::sys(Box::pin(async {
    let w = world().await;
    let v = w.vault().await;
    // a member leaves; a non-member leaving is a quiet no-op
    common::sys(vaults::leave_vault(&w.db, &w.member.id, &v)).await.unwrap();
    common::sys(vaults::leave_vault(&w.db, &w.outsider.id, &v)).await.unwrap();
    assert!(!allowed(common::sys(vaults::list_members(&w.db, &w.control, &w.member.id, &v)).await));
    // only the invitee can accept
    let v2 = w.vault().await;
    assert!(common::sys(vaults::accept_invitation(&w.db, &w.control, &w.outsider.id, &v2)).await.is_err());
    assert!(common::sys(vaults::accept_invitation(&w.db, &w.control, &w.pending.id, &v2)).await.is_ok());
    }))
    .await;
}

/// `authorize()` itself, every action for every kind of user, against the role matrix.
#[tokio::test]
async fn authorize_matches_the_role_matrix_for_every_action() {
    common::sys(Box::pin(async {
    use eunomia_backend::authz::{authorize, permits, Action, Role};
    let w = world().await;
    let v = w.vault().await;
    for who in WHOS {
        let role = match who {
            Owner => Some(Role::Owner),
            Member => Some(Role::Member),
            Pending | Outsider => None,
        };
        for action in Action::ALL {
            let got = authorize(&w.db, &w.user(who).id, action, &v).await;
            let want = role.is_some_and(|r| permits(r, action));
            assert_eq!(got.is_ok(), want, "{who:?} {action:?}");
            match got {
                Ok(scope) => assert_eq!((scope.vault(), Some(scope.role())), (&v, role)),
                Err(e) => assert_eq!((e.status, e.code.as_str()), (axum::http::StatusCode::FORBIDDEN, "vault.forbidden")),
            }
        }
    }
    }))
    .await;
}

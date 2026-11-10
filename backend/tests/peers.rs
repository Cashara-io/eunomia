//! Peer vaults (docs/peer-vaults.md) between separate instances in one process: each `TestApp` is its
//! own in-memory database, and `peers::net::test_route` sends one instance's peer calls to the
//! other's real router (gate, rate limits and all), with no socket.

mod common;

use common::{sys, TestApp};
use eunomia_backend::peers::{self, sync};
use eunomia_backend::rid::RecordIdExt;
use eunomia_backend::state::OrgState;
use serde_json::{json, Value};
use surrealdb::types::RecordId;

/// The peer tests' futures are deep (two instances' service stacks): poll them on a roomy thread.
fn run<F, Fut>(f: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()>,
{
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || tokio::runtime::Builder::new_multi_thread().worker_threads(2).thread_stack_size(32 * 1024 * 1024).enable_all().build().unwrap().block_on(f()))
        .unwrap()
        .join()
        .unwrap_or_else(|e| std::panic::resume_unwind(e));
}

struct Node {
    app: TestApp,
    base: String,
}

impl Node {
    async fn new(name: &str) -> Node {
        let app = TestApp::new().await;
        let base = format!("http://{name}.test:18195");
        peers::net::test_route(&base, app.router.clone());
        Node { app, base }
    }

    async fn org(&self) -> OrgState {
        self.app.state.org(&self.app.user.org).await.unwrap()
    }

    async fn vault(&self, name: &str) -> RecordId {
        let v = self.app.tool("vault_create", json!({"name": name, "kind": "org"})).await;
        eunomia_backend::rid::parse(v["id"].as_str().unwrap()).unwrap()
    }

    async fn fact(&self, vault: &RecordId, subject: &str, text: &str) -> Value {
        let out = self.app.tool("memory_write", json!({"subject_name": subject, "subject_kind": "person", "text": text, "vault_id": vault.to_string()})).await;
        assert!(out.get("error").is_none(), "{out}");
        out
    }

    async fn code(&self, vault: &RecordId, role: &str) -> String {
        let st = self.org().await;
        sys(peers::create_share(&st, &self.app.user.id, vault, role, None, vec![self.base.clone()])).await.unwrap().code
    }

    /// Joins with `code`; returns the new vault and the link id.
    async fn join(&self, code: &str) -> (RecordId, String) {
        let st = self.org().await;
        let out = sys(peers::join(&st, &self.app.user.id, code, vec![self.base.clone()], &self.app.user.email)).await.unwrap();
        (eunomia_backend::rid::parse(&out.vault.id).unwrap(), out.link.id)
    }

    async fn sync(&self, link: &str) -> sync::Report {
        let st = self.org().await;
        sys(sync::sync_link(&st, link)).await.unwrap()
    }

    async fn digest(&self, vault: &RecordId) -> (String, usize) {
        sync::digest(&self.app.db().await, vault).await.unwrap()
    }

    async fn link(&self, key: &str) -> peers::LinkRow {
        peers::load_link(&self.app.db().await, key).await.unwrap().expect("link")
    }

    async fn log_len(&self, vault: &RecordId) -> usize {
        eunomia_backend::changes::after(&self.app.db().await, vault, 0, 1000).await.unwrap().changes.len()
    }

    async fn find(&self, vault: &RecordId, query: &str) -> Value {
        self.app.tool("entities_search", json!({"query": query, "vault_id": vault.to_string()})).await
    }
}

fn names(search: &Value) -> Vec<String> {
    search["results"].as_array().into_iter().flatten().filter_map(|e| e["name"].as_str().map(str::to_string)).collect()
}

#[test]
fn share_join_and_sync_both_ways() {
    run(|| async {
        let (a, b) = (Node::new("a1").await, Node::new("b1").await);
        let va = a.vault("Project Atlas").await;
        a.fact(&va, "Alice", "Alice likes green tea").await;
        a.app.tool("skill_save", json!({"name": "brew-tea", "description": "How to brew", "body": "Boil water.", "vault_id": va.to_string()})).await;

        let code = a.code(&va, "edit").await;
        let (vb, link) = b.join(&code).await;
        // the first sync ran during the join: both copies hold the same items
        let (da, na) = a.digest(&va).await;
        assert!(na >= 3, "entity, fact, skill: {na}");
        assert_eq!(b.digest(&vb).await, (da, na));
        assert_eq!(b.link(&link).await.status, peers::status::UP_TO_DATE);
        assert!(names(&b.find(&vb, "Alice").await).contains(&"Alice".to_string()));

        // a code works once
        let st = b.org().await;
        assert!(sys(peers::join(&st, &b.app.user.id, &code, vec![], "again")).await.is_err());

        // the guest's write reaches the host
        b.fact(&vb, "Bob", "Bob plays jazz piano").await;
        let r = b.sync(&link).await;
        assert_eq!(r.rejected, 0);
        assert!(names(&a.find(&va, "Bob").await).contains(&"Bob".to_string()));
        assert_eq!(a.digest(&va).await, b.digest(&vb).await);

        // the host's write reaches the guest
        a.fact(&va, "Alice", "Alice moved to Lisbon").await;
        b.sync(&link).await;
        assert_eq!(a.digest(&va).await, b.digest(&vb).await);

        // nothing echoes: another sync writes nothing on either side
        let (la, lb) = (a.log_len(&va).await, b.log_len(&vb).await);
        let r = b.sync(&link).await;
        assert_eq!((r.applied, r.rejected), (0, 0), "{r:?}");
        assert_eq!((a.log_len(&va).await, b.log_len(&vb).await), (la, lb), "an idle sync wrote change-log rows");

        // a delete travels as a tombstone
        let bob = a.find(&va, "Bob").await;
        let bob_id = bob["results"].as_array().unwrap().iter().find(|e| e["name"] == "Bob").unwrap()["id"].as_str().unwrap().to_string();
        a.app.tool("entity_delete", json!({"entity_id": bob_id})).await;
        b.sync(&link).await;
        assert!(!names(&b.find(&vb, "Bob").await).contains(&"Bob".to_string()));
        assert_eq!(a.digest(&va).await, b.digest(&vb).await);

        // verify recomputes both digests
        let v = sys(peers::verify(&st, &b.app.user.id, &link)).await.unwrap();
        assert_eq!(v.matches, Some(true));
    });
}

#[test]
fn a_copy_relays_down_a_tree() {
    run(|| async {
        let (a, b, c) = (Node::new("a2").await, Node::new("b2").await, Node::new("c2").await);
        let va = a.vault("Tree").await;
        a.fact(&va, "Root", "Root is the top").await;
        let (vb, ab) = b.join(&a.code(&va, "edit").await).await;
        let (vc, bc) = c.join(&b.code(&vb, "edit").await).await;
        assert_eq!(c.digest(&vc).await, a.digest(&va).await);

        // A's change goes A -> B -> C
        a.fact(&va, "Leaf", "Leaf hangs low").await;
        b.sync(&ab).await;
        c.sync(&bc).await;
        assert!(names(&c.find(&vc, "Leaf").await).contains(&"Leaf".to_string()));

        // and C's goes C -> B -> A
        c.fact(&vc, "Twig", "Twig grew on C").await;
        c.sync(&bc).await;
        b.sync(&ab).await;
        assert!(names(&a.find(&va, "Twig").await).contains(&"Twig".to_string()));
        let d = a.digest(&va).await;
        assert_eq!(b.digest(&vb).await, d);
        c.sync(&bc).await;
        assert_eq!(c.digest(&vc).await, d);
    });
}

#[test]
fn read_only_guests_do_not_write_back() {
    run(|| async {
        let (a, b) = (Node::new("a3").await, Node::new("b3").await);
        let va = a.vault("Readers").await;
        a.fact(&va, "Ann", "Ann reads").await;
        let (vb, link) = b.join(&a.code(&va, "read").await).await;
        b.fact(&vb, "Mallory", "Mallory tries to write").await;
        b.sync(&link).await;
        assert!(!names(&a.find(&va, "Mallory").await).contains(&"Mallory".to_string()));
        let report = b.link(&link).await.last_report.unwrap();
        assert!(report["sent"]["ignored"].as_u64().unwrap() >= 1, "{report}");
    });
}

#[test]
fn tampered_items_and_forged_requests_are_refused() {
    run(|| async {
        let (a, b) = (Node::new("a4").await, Node::new("b4").await);
        let va = a.vault("Integrity").await;
        let (_vb, key) = b.join(&a.code(&va, "edit").await).await;

        // an item whose content does not match its hash is refused, and nothing is written
        let st = a.org().await;
        let row = a.link(&key).await;
        let ctx = sync::Ctx { state: &st, link: &row, key: key.clone(), me: peers::instance_id(&st.db).await.unwrap() };
        let data = sync::entity_content("person", "Eve", &[], "");
        let good = sync::item_hash("entity", &data);
        let mut item = sync::Item {
            kind: "entity".into(),
            id: "person:evil".into(),
            peer_id: None,
            origin: "x:person:evil".into(),
            op: "upsert".into(),
            data: sync::entity_content("person", "Eve (tampered)", &[], ""),
            refs: Default::default(),
            hash: good.clone(),
            updated_at: None,
            into: None,
        };
        let before = a.log_len(&va).await;
        let r = sync::apply(&ctx, vec![item.clone()]).await;
        assert_eq!(r.rejected, 1);
        assert_eq!(a.log_len(&va).await, before);
        // the same item with its true content goes in; applying it again is a no-op
        item.data = data;
        assert_eq!(sync::apply(&ctx, vec![item.clone()]).await.applied, 1);
        let after = a.log_len(&va).await;
        assert_eq!(sync::apply(&ctx, vec![item]).await.unchanged, 1);
        assert_eq!(a.log_len(&va).await, after);

        // a request not sealed with the link key gets nothing
        let org = a.app.user.org.key();
        let forged = json!({"link": key, "ts": chrono::Utc::now().timestamp(), "nonce": "n1", "data": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"});
        let res = sync::host_rpc(&st, &org, &key, &forged).await;
        assert_eq!(res.unwrap_err().0, 401);

        // a join whose MAC does not verify is refused
        let code = a.code(&va, "edit").await;
        let body = peers::decode_code(&code).unwrap();
        let bad = peers::JoinRequest {
            share: body.share,
            guest_instance: "g".into(),
            guest_org: String::new(),
            guest_name: "x".into(),
            guest_addrs: vec![],
            nonce_g: "n".into(),
            mac: "00".repeat(32),
        };
        assert!(peers::accept_join(&st, bad).await.is_err());
    });
}

#[test]
fn stopping_keeps_both_copies() {
    run(|| async {
        let (a, b) = (Node::new("a5").await, Node::new("b5").await);
        let va = a.vault("Parting").await;
        a.fact(&va, "Pat", "Pat waves").await;
        let (vb, link) = b.join(&a.code(&va, "edit").await).await;
        // the host revokes; the guest learns on its next sync and keeps its copy
        let st = a.org().await;
        sys(peers::stop(&st, &a.app.user.id, &link)).await.unwrap();
        b.sync(&link).await;
        assert_eq!(b.link(&link).await.status, peers::status::REVOKED);
        assert!(names(&b.find(&vb, "Pat").await).contains(&"Pat".to_string()));
        assert!(names(&a.find(&va, "Pat").await).contains(&"Pat".to_string()));
    });
}

#[test]
fn review209_owner_merge_redirects_sync_to_existing_and_new_peer() {
    run(|| async {
        let (a, b, c) = (Node::new("merge-a").await, Node::new("merge-b").await, Node::new("merge-c").await);
        let va = a.vault("Stores").await;
        a.app.tool("ontology_propose", json!({"vault_id":va.to_string(),"kinds":[{"kind":"store","description":"A shop"}]})).await;
        let (status, decided) = a.app.http_session("POST", &format!("/api/vaults/{}/ontology/kinds/decide", va.to_string()), Some(json!({"kind":"store","decision":{"action":"approve"}}))).await;
        assert_eq!(status, 200, "{decided}");
        let winner = a.app.tool("memory_write", json!({"subject_kind":"store","subject_name":"North","text":"North opens daily","vault_id":va.to_string()})).await;
        let loser = a.app.tool("memory_write", json!({"subject_kind":"organisation","subject_name":"North Co","text":"North Co sells food","vault_id":va.to_string()})).await;
        let (vb, link) = b.join(&a.code(&va, "edit").await).await;
        let merged = a.app.tool("entity_merge", json!({"winner_id":winner["entity"]["id"],"loser_id":loser["entity"]["id"]})).await;
        assert!(merged.get("error").is_none(), "{merged}");
        let r = b.sync(&link).await;
        assert_eq!(r.failed, 0, "{r:?}");
        assert_eq!(a.digest(&va).await, b.digest(&vb).await);
        let (vc, _) = c.join(&a.code(&va, "edit").await).await;
        assert_eq!(a.digest(&va).await, c.digest(&vc).await);
        for (node, vault) in [(&a, &va), (&b, &vb), (&c, &vc)] {
            let got = node.find(vault, "North").await;
            assert!(!got.to_string().contains("@eunomia/merged/"), "{got}");
            assert_eq!(names(&got), vec!["North"]);
        }
        // Also exercise a guest-to-host update carrying the redirect back.
        let entity = b.find(&vb, "North").await["results"][0]["id"].clone();
        b.app.tool("entity_update", json!({"entity_id":entity,"summary":"Fresh summary"})).await;
        assert_eq!(b.sync(&link).await.failed, 0);
        assert_eq!(a.digest(&va).await, b.digest(&vb).await);
    });
}

#[test]
fn review209_failed_host_write_does_not_advance_cursor() {
    run(|| async {
        let (a, b) = (Node::new("failed-a").await, Node::new("failed-b").await);
        let va = a.vault("Failures").await;
        let (_, key) = b.join(&a.code(&va, "edit").await).await;
        let st = a.org().await;
        let row = a.link(&key).await;
        let ctx = sync::Ctx { state: &st, link: &row, key: key.clone(), me: peers::instance_id(&st.db).await.unwrap() };
        let data = sync::entity_content("person", "", &[], "");
        let item = sync::Item { kind:"entity".into(), id:"person:invalid".into(), peer_id:None, origin:"failed:person:invalid".into(), op:"upsert".into(), hash:sync::item_hash("entity", &data), data, refs:Default::default(), updated_at:None, into:None };
        let response = sys(sync::host_exchange(&ctx, sync::ExchangeRequest { changes:vec![item], upto:row.in_cursor + 10, ..Default::default() })).await;
        assert!(response.is_err(), "a failed page must not be acknowledged: {response:?}");
        assert_eq!(a.link(&key).await.in_cursor, row.in_cursor);
    });
}

#[test]
fn review209_failed_guest_write_does_not_advance_cursor() {
    run(|| async {
        use axum::{Json, Router, routing::post};
        use eunomia_backend::peers::crypto;
        let (a, b) = (Node::new("failed-guest-a").await, Node::new("failed-guest-b").await);
        let va = a.vault("Guest failures").await;
        let (_, key) = b.join(&a.code(&va, "edit").await).await;
        let row = b.link(&key).await;
        let secret = peers::link_secret(&b.app.state.settings, &row).unwrap();
        let next = row.in_cursor + 10;
        let route_key = key.clone();
        // This authenticated in-process host sends a valid hash with a name the write rejects.
        let router = Router::new().route("/api/peer/v1/{org}/links/{key}", post(move |Json(env): Json<Value>| {
            let key = route_key.clone();
            async move {
                let data = sync::entity_content("person", "", &[], "");
                let item = sync::Item { kind:"entity".into(), id:"person:invalid".into(), peer_id:None, origin:"failed:person:invalid".into(), op:"upsert".into(), hash:sync::item_hash("entity", &data), data, refs:Default::default(), updated_at:None, into:None };
                let response = sync::ExchangeResponse { changes:vec![item], next, ..Default::default() };
                let ts = chrono::Utc::now().timestamp();
                let nonce = crypto::random_id();
                let aad = crypto::response_aad(&key, env["nonce"].as_str().unwrap(), ts, &nonce);
                let data = crypto::seal(&secret, &aad, &serde_json::to_vec(&response).unwrap());
                Json(json!({"ts":ts,"nonce":nonce,"data":data}))
            }
        }));
        peers::net::test_route(&a.base, router);
        let report = b.sync(&key).await;
        assert_eq!(report.failed, 1);
        let after = b.link(&key).await;
        assert_eq!(after.in_cursor, row.in_cursor, "failed guest page was skipped");
        assert_eq!(after.status, peers::status::UNREACHABLE);
    });
}

import json
import os
import threading
import unittest
import uuid
from http.server import BaseHTTPRequestHandler, HTTPServer

from eunomia import Eunomia, EunomiaError


class Stub:
    """Local HTTP server. `routes` maps "METHOD /path" to a callable(body) -> (status, json, headers)."""

    def __init__(self):
        self.routes = {}
        self.calls = []
        stub = self

        class H(BaseHTTPRequestHandler):
            def log_message(self, *a):
                pass

            def handle_any(self):
                n = int(self.headers.get("Content-Length") or 0)
                body = json.loads(self.rfile.read(n)) if n else None
                stub.calls.append((self.command, self.path, body, self.headers.get("Authorization")))
                fn = stub.routes.get(f"{self.command} {self.path}")
                status, payload, headers = fn(body) if fn else (404, {"code": "x.none", "detail": "no route"}, {})
                raw = json.dumps(payload).encode()
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(raw)))
                for k, v in headers.items():
                    self.send_header(k, v)
                self.end_headers()
                self.wfile.write(raw)

            do_GET = do_POST = do_PATCH = do_DELETE = handle_any

        self.server = HTTPServer(("127.0.0.1", 0), H)
        self.url = f"http://127.0.0.1:{self.server.server_port}"
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def close(self):
        self.server.shutdown()
        self.server.server_close()


class UnitTests(unittest.TestCase):
    def setUp(self):
        self.stub = Stub()
        self.sleeps = []
        self.eu = Eunomia(self.stub.url, "tok123", sleep=self.sleeps.append)

    def tearDown(self):
        self.stub.close()

    def test_list_vaults_sends_bearer(self):
        self.stub.routes["GET /api/vaults"] = lambda b: (
            200, {"results": [{"id": "vault:1", "name": "P", "kind": "personal", "role": "owner", "created_at": None}]}, {})
        vaults = self.eu.list_vaults()
        self.assertEqual(vaults[0].id, "vault:1")
        self.assertEqual(vaults[0].role, "owner")
        self.assertEqual(self.stub.calls[0][3], "Bearer tok123")

    def test_get_vault_by_id_or_name_and_missing(self):
        self.stub.routes["GET /api/vaults"] = lambda b: (200, {"results": [{"id": "vault:1", "name": "P", "kind": "org"}]}, {})
        self.assertEqual(self.eu.get_vault("P").id, "vault:1")
        with self.assertRaises(EunomiaError) as cm:
            self.eu.get_vault("vault:zzz")
        self.assertEqual(cm.exception.code, "vault.not_found")

    def test_merge_sends_both_ids(self):
        self.stub.routes["POST /api/vaults/merge"] = lambda b: (
            200, {"id": "vault:3", "name": b["name"], "kind": "org", "entities": 4, "merged_from": b["vault_ids"]}, {})
        m = self.eu.merge_vaults("vault:1", "vault:2", name="All")
        self.assertEqual(m.entities, 4)
        self.assertEqual(m.merged_from, ["vault:1", "vault:2"])
        self.assertEqual(self.stub.calls[0][2], {"vault_ids": ["vault:1", "vault:2"], "kind": "org", "name": "All"})

    def test_vault_crud_paths(self):
        r = self.stub.routes
        r["POST /api/vaults"] = lambda b: (200, {"id": "vault:9", "name": b["name"], "kind": b["kind"]}, {})
        r["PATCH /api/vaults/vault%3A9"] = lambda b: (200, {"id": "vault:9", "name": b["name"], "kind": "org"}, {})
        r["POST /api/vaults/vault%3A9/clone"] = lambda b: (200, {"id": "vault:10", "name": "c", "kind": "org", "entities_copied": 2}, {})
        r["DELETE /api/vaults/vault%3A9"] = lambda b: (200, {"deleted": True}, {})
        self.assertEqual(self.eu.create_vault("x").id, "vault:9")
        self.assertEqual(self.eu.rename_vault("vault:9", "y").name, "y")
        self.assertEqual(self.eu.clone_vault("vault:9").entities_copied, 2)
        self.assertIsNone(self.eu.delete_vault("vault:9"))

    def test_members(self):
        r = self.stub.routes
        r["GET /api/vaults/v/members"] = lambda b: (200, {"results": [{"email": "a@b.c", "role": "owner"}]}, {})
        r["POST /api/vaults/v/members"] = lambda b: (200, {"vault_id": "v", "user_email": b["email"], "role": b["role"]}, {})
        r["DELETE /api/vaults/v/members/a%40b.c"] = lambda b: (200, {"removed": True}, {})
        r["POST /api/vaults/v/leave"] = lambda b: (200, {"left": True}, {})
        self.assertEqual(self.eu.list_members("v")[0].email, "a@b.c")
        self.assertEqual(self.eu.invite_member("v", "d@e.f", "editor").role, "editor")
        self.eu.remove_member("v", "a@b.c")
        self.eu.leave_vault("v")

    def test_error_carries_problem_fields(self):
        self.stub.routes["GET /api/vaults"] = lambda b: (
            403, {"code": "auth.forbidden", "detail": "nope", "trace_id": "abc"}, {})
        with self.assertRaises(EunomiaError) as cm:
            self.eu.list_vaults()
        e = cm.exception
        self.assertEqual((e.status, e.code, e.detail, e.trace_id), (403, "auth.forbidden", "nope", "abc"))

    def test_memory_tools(self):
        r = self.stub.routes
        r["POST /api/tools/memory_write"] = lambda b: (200, {"entity": {"id": "e"}, "memory": {"id": "m"}}, {})
        r["POST /api/tools/recall"] = lambda b: (200, {"results": [{"text": "t"}]}, {})
        r["POST /api/tools/entities_search"] = lambda b: (200, {"results": [{"name": "Ada"}], "total": 1}, {})
        self.assertEqual(self.eu.memory_write("Ada", "person", "hi", vault_id="V")["memory"]["id"], "m")
        self.assertEqual(self.stub.calls[0][2]["vault_id"], "V")
        self.assertEqual(self.eu.recall("q", vault_id="V")[0]["text"], "t")
        self.assertEqual(self.eu.entities_search("Ada", vault_id="V")[0]["name"], "Ada")

    def test_429_retries_after_retry_after(self):
        state = {"n": 0}

        def route(b):
            state["n"] += 1
            if state["n"] == 1:
                return 429, {"code": "rate.limited", "detail": "slow"}, {"Retry-After": "0.05"}
            return 200, {"results": []}, {}

        self.stub.routes["GET /api/vaults"] = route
        self.assertEqual(self.eu.list_vaults(), [])
        self.assertEqual(state["n"], 2)

    def test_429_gives_up_after_max_retries(self):
        self.stub.routes["GET /api/vaults"] = lambda b: (429, {"code": "rate.limited", "detail": "slow"}, {"Retry-After": "0"})
        eu = Eunomia(self.stub.url, "t", max_retries=2)
        with self.assertRaises(EunomiaError) as cm:
            eu.list_vaults()
        self.assertEqual(cm.exception.code, "rate.limited")
        self.assertEqual(len(self.stub.calls), 3)

    def test_write_memories_bulk_with_rate_limit_and_failures(self):
        seen = {"limited": False}
        lock = threading.Lock()

        def route(b):
            if b["text"] == "bad":
                return 400, {"code": "request.invalid", "detail": "bad"}, {}
            with lock:
                if b["text"] == "f3" and not seen["limited"]:
                    seen["limited"] = True
                    return 429, {"code": "rate.limited", "detail": "slow"}, {"Retry-After": "0.05"}
            return 200, {"entity": {}, "memory": {"id": b["text"]}}, {}

        self.stub.routes["POST /api/tools/memory_write"] = route
        eu = Eunomia(self.stub.url, "t")
        items = [{"subject_name": "S", "subject_kind": "person", "text": f"f{i}"} for i in range(6)]
        items.insert(2, {"subject_name": "S", "subject_kind": "person", "text": "bad"})
        items.append({"subject_name": "S"})
        out = eu.write_memories(items, vault_id="V", concurrency=3)
        self.assertEqual([o.index for o in out], list(range(len(items))))
        self.assertEqual([o.ok for o in out], [True, True, False, True, True, True, True, False])
        self.assertEqual(out[2].error.code, "request.invalid")
        self.assertEqual(out[-1].error.code, "client.bad_item")
        self.assertTrue(seen["limited"])
        self.assertTrue(all(c[2]["vault_id"] == "V" for c in self.stub.calls))

    def test_transport_error(self):
        eu = Eunomia("http://127.0.0.1:1", "t")
        with self.assertRaises(EunomiaError) as cm:
            eu.list_vaults()
        self.assertEqual(cm.exception.code, "client.transport")


URL, TOKEN = os.environ.get("EUNOMIA_URL"), os.environ.get("EUNOMIA_TOKEN")


@unittest.skipUnless(URL and TOKEN, "set EUNOMIA_URL and EUNOMIA_TOKEN to run")
class IntegrationTest(unittest.TestCase):
    def test_create_write_merge_recall(self):
        eu = Eunomia(URL, TOKEN)
        tag = uuid.uuid4().hex[:8]
        made = []
        try:
            a = eu.create_vault(f"sdk-py-a-{tag}")
            made.append(a.id)
            b = eu.create_vault(f"sdk-py-b-{tag}")
            made.append(b.id)
            res = eu.write_memories(
                [{"subject_name": f"Ada{tag}", "subject_kind": "person", "text": f"Ada{tag} likes green tea"}], vault_id=a.id)
            res += eu.write_memories(
                [{"subject_name": f"Grace{tag}", "subject_kind": "person", "text": f"Grace{tag} likes compilers"}], vault_id=b.id)
            self.assertTrue(all(r.ok for r in res), [r.error for r in res])
            merged = eu.merge_vaults(a.id, b.id, name=f"sdk-py-merged-{tag}")
            made.append(merged.id)
            self.assertEqual(merged.entities, 2)
            self.assertIn(merged.id, [v.id for v in eu.list_vaults()])
            hits = eu.recall("green tea", vault_id=merged.id)
            self.assertTrue(any(f"Ada{tag}" in h["text"] for h in hits), hits)
            names = [e["name"] for e in eu.entities_search(f"Grace{tag}", vault_id=merged.id)]
            self.assertIn(f"Grace{tag}", names)
            self.assertEqual(eu.rename_vault(merged.id, f"sdk-py-renamed-{tag}").name, f"sdk-py-renamed-{tag}")
            self.assertEqual(eu.clone_vault(merged.id, name=f"sdk-py-clone-{tag}").entities_copied, 2)
            made.append(next(v.id for v in eu.list_vaults() if v.name == f"sdk-py-clone-{tag}"))
        finally:
            for vid in made:
                try:
                    eu.delete_vault(vid)
                except EunomiaError:
                    pass


if __name__ == "__main__":
    unittest.main()

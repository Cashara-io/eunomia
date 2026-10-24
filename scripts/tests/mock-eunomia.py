"""Just enough of Eunomia's auth API for connect-agents.sh: login/register,
list/create/delete tokens. Prints its port, logs every request to argv[1]."""
import json, secrets, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

log = open(sys.argv[1], "a", buffering=1)
state = {"registered": False, "tokens": [], "n": 0}


class H(BaseHTTPRequestHandler):
    def _send(self, code, body):
        data = json.dumps(body).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def _body(self):
        if not hasattr(self, "_parsed"):
            n = int(self.headers.get("Content-Length") or 0)
            self._parsed = json.loads(self.rfile.read(n) or b"{}")
        return self._parsed

    def _peek_email(self):
        return self._body().get("email")

    def handle_any(self, method):
        log.write(f"{method} {self.path}\n")
        if self.path == "/api/auth/login" and method == "POST":
            ok = state["registered"] and self._body().get("password") == "correct-horse" and self._peek_email() == "a@b.co"
            return self._send(200 if ok else 401, {"email": "a@b.co"} if ok else {"detail": "no"})
        if self.path == "/api/auth/register" and method == "POST":
            if self._peek_email() == "closed@b.co":
                return self._send(403, {"code": "auth.forbidden", "detail": "Signup is closed on this server (SIGNUP). Ask the admin to create your account or to open signup."})
            if state["registered"]:
                return self._send(409, {"detail": "exists"})
            state["registered"] = True
            return self._send(200, {"email": "a@b.co"})
        if self.path == "/api/settings" and method == "GET":
            if not (self.headers.get("Authorization") or "").startswith("Bearer tok"):
                return self._send(401, {})
            return self._send(200, {"memory_skill": "---\nname: eunomia-memory\ndescription: test\n---\n\n# Eunomia memory\n\nRecall first."})
        if self.path == "/api/tools/recall" and method == "POST":
            q = self._body().get("query", "")
            hits = [{"id": "memory:1", "text": "Ada prefers\nasync updates.", "kind": "memory"}] if "ada" in q.lower() else []
            return self._send(200, {"results": hits})
        if self.path == "/api/auth/tokens" and method == "GET":
            return self._send(200, [{"id": t["id"], "name": t["name"]} for t in state["tokens"]])
        if self.path == "/api/auth/tokens" and method == "POST":
            state["n"] += 1
            t = {"id": f"api_token:t{state['n']}", "name": self._body()["name"], "token": f"tok{state['n']}_{secrets.token_urlsafe(8)}"}
            state["tokens"].append(t)
            return self._send(200, t)
        if self.path.startswith("/api/auth/tokens/") and method == "DELETE":
            tid = self.path.rsplit("/", 1)[1]
            state["tokens"] = [t for t in state["tokens"] if t["id"] != tid]
            return self._send(200, {"ok": True})
        self._send(404, {})

    do_GET = lambda self: self.handle_any("GET")
    do_POST = lambda self: self.handle_any("POST")
    do_DELETE = lambda self: self.handle_any("DELETE")

    def log_message(self, *a):
        pass


srv = HTTPServer(("127.0.0.1", 0), H)
print(srv.server_address[1], flush=True)
srv.serve_forever()

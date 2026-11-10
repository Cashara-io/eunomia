import { test, expect } from "@playwright/test";
import { randomUUID } from "node:crypto";
import { request as httpRequest, createServer, type Server } from "node:http";

import { refusedBeforeBodyCompletes } from "./proxy-request";

let backend: Server;
let bodyRequests = 0;
let confirmed = 0;
const fixture = randomUUID();
test.beforeAll(async ({ request }) => {
  // A backend double confirms credentials, counts body bytes, and echoes forwarding headers.
  backend = createServer(async (req, res) => {
    res.setHeader("x-proxy-fixture", fixture);
    if (req.method === "HEAD") {
      confirmed++;
      const allowed = req.headers.authorization === "Bearer valid" || (req.url !== "/mcp" && req.headers.cookie === "session=valid");
      const readOnly = req.headers.authorization === "Bearer readonly";
      res.setHeader("x-eunomia-body-limit", allowed ? String(151 * 1024 * 1024) : String(1024 * 1024));
      res.setHeader("x-eunomia-upload-allowed", allowed ? "1" : "0");
      res.writeHead(allowed || (readOnly && req.url === "/mcp") ? 200 : readOnly ? 403 : 401);
      res.end();
      return;
    }
    if (req.url === "/api/auth/me") {
      confirmed++;
      res.writeHead(req.headers.authorization === "Bearer valid" || req.headers.cookie === "session=valid" ? 200 : 401);
      res.end("{}");
      return;
    }
    let bytes = 0;
    let probe = "";
    try {
      for await (const chunk of req) {
        bytes += chunk.length;
        if (bytes < 1024) probe += chunk.toString();
      }
    } catch { return; }
    if (req.url === "/mcp" && probe === '{"jsonrpc":"2.0","id":"proxy-auth","method":"ping"}') {
      confirmed++;
      res.writeHead(["Bearer valid", "Bearer readonly"].includes(req.headers.authorization ?? "") ? 200 : 401);
      res.end("{}");
      return;
    }
    if (req.method === "POST") bodyRequests++;
    res.setHeader("content-type", "application/json");
    res.end(JSON.stringify({ bytes, forwarded: req.headers["x-forwarded-for"] ?? null, url: req.url }));
  });
  await new Promise<void>((resolve) => backend.listen(18191, "127.0.0.1", resolve));
  // Compile the handlers before testing the early-response deadline.
  // Prove both destinations before any fixture sends a POST.
  for (const path of ["/mcp", "/api/documents"]) {
    const probe = await request.get(path);
    expect(probe.headers()["x-proxy-fixture"]).toBe(fixture);
  }
});
test.afterAll(() => backend.close());

for (const path of ["/mcp", "/api/documents?filename=large.csv"]) {
  for (const credential of [undefined, "Bearer fake"]) {
    for (const chunked of [false, true]) {
      test(`${path}: refuses ${credential ?? "anonymous"} ${chunked ? "chunked" : "20 MiB"} body before it arrives`, async () => {
        const before = bodyRequests;
        const status = await refusedBeforeBodyCompletes({ url: `http://127.0.0.1:18190${path}`, credential, chunked });
        expect(status).toBe(401);
        expect(bodyRequests).toBe(before);
      });
    }
  }
  test(`${path}: refuses a read-only 20 MiB body before buffering`, async () => {
    const before = bodyRequests;
    expect(await refusedBeforeBodyCompletes({ url: `http://127.0.0.1:18190${path}`, credential: "Bearer readonly" })).toBe(403);
    expect(bodyRequests).toBe(before);
  });
  if (path === "/mcp") {
    test("MCP read-only chunked bodies stop at the small limit", async () => {
      expect(await refusedBeforeBodyCompletes({ url: "http://127.0.0.1:18190/mcp", credential: "Bearer readonly", chunked: true, bytes: 2 * 1024 * 1024 })).toBe(413);
    });
  }
  test(`${path}: times out a scoped sender that never sends EOF`, async () => {
    expect(await refusedBeforeBodyCompletes({ url: `http://127.0.0.1:18190${path}`, credential: "Bearer valid", chunked: true, bytes: 2 * 1024 * 1024 })).toBe(408);
  });
  test(`${path}: allows an honest 128 KiB/s upload beyond the old test deadline`, async () => {
    const result = await timedUpload(path, 128 * 1024, 2 * 1024 * 1024);
    expect(result.status).toBe(200);
    expect(JSON.parse(result.body).bytes).toBe(2 * 1024 * 1024);
  });
  test(`${path}: cuts off a sender below the minimum rate`, async () => {
    const result = await timedUpload(path, 8 * 1024, 2 * 1024 * 1024);
    expect(result.status).toBe(408);
    expect(result.sent).toBeLessThan(2 * 1024 * 1024);
  });
  test(`${path}: streams an authenticated 2 MiB body intact`, async ({ request }) => {
    const credentials: Record<string, string>[] = [{ authorization: "Bearer valid" }];
    if (path.startsWith("/api/")) credentials.push({ cookie: "session=valid" });
    for (const headers of credentials) {
      const before = confirmed;
      const res = await request.post(path, {
        headers: { ...headers, "x-forwarded-for": "198.51.100.10" },
        data: Buffer.alloc(2 * 1024 * 1024, 120),
      });
      expect(res.status()).toBe(200);
      expect(await res.json()).toEqual({ bytes: 2 * 1024 * 1024, forwarded: null, url: path });
      expect(confirmed).toBe(before + 1);
    }
  });
}

function timedUpload(path: string, rate: number, size: number) {
  return new Promise<{ status: number | undefined; body: string; sent: number }>((resolve, reject) => {
    let sent = 0;
    const req = httpRequest(`http://127.0.0.1:18190${path}`, { method:"POST", headers:{ authorization:"Bearer valid", "content-length":String(size) } }, (res) => {
      let body = "";
      res.on("data", (chunk) => { body += chunk; });
      res.on("end", () => { clearInterval(timer); resolve({ status:res.statusCode, body, sent }); req.destroy(); });
    });
    const timer = setInterval(() => {
      const length = Math.min(rate / 8, size - sent);
      if (length > 0) { req.write(Buffer.alloc(length, 120)); sent += length; }
      if (sent === size) { clearInterval(timer); req.end(); }
    }, 125);
    req.on("error", (err) => { clearInterval(timer); reject(err); });
    req.setTimeout(25_000, () => req.destroy(new Error("upload fixture timed out")));
  });
}

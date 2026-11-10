import { test, expect } from "@playwright/test";
import { randomUUID } from "node:crypto";
import { createServer, request as httpRequest, type Server } from "node:http";

import { refusedBeforeBodyCompletes } from "./proxy-request";

let backend: Server;
let bodyRequests = 0;
let confirmed = 0;
const fixture = randomUUID();
const firstChunks = new Map<string, () => void>();
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
        const uploadId = req.headers["x-proxy-upload-id"];
        if (typeof uploadId === "string") {
          firstChunks.get(uploadId)?.();
          firstChunks.delete(uploadId);
        }
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
  await new Promise<void>((resolve) => backend.listen(28191, "127.0.0.1", resolve));
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
      test(`${path}: refuses ${credential ?? "anonymous"} ${chunked ? "chunked" : "20 MiB"} body before it arrives`, async ({ baseURL }) => {
        const before = bodyRequests;
        const status = await refusedBeforeBodyCompletes({ url: `${baseURL}${path}`, credential, chunked });
        expect(status).toBe(401);
        expect(bodyRequests).toBe(before);
      });
    }
  }
  test(`${path}: refuses a read-only 20 MiB body before buffering`, async ({ baseURL }) => {
    const before = bodyRequests;
    expect(await refusedBeforeBodyCompletes({ url: `${baseURL}${path}`, credential: "Bearer readonly" })).toBe(403);
    expect(bodyRequests).toBe(before);
  });
  if (path === "/mcp") {
    test("MCP read-only chunked bodies stop at the small limit", async ({ baseURL }) => {
      expect(await refusedBeforeBodyCompletes({ url: `${baseURL}/mcp`, credential: "Bearer readonly", chunked: true, bytes: 2 * 1024 * 1024 })).toBe(413);
    });
  }
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
  test(`${path}: forwards the first chunk before the sender sends the rest`, async ({ baseURL }) => {
    const result = await observedUpload(`${baseURL}${path}`, true);
    expect(result.status).toBe(200);
    expect(JSON.parse(result.body).bytes).toBe(2 * 1024 * 1024);
  });
  test(`${path}: returns 408 to an authenticated sender that withholds EOF`, async ({ baseURL }) => {
    const result = await observedUpload(`${baseURL}${path}`, false);
    expect(result.status).toBe(408);
  });
}

async function observedUpload(url: string, sendEOF: boolean) {
  const uploadId = randomUUID();
  const firstChunk = new Promise<void>((resolve) => firstChunks.set(uploadId, resolve));
  const req = httpRequest(url, { method: "POST", headers: {
    authorization: "Bearer valid", "transfer-encoding": "chunked", "x-proxy-upload-id": uploadId,
  } });
  const response = new Promise<{ status: number | undefined; body: string }>((resolve, reject) => {
    req.on("response", (res) => {
      let body = "";
      res.on("data", (chunk) => { body += chunk; });
      res.on("error", reject);
      res.on("end", () => resolve({ status: res.statusCode, body }));
    });
    req.on("error", reject);
  });
  // This is only a deadlock guard. No assertion depends on elapsed wall time.
  req.setTimeout(25_000, () => req.destroy(new Error("upload fixture made no progress")));
  try {
    req.write(Buffer.alloc(256 * 1024, 120));
    await Promise.race([
      firstChunk,
      response.then(() => { throw new Error("proxy responded before forwarding the first chunk"); }),
    ]);
    expect(req.writableEnded).toBe(false);
    // A buffering proxy cannot reach here: the backend has to receive data
    // before the client releases the remaining body and EOF.
    if (sendEOF) req.end(Buffer.alloc(2 * 1024 * 1024 - 256 * 1024, 120));
    return await response;
  } finally {
    req.destroy();
    firstChunks.delete(uploadId);
  }
}

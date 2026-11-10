import { test, expect } from "@playwright/test";
import { createServer, type Server } from "node:http";

import { refusedBeforeBodyCompletes } from "./proxy-request";

let backend: Server;
let bodyRequests = 0;
let confirmed = 0;
test.beforeAll(async ({ request }) => {
  // A backend double confirms credentials, counts body bytes, and echoes forwarding headers.
  backend = createServer(async (req, res) => {
    if (req.url === "/api/auth/me") {
      confirmed++;
      res.writeHead(req.headers.authorization === "Bearer valid" || req.headers.cookie === "session=valid" ? 200 : 401);
      res.end("{}");
      return;
    }
    let bytes = 0;
    let probe = "";
    for await (const chunk of req) {
      bytes += chunk.length;
      if (bytes < 1024) probe += chunk.toString();
    }
    if (req.url === "/mcp" && probe === '{"jsonrpc":"2.0","id":"proxy-auth","method":"ping"}') {
      confirmed++;
      res.writeHead(req.headers.authorization === "Bearer valid" ? 200 : 401);
      res.end("{}");
      return;
    }
    if (req.method === "POST") bodyRequests++;
    res.setHeader("content-type", "application/json");
    res.end(JSON.stringify({ bytes, forwarded: req.headers["x-forwarded-for"] ?? null, url: req.url }));
  });
  await new Promise<void>((resolve) => backend.listen(18191, "127.0.0.1", resolve));
  // Compile the handlers before testing the early-response deadline.
  await request.get("/mcp");
  await request.get("/api/documents");
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

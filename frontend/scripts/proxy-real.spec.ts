import { test, expect } from "@playwright/test";
import { createHash } from "node:crypto";
import { refusedBeforeBodyCompletes } from "./proxy-request";

test.beforeAll(async ({ request }) => {
  const marker = process.env.PROXY_TEST_EXPECTED_FIXTURE;
  expect(marker).toBeTruthy();
  // Verify both a config rewrite and a streaming route before any write. Refuse a stale build.
  for (const path of ["/readyz", "/api/documents"]) {
    const probe = await request.get(path);
    expect(probe.headers()["x-eunomia-proxy-fixture"]).toBe(marker);
  }
});

test("real backend confirms credentials before intact 2 MiB HTTP and MCP uploads", async ({ request, baseURL }) => {
  const registered = await request.post("/api/auth/register", {
    data: { email: `proxy-${Date.now()}@example.com`, password: "correct-horse-battery-staple" },
  });
  expect(registered.status()).toBe(200);
  const tokenResponse = await request.post("/api/auth/tokens", { data: { name: "proxy proof" } });
  expect(tokenResponse.status()).toBe(200);
  const { token, id } = await tokenResponse.json();
  const text = "upload proof\n".repeat(Math.ceil(2 * 1024 * 1024 / 13)).slice(0, 2 * 1024 * 1024);
  const upload = await request.post("/api/documents?filename=large.txt", {
    headers: { "content-type": "text/plain" }, data: text,
  });
  expect(upload.status(), await upload.text()).toBe(200);
  const document = await upload.json();
  const download = await request.get(`/api/documents/${document.id}/download`);
  expect(await download.text()).toBe(text);

  // Drop cookies to prove the bearer credential alone works through the same frontend.
  const mcp = await request.post("/mcp", {
    headers: { authorization: `Bearer ${token}`, cookie: "" },
    data: { jsonrpc: "2.0", id: 1, method: "tools/call", params: {
      name: "document_upload", arguments: { filename: "mcp-large.txt", text },
    } },
  });
  expect(mcp.status()).toBe(200);
  const rpc = await mcp.json();
  expect(rpc.error).toBeUndefined();
  expect(rpc.result.isError).not.toBe(true);
  const uploaded = JSON.parse(rpc.result.content[0].text);
  expect(uploaded.error).toBeUndefined();
  const mcpDownload = await request.get(`/api/documents/${uploaded.id}/download`);
  expect(await mcpDownload.text()).toBe(text);

  // OAuth tokens deliberately do not authenticate /api/auth/me. The MCP probe must use /mcp.
  const redirect = "http://localhost:18201/callback";
  const verifier = "v".repeat(64);
  const registeredClient = await request.post("/oauth/register", { data: {
    client_name: "proxy proof", redirect_uris: [redirect], token_endpoint_auth_method: "none",
  } });
  expect(registeredClient.status()).toBe(201);
  const { client_id } = await registeredClient.json();
  const discovery = await (await request.get("/.well-known/oauth-protected-resource/mcp")).json();
  const consent = await request.post("/api/oauth/consent", { data: {
    response_type: "code", client_id, redirect_uri: redirect, resource: discovery.resource,
    code_challenge: createHash("sha256").update(verifier).digest("base64url"),
    code_challenge_method: "S256", state: "proxy-proof", approve: true,
  } });
  expect(consent.status(), await consent.text()).toBe(200);
  const code = new URL((await consent.json()).redirect_to).searchParams.get("code");
  expect(code).toBeTruthy();
  const exchanged = await request.post("/oauth/token", { form: {
    grant_type: "authorization_code", client_id, code: code ?? "", redirect_uri: redirect,
    code_verifier: verifier, resource: discovery.resource,
  } });
  expect(exchanged.status(), await exchanged.text()).toBe(200);
  const { access_token } = await exchanged.json();
  const oauthUpload = await request.post("/mcp", {
    headers: { authorization: `Bearer ${access_token}`, cookie: "" },
    data: { jsonrpc: "2.0", id: 2, method: "tools/call", params: {
      name: "document_upload", arguments: { filename: "oauth-large.txt", text },
    } },
  });
  expect(oauthUpload.status()).toBe(200);
  const oauthResult = JSON.parse((await oauthUpload.json()).result.content[0].text);
  expect(oauthResult.error).toBeUndefined();
  const oauthDownload = await request.get(`/api/documents/${oauthResult.id}/download`);
  expect(await oauthDownload.text()).toBe(text);
  const readTokenResponse = await request.post("/api/auth/tokens", { data: { name: "read-only proof", scopes: ["memory:read"] } });
  expect(readTokenResponse.status()).toBe(200);
  const { token: readToken } = await readTokenResponse.json();
  for (const path of ["/mcp", "/api/documents?filename=readonly.txt"]) {
    expect(await refusedBeforeBodyCompletes({ url: `${baseURL}${path}`, credential: `Bearer ${readToken}` })).toBe(403);
    expect(await refusedBeforeBodyCompletes({ url: `${baseURL}${path}`, credential: `Bearer ${token}`, chunked: true, bytes: 2 * 1024 * 1024, timeoutMs: 25_000 })).toBe(408);
  }
  // The timed-out stream is cancelled upstream, so a later upload can acquire the same slot.
  const afterTimeout = await request.post("/api/documents?filename=after-timeout.txt", {
    headers: { authorization: `Bearer ${token}`, cookie: "" }, data: text,
  });
  expect(afterTimeout.status(), await afterTimeout.text()).toBe(200);
  const revoked = await request.delete(`/api/auth/tokens/${id}`);
  expect(revoked.ok()).toBe(true);
  expect(await refusedBeforeBodyCompletes({ url: `${baseURL}/api/documents?filename=revoked.txt`, credential: `Bearer ${token}` })).toBe(401);
});

test("real backend refuses missing and fake credentials before a 20 MiB body arrives", async ({ request, baseURL }) => {
  for (const path of ["/mcp", "/api/documents?filename=anonymous.txt"]) {
    await request.get(path);
    for (const credential of [undefined, "Bearer fake"]) {
      for (const chunked of [false, true]) {
        expect(await refusedBeforeBodyCompletes({ url: `${baseURL}${path}`, credential, chunked })).toBe(401);
      }
    }
  }
});

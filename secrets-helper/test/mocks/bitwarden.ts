// A stand-in for Bitwarden's identity and API servers (a self-hosted layout: /identity and /api under one
// URL), doing the client-side crypto the real ones expect: the login payload is encrypted with the access
// token's key, the secret's value with the organisation key.
import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { randomBytes } from "node:crypto";
import { encrypt, keyFromB64, tokenKey } from "../../src/providers/bitwarden_crypto.js";

export const TOKEN_ID = "6b2f0a8e-1c3d-4e5f-8a9b-0c1d2e3f4a5b";
export const CLIENT_SECRET = "e2eClientSecretNotReal0123456789";
export const SEED = Buffer.from("00112233445566778899aabbccddeeff", "hex");
export const ACCESS_TOKEN = `0.${TOKEN_ID}.${CLIENT_SECRET}:${SEED.toString("base64")}`;
export const SECRET_ID = "3f1c2b4a-5d6e-4f70-8a91-b2c3d4e5f607";
export const DENIED_ID = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
export const VALUE = "bws-canary-secret-value-42";

export async function mockBitwarden(bind = "127.0.0.1", host = "127.0.0.1") {
  const hits: string[] = [];
  const orgKeyB64 = randomBytes(64).toString("base64");
  const org = keyFromB64(orgKeyB64);
  const bearer = "bw-bearer-" + randomBytes(8).toString("hex");
  const server: Server = createServer(async (req, res) => {
    hits.push(`${req.method} ${req.url}`);
    const json = (status: number, body: unknown) => {
      res.writeHead(status, { "content-type": "application/json" });
      res.end(JSON.stringify(body));
    };
    const chunks: Buffer[] = [];
    for await (const c of req) chunks.push(c as Buffer);
    const path = (req.url ?? "").split("?")[0]!;
    if (req.method === "POST" && path === "/identity/connect/token") {
      const form = new URLSearchParams(Buffer.concat(chunks).toString());
      if (form.get("grant_type") !== "client_credentials" || form.get("scope") !== "api.secrets") return json(400, { error: "invalid_request" });
      if (form.get("client_id") !== TOKEN_ID || form.get("client_secret") !== CLIENT_SECRET) return json(400, { error: "invalid_client" });
      return json(200, { access_token: bearer, expires_in: 3600, token_type: "Bearer", encrypted_payload: encrypt(JSON.stringify({ encryptionKey: orgKeyB64 }), tokenKey(SEED)) });
    }
    const m = path.match(/^\/api\/secrets\/([0-9a-f-]+)$/);
    if (req.method === "GET" && m) {
      if (req.headers.authorization !== `Bearer ${bearer}`) return json(401, { message: "unauthorised" });
      if (m[1] === DENIED_ID) return json(403, { message: "forbidden" });
      if (m[1] !== SECRET_ID) return json(404, { message: "Resource not found." });
      return json(200, { id: SECRET_ID, organizationId: "11111111-2222-4333-8444-555555555555", key: encrypt("square-key", org), value: encrypt(VALUE, org), note: encrypt("", org) });
    }
    return json(404, { message: "not found" });
  });
  await new Promise<void>((r) => server.listen(0, bind, r));
  const port = (server.address() as AddressInfo).port;
  return { base: `http://${host}:${port}`, hits, close: () => new Promise<void>((r) => server.close(() => r())) };
}

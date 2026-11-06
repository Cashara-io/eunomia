// A stand-in for a customer's 1Password Connect server: the three GET routes the sidecar uses, behind a
// Bearer token. Data: vault "Ops" with item "Square" (a top-level access_token, and a section "api"
// holding a field "secret") and an item "Twin" twice (ambiguous).
import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";

export const CONNECT_TOKEN = "connect-token-e2e-0123456789";
export const VALUES = { access: "sq0atp-canary-access-token-value", secret: "sq0csp-canary-section-secret-value" };

export type Mock = { base: string; hits: string[]; close: () => Promise<void> };

export async function mockConnect(bind = "127.0.0.1", host = "127.0.0.1"): Promise<Mock> {
  const hits: string[] = [];
  const vaults = [{ id: "vlt0ops", name: "Ops" }, { id: "vlt1dev", name: "Dev" }];
  const items: Record<string, Array<{ id: string; title: string }>> = {
    vlt0ops: [{ id: "itm0sq", title: "Square" }, { id: "itm1tw", title: "Twin" }, { id: "itm2tw", title: "Twin" }],
    vlt1dev: [],
  };
  const full: Record<string, unknown> = {
    itm0sq: {
      id: "itm0sq",
      title: "Square",
      sections: [{ id: "sec0api", label: "api" }],
      fields: [
        { id: "username", label: "username", value: "ops@example.com" },
        { id: "fld0", label: "access_token", value: VALUES.access },
        { id: "fld1", label: "secret", value: VALUES.secret, section: { id: "sec0api" } },
        { id: "fld2", label: "empty", value: "" },
      ],
    },
  };
  const server: Server = createServer((req, res) => {
    hits.push(`${req.method} ${req.url}`);
    const json = (status: number, body: unknown) => {
      res.writeHead(status, { "content-type": "application/json" });
      res.end(JSON.stringify(body));
    };
    if (req.headers.authorization !== `Bearer ${CONNECT_TOKEN}`) return json(401, { status: 401, message: "Invalid token signature" });
    const path = (req.url ?? "").split("?")[0]!;
    let m;
    if (path === "/v1/vaults") return json(200, vaults);
    if ((m = path.match(/^\/v1\/vaults\/([^/]+)\/items$/))) return items[m[1]!] ? json(200, items[m[1]!]) : json(404, { status: 404, message: "vault not found" });
    if ((m = path.match(/^\/v1\/vaults\/([^/]+)\/items\/([^/]+)$/))) return full[m[2]!] ? json(200, full[m[2]!]) : json(404, { status: 404, message: "item not found" });
    return json(404, { status: 404, message: "not found" });
  });
  await new Promise<void>((r) => server.listen(0, bind, r));
  const port = (server.address() as AddressInfo).port;
  return { base: `http://${host}:${port}`, hits, close: () => new Promise((r) => server.close(() => r())) };
}

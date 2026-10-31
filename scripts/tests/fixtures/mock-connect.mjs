// A stand-in 1Password Connect server for scripts/tests/secrets-helper-compose.test.sh: the routes the
// secrets-helper uses, behind a Bearer token (MOCK_CONNECT_TOKEN). Test data only, no real secret.
import { createServer } from "node:http";

const token = process.env.MOCK_CONNECT_TOKEN ?? "";
const vaults = [{ id: "vlt0ops", name: "Ops" }, { id: "vlt1dev", name: "Dev" }];
const items = { vlt0ops: [{ id: "itm0sq", title: "Square" }], vlt1dev: [] };
const item = { id: "itm0sq", title: "Square", sections: [], fields: [{ id: "f0", label: "access_token", value: "compose-canary-value-91d2" }] };

createServer((req, res) => {
  const json = (status, body) => {
    res.writeHead(status, { "content-type": "application/json" });
    res.end(JSON.stringify(body));
  };
  if (req.headers.authorization !== `Bearer ${token}`) return json(401, { message: "Invalid token" });
  const path = (req.url ?? "").split("?")[0];
  let m;
  if (path === "/v1/vaults") return json(200, vaults);
  if ((m = path.match(/^\/v1\/vaults\/([^/]+)\/items$/))) return items[m[1]] ? json(200, items[m[1]]) : json(404, {});
  if ((m = path.match(/^\/v1\/vaults\/[^/]+\/items\/([^/]+)$/))) return m[1] === item.id ? json(200, item) : json(404, {});
  return json(404, {});
}).listen(8080, "0.0.0.0", () => console.log("mock connect on :8080"));

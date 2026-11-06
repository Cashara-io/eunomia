// secrets-helper: Eunomia's credential broker sidecar. Listens on the internal compose network only
// (docker-compose.yml publishes no port). Env: SECRETS_HELPER_KEY (the shared internal key; unset: the
// dedicated internal network is the only gate), SECRETS_HELPER_PORT (default 8080).

import { makeServer } from "./server.js";
import { readFileSync } from "node:fs";

const key = (process.env.SECRETS_HELPER_KEY ?? "").trim();
const port = Number(process.env.SECRETS_HELPER_PORT ?? 8080);
let version = "dev";
try {
  version = JSON.parse(readFileSync(new URL("../../package.json", import.meta.url), "utf8")).version ?? "dev";
} catch {
  // a missing package.json only loses the version label
}

const log = (line: string) => process.stdout.write(`${new Date().toISOString()} ${line}\n`);
if (!key) log("SECRETS_HELPER_KEY is not set: accepting calls on the internal network alone (the updater adds a key on the next update)");
const server = makeServer({ key, log, version });
server.headersTimeout = 5_000;
server.requestTimeout = 15_000;
server.listen(port, "0.0.0.0", () => log(`secrets-helper ${version} listening on :${port} (api 1)`));
for (const sig of ["SIGTERM", "SIGINT"] as const) process.on(sig, () => server.close(() => process.exit(0)));

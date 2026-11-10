import { createServer } from "node:http";
import { parseArgs } from "node:util";
import next from "next";

const { values } = parseArgs({ options: {
  dev: { type: "boolean", default: false },
  hostname: { type: "string", default: "0.0.0.0" },
  port: { type: "string", default: process.env.PORT || "3000" },
} });
const port = Number(values.port);
const app = next({ dev: values.dev, hostname: values.hostname, port });
await app.prepare();
// Node's default requestTimeout is five minutes, including the incoming body.
// Leave room for the fifteen-minute body budget and the authentication check.
const server = createServer({ requestTimeout: 960_000 }, app.getRequestHandler());
server.listen(port, values.hostname);
for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, () => { server.closeAllConnections(); server.close(); void app.close().then(() => process.exit(0)); });
}

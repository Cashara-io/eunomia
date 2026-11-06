// The real entry point in its own process: after a call the token is neither in the process's
// environment nor in any file under the directories it may write to (its cwd, HOME, TMPDIR).
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtempSync, readdirSync, readFileSync, statSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createServer } from "node:net";
import { mockConnect, CONNECT_TOKEN, VALUES } from "./mocks/connect.js";

const freePort = () =>
  new Promise<number>((resolve) => {
    const s = createServer().listen(0, "127.0.0.1", () => {
      const p = (s.address() as { port: number }).port;
      s.close(() => resolve(p));
    });
  });

function filesUnder(dir: string): string[] {
  const out: string[] = [];
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    const st = statSync(p);
    if (st.isDirectory()) out.push(...filesUnder(p));
    else out.push(p);
  }
  return out;
}

test("no token in the environment or on disk after a call", async () => {
  const connect = await mockConnect();
  const home = mkdtempSync(join(tmpdir(), "sh-home-"));
  const port = await freePort();
  const main = new URL("../src/main.js", import.meta.url).pathname;
  const child = spawn(process.execPath, [main], { cwd: home, env: { PATH: process.env.PATH, HOME: home, TMPDIR: home, SECRETS_HELPER_PORT: String(port), SECRETS_HELPER_KEY: "k" }, stdio: ["ignore", "pipe", "pipe"] });
  let out = "";
  child.stdout.on("data", (d) => (out += d));
  child.stderr.on("data", (d) => (out += d));
  try {
    for (let i = 0; i < 100 && !out.includes("listening"); i++) await new Promise((r) => setTimeout(r, 50));
    const res = await fetch(`http://127.0.0.1:${port}/v1/resolve`, {
      method: "POST",
      headers: { authorization: "Bearer k", "content-type": "application/json" },
      body: JSON.stringify({ provider: { kind: "onepassword_connect", token: CONNECT_TOKEN, host: connect.base }, refs: ["op://Ops/Square/access_token"] }),
    });
    assert.equal(((await res.json()) as any).results[0].value, VALUES.access);
    for (const f of filesUnder(home)) {
      const text = readFileSync(f, "latin1");
      assert.ok(!text.includes(CONNECT_TOKEN) && !text.includes(VALUES.access), `${f} holds the token or value`);
    }
    const environ = `/proc/${child.pid}/environ`;
    if (existsSync(environ)) assert.ok(!readFileSync(environ, "latin1").includes(CONNECT_TOKEN), "the token is in the environment");
    assert.ok(!out.includes(CONNECT_TOKEN) && !out.includes(VALUES.access), "the token or value was logged");
  } finally {
    child.kill("SIGTERM");
    await connect.close();
  }
});

import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { after, before, beforeEach, describe, it } from "node:test";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import { connectAgentsScript, init, sync, type InitOptions } from "../src/init.js";
import { credentialsPath, loadCredentials } from "../src/project.js";
import { defaultsPrompter, type Prompter } from "../src/prompt.js";

// ---------------------------------------------------------------------------
// A small fake Eunomia: the routes init and sync call, with real-ish auth rules.
// ---------------------------------------------------------------------------

type Tok = { id: string; name: string; token: string; scopes: string[]; vault_id: string | null };
type Skill = { name: string; description: string; body: string; vault: string };
const fake = {
  authMode: "password",
  vaults: [] as { id: string; name: string; kind: string; role: string }[],
  tokens: [] as Tok[],
  skills: [] as Skill[],
  calls: [] as { method: string; path: string; body: any }[],
  n: 0,
};
const PASSWORD = "s3cret-pass";
/** The package folder (tests run from dist-test/test). */
const PKG = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const USER_TOKEN = "eu_user_full_token";

function reset() {
  fake.authMode = "password";
  fake.vaults = [{ id: "vault:personal", name: "Personal", kind: "personal", role: "owner" }];
  fake.tokens = [{ id: "api_token:user", name: "mine", token: USER_TOKEN, scopes: ["memory:read", "memory:write", "vaults:admin", "connectors"], vault_id: null }];
  fake.skills = [];
  fake.calls = [];
}

function handle(method: string, path: string, body: any, headers: IncomingMessage["headers"]): [number, unknown, Record<string, string>?] {
  if (path === "/healthz") return [200, { status: "ok" }];
  if (path === "/api/auth/bootstrap") return [200, { has_users: true, auth_mode: fake.authMode }];
  if (path === "/api/auth/login") {
    if (fake.authMode === "oidc") return [403, { code: "auth.password_disabled", detail: "off" }];
    if (body.email !== "ada@example.com" || body.password !== PASSWORD) return [401, { code: "auth.unauthorized", detail: "Invalid email or password." }];
    return [200, { id: "user:1", email: body.email }, { "set-cookie": "eunomia_session=sess123; HttpOnly; Path=/; SameSite=Lax" }];
  }
  if (path === "/api/auth/logout") return [200, { ok: true }];
  // authenticate
  const bearer = /^Bearer (.+)$/.exec(headers.authorization ?? "")?.[1];
  const session = headers.cookie === "eunomia_session=sess123";
  const tok = bearer ? fake.tokens.find((t) => t.token === bearer) : undefined;
  if (!session && !tok) return [401, { code: "auth.unauthorized", detail: "Not authenticated." }];
  const restricted = tok?.vault_id ?? null;
  const can = (s: string) => session || tok!.scopes.includes(s) || (s === "memory:read" && tok!.scopes.includes("memory:write"));
  const deny = (d: string): [number, unknown] => [403, { code: "auth.scope", detail: d }];

  if (path.startsWith("/api/auth/tokens")) {
    if (!can("vaults:admin") || restricted) return deny("this route is not available to a vault-restricted token");
    if (method === "GET") return [200, fake.tokens.map(({ token: _t, ...rest }) => rest)];
    if (method === "DELETE") {
      const id = decodeURIComponent(path.split("/").pop()!);
      fake.tokens = fake.tokens.filter((t) => t.id !== id);
      return [200, { ok: true }];
    }
    const t: Tok = { id: `api_token:${++fake.n}`, name: body.name, token: `eu_project_${fake.n}_${Math.random().toString(36).slice(2)}`, scopes: body.scopes, vault_id: body.vault_id ?? null };
    fake.tokens.push(t);
    return [200, t];
  }
  if (path === "/api/vaults") {
    if (method === "GET") return [200, { results: fake.vaults.filter((v) => !restricted || v.id === restricted) }];
    if (!can("vaults:admin") || restricted) return deny("no");
    const v = { id: `vault:${++fake.n}`, name: body.name, kind: body.kind, role: "owner" };
    fake.vaults.push(v);
    return [200, v];
  }
  const tool = /^\/api\/tools\/(\w+)$/.exec(path)?.[1];
  if (tool) {
    const err = (error: string, code: string): [number, unknown] => [200, { error, code }];
    if (restricted && body.vault_id !== restricted) return err(`this token is restricted to vault ${restricted}`, "auth.scope");
    if (tool === "skill_get") {
      const s = fake.skills.find((x) => x.vault === body.vault_id && x.name === body.name);
      return s ? [200, { id: "skill:1", ...s }] : err(`no skill named "${body.name}"`, "resource.not_found");
    }
    if (tool === "skill_save") {
      if (!can("memory:write")) return deny("this token does not have the memory:write scope");
      if (body.name === "server-rejects") return err("description must be 1 to 1024 characters", "validation.invalid");
      fake.skills = fake.skills.filter((x) => !(x.vault === body.vault_id && x.name === body.name));
      fake.skills.push({ name: body.name, description: body.description, body: body.body, vault: body.vault_id });
      return [200, { id: "skill:1", name: body.name }];
    }
    if (tool === "skill_find") return [200, { skills: fake.skills.filter((x) => x.vault === body.vault_id).map(({ name, description }) => ({ name, description })) }];
  }
  return [404, { code: "x.none", detail: `no route ${method} ${path}` }];
}

const server = createServer(async (req: IncomingMessage, res: ServerResponse) => {
  let raw = "";
  for await (const c of req) raw += c;
  const body = raw ? JSON.parse(raw) : undefined;
  fake.calls.push({ method: req.method!, path: req.url!, body });
  const [status, payload, headers] = handle(req.method!, req.url!, body, req.headers);
  res.writeHead(status, { "content-type": "application/json", ...headers });
  res.end(JSON.stringify(payload));
});

let URL_ = "";
before(async () => {
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  URL_ = `http://127.0.0.1:${(server.address() as { port: number }).port}`;
});
after(() => server.close());

// ---------------------------------------------------------------------------

let repo = "";
let home = "";
let lines: string[] = [];
let agentRuns: { args: string[]; env: NodeJS.ProcessEnv }[] = [];
beforeEach(() => {
  reset();
  const tmp = mkdtempSync(join(tmpdir(), "create-eunomia-"));
  repo = join(tmp, "my-repo");
  home = join(tmp, "home");
  mkdirSync(repo);
  mkdirSync(home);
  lines = [];
  agentRuns = [];
});

const env = () => ({ HOME: home, PATH: process.env.PATH });
function opts(extra: Partial<InitOptions> = {}): InitOptions {
  return {
    cwd: repo,
    url: URL_,
    email: "ada@example.com",
    password: PASSWORD,
    yes: true,
    agents: "none",
    env: env(),
    prompt: defaultsPrompter(),
    log: (l) => lines.push(l),
    connectAgents: (args, e) => (agentRuns.push({ args, env: e }), true),
    ...extra,
  };
}

/** Answers questions in order, by matching each against the expected prompt text. */
function scripted(answers: [RegExp, string][]): Prompter {
  const next = (q: string) => {
    const a = answers.shift();
    assert.ok(a, `unexpected question: ${q}`);
    assert.match(q, a[0]);
    return a[1];
  };
  return {
    ask: async (q) => next(q),
    secret: async (q) => next(q),
    confirm: async (q) => next(q) === "y",
    close() {},
  };
}

function filesUnder(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e) => (e.isDirectory() ? filesUnder(join(dir, e.name)) : [join(dir, e.name)]));
}

function assertNoSecretsIn(dir: string) {
  const secrets = [PASSWORD, USER_TOKEN, ...fake.tokens.map((t) => t.token), "sess123"];
  for (const f of filesUnder(dir)) {
    const text = readFileSync(f, "utf8");
    for (const s of secrets) assert.ok(!text.includes(s), `${f} contains a secret`);
  }
}

const toml = () => readFileSync(join(repo, ".eunomia", "eunomia.toml"), "utf8");
const skill = (name: string, description: string, body = "Do the thing.") =>
  writeFileSync(join(repo, ".eunomia", "skills", `${name}.md`), `---\nname: ${name}\ndescription: ${description}\n---\n\n${body}\n`);

describe("init", () => {
  it("signs in, creates the vault, mints a narrow token outside the repo and writes .eunomia/", async () => {
    assert.equal(await init(opts({ vault: "Acme" })), 0);
    const vault = fake.vaults.find((v) => v.name === "Acme")!;
    assert.ok(vault);
    const minted = fake.tokens.filter((t) => t.id !== "api_token:user");
    assert.equal(minted.length, 1);
    assert.deepEqual(minted[0].scopes, ["memory:write"]);
    assert.equal(minted[0].vault_id, vault.id);
    assert.match(minted[0].name, /^create-eunomia: Acme \(/);

    assert.match(toml(), /\[server\]\nurl = "http:\/\/127\.0\.0\.1:\d+"\n\n\[vault\]\nid = "vault:\d+"\nname = "Acme"\n/);
    assert.match(toml(), /^# .*\n# Docs: https:\/\/github.com\/Cashara-io\/eunomia\/blob\/main\/docs\/projects.md/);
    assert.ok(statSync(join(repo, ".eunomia", "skills", "README.md")).isFile());
    assert.ok(statSync(join(repo, ".eunomia", ".gitignore")).isFile());

    const p = credentialsPath(URL_, vault.id, env());
    assert.ok(p.startsWith(join(home, ".config", "eunomia", "projects")));
    assert.equal(statSync(p).mode & 0o777, 0o600);
    assert.equal(loadCredentials(URL_, vault.id, env())!.token, minted[0].token);
    assertNoSecretsIn(repo);
    assert.ok(fake.calls.some((c) => c.path === "/api/auth/logout"));
  });

  it("is safe to run again: keeps files and token, and changes the vault only when asked", async () => {
    await init(opts({ vault: "Acme" }));
    const before = toml();
    writeFileSync(join(repo, ".eunomia", "skills", "README.md"), "my own notes\n");
    writeFileSync(join(repo, ".eunomia", "eunomia.toml"), before + '\n[ontology]\nmode = "strict"\n');
    const tokens = fake.tokens.length;

    // no credentials needed the second time: the stored token still works
    assert.equal(await init(opts({ email: undefined, password: undefined, vault: undefined })), 0);
    assert.equal(fake.tokens.length, tokens);
    assert.equal(readFileSync(join(repo, ".eunomia", "skills", "README.md"), "utf8"), "my own notes\n");
    assert.match(toml(), /\[ontology\]\nmode = "strict"/);

    // change the vault
    assert.equal(await init(opts({ vault: "Other" })), 0);
    const other = fake.vaults.find((v) => v.name === "Other")!;
    assert.match(toml(), new RegExp(`id = "${other.id}"\nname = "Other"`));
    assert.match(toml(), /\[ontology\]\nmode = "strict"/, "other sections survive a vault change");
    assert.ok(loadCredentials(URL_, other.id, env()));
    assertNoSecretsIn(repo);
  });

  it("replaces its own token on a fresh sign-in instead of piling them up", async () => {
    await init(opts({ vault: "Acme" }));
    await init(opts({ vault: "Acme", agents: "claude" })); // asking for agents forces a sign-in
    assert.equal(fake.tokens.filter((t) => t.name.startsWith("create-eunomia: Acme")).length, 1);
  });

  it("asks for what it needs when interactive", async () => {
    fake.vaults.push({ id: "vault:team", name: "Team", kind: "org", role: "member" });
    const prompt = scripted([
      [/server URL/, URL_],
      [/Sign in with/, "1"],
      [/Email/, "ada@example.com"],
      [/Password/, PASSWORD],
      [/Vault for this project/, "2"],
      [/Connect your AI agents/, "y"],
    ]);
    assert.equal(await init(opts({ url: undefined, email: undefined, password: undefined, agents: undefined, yes: false, prompt })), 0);
    assert.match(toml(), /id = "vault:team"/);
    // connect-agents.sh gets the account credentials (one token per agent), never the project token
    assert.equal(agentRuns.length, 1);
    assert.deepEqual(agentRuns[0].args, ["--url", URL_, "--agents", "auto"]);
    assert.equal(agentRuns[0].env.EUNOMIA_EMAIL, "ada@example.com");
    assert.equal(agentRuns[0].env.EUNOMIA_PASSWORD, PASSWORD);
    assert.equal(agentRuns[0].env.EUNOMIA_TOKEN, "");
  });

  it("signs in with a token, and passes that token to connect-agents.sh", async () => {
    assert.equal(await init(opts({ email: undefined, password: undefined, token: USER_TOKEN, vault: "Acme", agents: "codex" })), 0);
    assert.equal(agentRuns[0].env.EUNOMIA_TOKEN, USER_TOKEN);
    const stored = loadCredentials(URL_, fake.vaults.find((v) => v.name === "Acme")!.id, env())!;
    assert.notEqual(stored.token, USER_TOKEN, "a narrower project token was minted");
  });

  it("uses a vault-restricted token as it is when it cannot mint", async () => {
    fake.tokens.push({ id: "api_token:r", name: "r", token: "eu_restricted", scopes: ["memory:write"], vault_id: "vault:personal" });
    assert.equal(await init(opts({ email: undefined, password: undefined, token: "eu_restricted", vault: "vault:personal" })), 0);
    assert.equal(loadCredentials(URL_, "vault:personal", env())!.token, "eu_restricted");
    assert.ok(lines.some((l) => l.includes("using the token you gave")));
  });

  it("needs --token on a single sign-on server and says where to make one", async () => {
    fake.authMode = "oidc";
    await assert.rejects(init(opts()), /Settings, API tokens.*--token/);
  });

  it("refuses a server that does not answer", async () => {
    await assert.rejects(init(opts({ url: "http://127.0.0.1:9" })), /no Eunomia server answers/);
  });

  it("tells a teammate who is not in the project's vault to ask for an invite", async () => {
    await init(opts({ vault: "Acme" }));
    fake.vaults = fake.vaults.filter((v) => v.name !== "Acme");
    await assert.rejects(init(opts({ env: { HOME: join(home, "teammate") }, vault: undefined })), /ask its owner to invite you/);
  });
});

describe("sync", () => {
  it("creates, leaves unchanged, updates, reports errors without stopping, and never deletes", async () => {
    await init(opts({ vault: "Acme" }));
    const vault = fake.vaults.find((v) => v.name === "Acme")!.id;
    skill("deploy-rollback", "Roll back a bad deploy.");
    skill("db-migrate", "Run a migration.");
    writeFileSync(join(repo, ".eunomia", "skills", "Bad_Name.md"), "---\ndescription: x\n---\nbody\n");
    writeFileSync(join(repo, ".eunomia", "skills", "no-frontmatter.md"), "just text\n");
    writeFileSync(join(repo, ".eunomia", "skills", "fallback-name.md"), "---\ndescription: Named after its file.\n---\nbody\n");
    fake.skills.push({ name: "only-on-server", description: "x", body: "y", vault });

    const run = async () => {
      lines = [];
      const code = await sync({ cwd: repo, env: env(), log: (l) => lines.push(l) });
      return { code, out: lines.join("\n") };
    };
    let r = await run();
    assert.equal(r.code, 1);
    assert.match(r.out, /created\s+db-migrate/);
    assert.match(r.out, /created\s+deploy-rollback/);
    assert.match(r.out, /created\s+fallback-name/);
    assert.match(r.out, /error\s+-\s+\.eunomia\/skills\/Bad_Name\.md: name "Bad_Name" must be/);
    assert.match(r.out, /error\s+-\s+\.eunomia\/skills\/no-frontmatter\.md: must start with/);
    assert.match(r.out, /on the server only \(kept, not deleted\): only-on-server/);
    assert.match(r.out, /3 created, 0 updated, 0 unchanged, 2 error/);
    assert.ok(!r.out.includes("README"));

    writeFileSync(join(repo, ".eunomia", "skills", "Bad_Name.md"), "---\nname: bad-name\ndescription: x\n---\nbody\n");
    writeFileSync(join(repo, ".eunomia", "skills", "no-frontmatter.md"), "---\nname: deploy-rollback\ndescription: dup\n---\nbody\n");
    skill("deploy-rollback", "Roll back a bad deploy.", "New steps.");
    r = await run();
    assert.match(r.out, /created\s+bad-name/);
    assert.match(r.out, /unchanged\s+db-migrate/);
    assert.match(r.out, /updated\s+deploy-rollback/);
    assert.match(r.out, /error\s+deploy-rollback\s+\.eunomia\/skills\/no-frontmatter\.md: name deploy-rollback is already used by deploy-rollback\.md/);
    assert.equal(fake.skills.find((s) => s.name === "deploy-rollback")!.body, "New steps.");
    assert.ok(fake.skills.some((s) => s.name === "only-on-server"), "never deleted");
    // unchanged skills are not saved again
    const saves = fake.calls.filter((c) => c.path === "/api/tools/skill_save" && c.body.name === "db-migrate");
    assert.equal(saves.length, 1);
  });

  it("reports a server-side rejection per skill", async () => {
    await init(opts({ vault: "Acme" }));
    skill("server-rejects", "x");
    skill("fine", "y");
    lines = [];
    assert.equal(await sync({ cwd: repo, env: env(), log: (l) => lines.push(l) }), 1);
    assert.match(lines.join("\n"), /error\s+server-rejects .*description must be/);
    assert.match(lines.join("\n"), /created\s+fine/);
  });

  it("init runs the first sync", async () => {
    mkdirSync(join(repo, ".eunomia", "skills"), { recursive: true });
    skill("first", "On first init.");
    await init(opts({ vault: "Acme" }));
    assert.ok(lines.some((l) => /created\s+first/.test(l)));
  });

  it("works from the command line with EUNOMIA_TOKEN and without a stored token", async () => {
    await init(opts({ vault: "Acme" }));
    skill("cli-skill", "From the CLI.");
    const cli = join(PKG, "dist", "cli.js");
    const run = promisify(execFile);
    const ok = await run(process.execPath, [cli, "sync"], { cwd: repo, env: env() });
    assert.match(ok.stdout, /created\s+cli-skill/);
    await assert.rejects(run(process.execPath, [cli, "sync"], { cwd: repo, env: { HOME: join(home, "nobody") } }), /no token for/);
    const viaEnv = await run(process.execPath, [cli, "sync"], { cwd: repo, env: { HOME: join(home, "nobody"), EUNOMIA_TOKEN: fake.tokens.at(-1)!.token } });
    assert.match(viaEnv.stdout, /unchanged\s+cli-skill/);
  });
});

describe("package", () => {
  it("ships an exact copy of scripts/connect-agents.sh where the built CLI looks for it", () => {
    assert.equal(readFileSync(join(PKG, "connect-agents.sh"), "utf8"), readFileSync(join(PKG, "..", "..", "scripts", "connect-agents.sh"), "utf8"));
    // dist/init.js resolves it one folder up, as connectAgentsScript() does from any build folder
    assert.equal(connectAgentsScript(), join(PKG, "dist-test", "connect-agents.sh"));
    assert.match(readFileSync(join(PKG, "dist", "init.js"), "utf8"), /new URL\("\.\.\/connect-agents\.sh", import\.meta\.url\)/);
    const pkg = JSON.parse(readFileSync(join(PKG, "package.json"), "utf8"));
    assert.ok(pkg.files.includes("connect-agents.sh") && pkg.files.includes("dist"));
    assert.equal(pkg.bin["create-eunomia"], "dist/cli.js");
  });
});

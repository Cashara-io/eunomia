import assert from "node:assert/strict";
import { execFile, spawn } from "node:child_process";
import { chmodSync, copyFileSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, statSync, symlinkSync, writeFileSync } from "node:fs";
import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { after, before, beforeEach, describe, it } from "node:test";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import { connectAgentsScript, init, normalizeUrl, sync, type InitOptions } from "../src/init.js";
import { credentialsPath, isLocalUrl, loadCredentials, stringifyToml } from "../src/project.js";
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
  failMint: false,
  /** Tokens the server refuses for vault reads (lost access), though they still exist. */
  blind: new Set<string>(),
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
  fake.failMint = false;
  fake.blind = new Set();
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
  if (path === "/api/auth/me") return [200, { id: "user:1", email: "ada@example.com" }];
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
    if (fake.failMint) return [500, { code: "internal", detail: "boom" }];
    const t: Tok = { id: `api_token:${++fake.n}`, name: body.name, token: `eu_project_${fake.n}_${Math.random().toString(36).slice(2)}`, scopes: body.scopes, vault_id: body.vault_id ?? null };
    fake.tokens.push(t);
    return [200, t];
  }
  if (path === "/api/vaults") {
    if (method === "GET") {
      if (bearer && fake.blind.has(bearer)) return [200, { results: [] }];
      return [200, { results: fake.vaults.filter((v) => !restricted || v.id === restricted) }];
    }
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

/** Another server that a hostile eunomia.toml points at: it answers health checks and records everything. */
const seen: { path: string; auth?: string; cookie?: string; body: string }[] = [];
const attacker: Server = createServer(async (req, res) => {
  let raw = "";
  for await (const c of req) raw += c;
  seen.push({ path: req.url!, auth: req.headers.authorization, cookie: req.headers.cookie, body: raw });
  res.writeHead(200, { "content-type": "application/json" });
  res.end(req.url === "/healthz" ? '{"status":"ok"}' : '{"auth_mode":"password"}');
});

let URL_ = "";
let EVIL = "";
const listen = async (s: Server) => {
  await new Promise<void>((r) => s.listen(0, "127.0.0.1", r));
  return `http://127.0.0.1:${(s.address() as { port: number }).port}`;
};
before(async () => {
  URL_ = await listen(server);
  EVIL = await listen(attacker);
});
after(() => {
  server.close();
  attacker.close();
});

// ---------------------------------------------------------------------------

let repo = "";
let home = "";
let lines: string[] = [];
let agentRuns: { args: string[]; env: NodeJS.ProcessEnv }[] = [];
let agentStatus: number | null = 0;
beforeEach(() => {
  reset();
  seen.length = 0;
  const tmp = mkdtempSync(join(tmpdir(), "create-eunomia-"));
  repo = join(tmp, "my-repo");
  home = join(tmp, "home");
  mkdirSync(repo);
  mkdirSync(home);
  lines = [];
  agentRuns = [];
  agentStatus = 0;
});

const env = (extra: NodeJS.ProcessEnv = {}) => ({ HOME: home, PATH: process.env.PATH, ...extra });
function opts(extra: Partial<InitOptions> = {}): InitOptions {
  return {
    cwd: repo,
    url: URL_,
    email: "ada@example.com",
    password: PASSWORD,
    yes: true,
    createVault: true,
    env: env(),
    prompt: defaultsPrompter(),
    log: (l) => lines.push(l),
    connectAgents: (args, e) => (agentRuns.push({ args, env: e }), agentStatus),
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
const vaultId = (name: string) => fake.vaults.find((v) => v.name === name)!.id;
const projectTokens = () => fake.tokens.filter((t) => t.name.startsWith("create-eunomia:"));

describe("init", () => {
  it("signs in, creates the vault, mints a narrow token outside the repo and writes .eunomia/", async () => {
    assert.equal(await init(opts({ vault: "Acme" })), 0);
    const minted = projectTokens();
    assert.equal(minted.length, 1);
    assert.deepEqual(minted[0].scopes, ["memory:write"]);
    assert.equal(minted[0].vault_id, vaultId("Acme"));
    assert.match(minted[0].name, /^create-eunomia: Acme \(/);

    assert.match(toml(), /\[server\]\nurl = "http:\/\/127\.0\.0\.1:\d+"\n\n\[vault\]\nid = "vault:\d+"\nname = "Acme"\n/);
    assert.match(toml(), /^# .*\n# Docs: https:\/\/github.com\/Cashara-io\/eunomia\/blob\/main\/docs\/projects.md/);
    assert.ok(statSync(join(repo, ".eunomia", "skills", "README.md")).isFile());
    assert.ok(statSync(join(repo, ".eunomia", ".gitignore")).isFile());

    const p = credentialsPath(URL_, vaultId("Acme"), env());
    assert.ok(p.startsWith(join(home, ".config", "eunomia", "projects")));
    assert.equal(statSync(p).mode & 0o777, 0o600);
    const stored = loadCredentials(URL_, vaultId("Acme"), env())!;
    assert.equal(stored.token, minted[0].token);
    assert.equal(stored.tokenId, minted[0].id);
    assertNoSecretsIn(repo);
    assert.ok(fake.calls.some((c) => c.path === "/api/auth/logout"));
  });

  it("tightens an existing config folder to 0700", async () => {
    mkdirSync(join(home, ".config", "eunomia", "projects"), { recursive: true });
    chmodSync(join(home, ".config", "eunomia"), 0o755);
    chmodSync(join(home, ".config", "eunomia", "projects"), 0o755);
    await init(opts({ vault: "Acme" }));
    assert.equal(statSync(join(home, ".config", "eunomia")).mode & 0o777, 0o700);
    assert.equal(statSync(join(home, ".config", "eunomia", "projects")).mode & 0o777, 0o700);
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
    assert.match(toml(), new RegExp(`id = "${vaultId("Other")}"\nname = "Other"`));
    assert.match(toml(), /\[ontology\]\nmode = "strict"/, "other sections survive a vault change");
    assert.ok(loadCredentials(URL_, vaultId("Other"), env()));
    assertNoSecretsIn(repo);
  });

  it("asks for what it needs when interactive, and does not offer the personal vault", async () => {
    fake.vaults.push({ id: "vault:team", name: "Team", kind: "org", role: "member" });
    const prompt = scripted([
      [/server URL/, URL_],
      [/Sign in with/, "1"],
      [/Email/, "ada@example.com"],
      [/Password/, PASSWORD],
      [/Vault for this project/, "1"],
      [/Connect the AI agents installed on this machine\? Each gets its own account-wide token/, "y"],
    ]);
    assert.equal(await init(opts({ url: undefined, email: undefined, password: undefined, yes: false, prompt })), 0);
    assert.match(toml(), /id = "vault:team"/);
    assert.ok(lines.includes("  1) Team (org, member)"));
    assert.ok(!lines.some((l) => l.includes("Personal")));
    // only agents that look installed, with the account credentials, never a token
    assert.equal(agentRuns.length, 1);
    assert.deepEqual(agentRuns[0].args, ["--url", URL_, "--agents", "all", "--detected-only"]);
    assert.equal(agentRuns[0].env.EUNOMIA_EMAIL, "ada@example.com");
    assert.equal(agentRuns[0].env.EUNOMIA_PASSWORD, PASSWORD);
    assert.equal(agentRuns[0].env.EUNOMIA_TOKEN, "");
  });

  it("revokes the login session even when a later step fails", async () => {
    await assert.rejects(init(opts({ vault: "Typo", createVault: false })), /no vault named Typo/);
    assert.ok(fake.calls.some((c) => c.path === "/api/auth/login"));
    assert.ok(fake.calls.some((c) => c.path === "/api/auth/logout"));
  });

  it("needs --token on a single sign-on server and says where to make one", async () => {
    fake.authMode = "oidc";
    await assert.rejects(init(opts()), /Settings, API tokens.*--token/);
  });

  it("checks a --token before saying signed in", async () => {
    await assert.rejects(init(opts({ email: undefined, password: undefined, token: "eu_wrong", vault: "Acme" })), /rejected the token/);
    assert.ok(!lines.some((l) => l.includes("signed in")));
  });

  it("uses a vault-restricted token as it is when it cannot mint", async () => {
    fake.tokens.push({ id: "api_token:r", name: "r", token: "eu_restricted", scopes: ["memory:write"], vault_id: "vault:personal" });
    assert.equal(await init(opts({ email: undefined, password: undefined, token: "eu_restricted", vault: "vault:personal" })), 0);
    assert.equal(loadCredentials(URL_, "vault:personal", env())!.token, "eu_restricted");
    assert.ok(lines.some((l) => l.includes("using the token you gave")));
    assert.ok(lines.some((l) => l.includes("personal vault: teammates cannot be invited")));
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

describe("server URL and credentials (a committed file cannot steer them)", () => {
  const evilRepo = () => {
    mkdirSync(join(repo, ".eunomia"), { recursive: true });
    writeFileSync(join(repo, ".eunomia", "eunomia.toml"), stringifyToml({ server: { url: EVIL }, vault: { id: "vault:x", name: "X" } }));
  };
  const noCredentialsReached = () => {
    for (const r of seen) {
      assert.equal(r.auth, undefined, `Authorization sent to ${r.path}`);
      assert.equal(r.cookie, undefined);
      assert.ok(!r.body.includes(PASSWORD) && !r.body.includes(USER_TOKEN), `secret sent to ${r.path}`);
      assert.ok(!r.path.startsWith("/api/auth/login"));
    }
  };

  it("sync does not send EUNOMIA_TOKEN to the file's server unless EUNOMIA_URL names it", async () => {
    evilRepo();
    await assert.rejects(sync({ cwd: repo, env: env({ EUNOMIA_TOKEN: USER_TOKEN }), log: () => {} }), /also set EUNOMIA_URL=/);
    await assert.rejects(sync({ cwd: repo, env: env({ EUNOMIA_TOKEN: USER_TOKEN, EUNOMIA_URL: URL_ }), log: () => {} }), /says the server is .*but you named/);
    await assert.rejects(sync({ cwd: repo, url: URL_, env: env({ EUNOMIA_TOKEN: USER_TOKEN }), log: () => {} }), /but you named/);
    assert.equal(seen.length, 0);
  });

  it("init with --yes will not sign in to the file's server without --url", async () => {
    evilRepo();
    await assert.rejects(
      init(opts({ url: undefined, email: undefined, password: undefined, env: env({ EUNOMIA_EMAIL: "ada@example.com", EUNOMIA_PASSWORD: PASSWORD, EUNOMIA_TOKEN: USER_TOKEN }) })),
      /needs the server named explicitly: run again with --url/,
    );
    noCredentialsReached();
  });

  it("init asks before signing in to the file's server, and stops on no", async () => {
    evilRepo();
    const prompt = scripted([
      [/Keep the vault X\?/, "y"],
      [/Sign in to http:\/\/127\.0\.0\.1:\d+, the server named in this repo's/, "n"],
    ]);
    await assert.rejects(init(opts({ url: undefined, yes: false, prompt })), /stopped: nothing was sent/);
    noCredentialsReached();
  });

  it("init stops when the named server differs from the file's", async () => {
    evilRepo();
    await assert.rejects(init(opts({ url: URL_ })), /says the server is .*but you named .*Nothing was sent/);
    await assert.rejects(init(opts({ url: undefined, env: env({ EUNOMIA_URL: URL_ }) })), /but you named/);
    assert.equal(seen.length, 0);
  });

  it("a stored token goes only to the exact URL it was stored for", async () => {
    await init(opts({ vault: "Acme" }));
    // same server spelled differently: the stored token is not used, so a sign-in is needed
    const other = URL_.replace("127.0.0.1", "localhost");
    await assert.rejects(sync({ cwd: repo, url: other, env: env(), log: () => {} }), /but you named/);
    assert.equal(loadCredentials(other, vaultId("Acme"), env()), undefined);
  });

  it("refuses a URL with a user name or password, without echoing it", () => {
    assert.throws(() => normalizeUrl("https://bob:hunter2@eunomia.example.com"), (e: Error) => /user name or password/.test(e.message) && !e.message.includes("hunter2"));
    assert.throws(() => normalizeUrl("bob:hunter2@host"), (e: Error) => !e.message.includes("hunter2"));
    assert.equal(normalizeUrl(" http://localhost:8001/mcp/ "), "http://localhost:8001");
  });

  it("warns about plain http to another machine", async () => {
    assert.ok(isLocalUrl("http://localhost:8001") && isLocalUrl("http://127.0.0.1:1") && isLocalUrl("http://[::1]:8001") && isLocalUrl("http://app.localhost"));
    assert.ok(!isLocalUrl("http://eunomia.example.com"));
    await assert.rejects(init(opts({ url: "http://eunomia.invalid" })));
    assert.ok(lines.some((l) => l.includes("is plain http")));
  });
});

describe("agents", () => {
  it("--yes connects none and prints how", async () => {
    await init(opts({ vault: "Acme" }));
    assert.equal(agentRuns.length, 0);
    assert.ok(lines.some((l) => /none connected \(--yes connects none\)\. To connect some: npm create eunomia -- --url/.test(l)));
  });

  it("an explicit list runs connect-agents.sh with it, and a failure makes init fail", async () => {
    agentStatus = 1;
    assert.equal(await init(opts({ vault: "Acme", agents: "bogus" })), 1);
    assert.deepEqual(agentRuns[0].args, ["--url", URL_, "--agents", "bogus"]);
    assert.ok(lines.some((l) => l.includes("connect-agents.sh failed (exit 1)")));
    assert.ok(lines.some((l) => l.includes("but connecting agents failed")));
  });

  it("without bash prints the manual steps", async () => {
    agentStatus = null;
    assert.equal(await init(opts({ vault: "Acme", agents: "claude" })), 0);
    assert.ok(lines.some((l) => l.includes("Connect agents by hand")));
  });

  it("a user's --token is never handed to connect-agents.sh", async () => {
    assert.equal(await init(opts({ email: undefined, password: undefined, token: USER_TOKEN, vault: "Acme", agents: "codex" })), 0);
    assert.equal(agentRuns.length, 0);
    assert.ok(lines.some((l) => l.includes("signed in with a token")));
    assert.notEqual(loadCredentials(URL_, vaultId("Acme"), env())!.token, USER_TOKEN, "a narrower project token was minted");
  });

  it("the real script fails init on a bad agent name", async () => {
    // the compiled tests look for the script next to dist-test/, as the package does next to dist/
    copyFileSync(join(PKG, "connect-agents.sh"), connectAgentsScript());
    const o = opts({ vault: "Acme", agents: "bogus" });
    delete o.connectAgents;
    assert.equal(await init(o), 1);
  });
});

describe("vaults", () => {
  it("--yes refuses an unknown vault name unless --create-vault", async () => {
    await assert.rejects(init(opts({ vault: "Acm", createVault: false })), /no vault named Acm.*--create-vault/);
    assert.ok(!fake.vaults.some((v) => v.name === "Acm"));
    await assert.rejects(init(opts({ createVault: false })), /pass --vault/);
  });

  it("interactive asks before creating a typed name", async () => {
    const prompt = scripted([
      [/Vault for this project/, "Typo"],
      [/no vault named Typo\. Create it\?/, "n"],
    ]);
    await assert.rejects(init(opts({ yes: false, createVault: false, agents: "none", prompt })), /stopped: no vault chosen/);
    assert.ok(!fake.vaults.some((v) => v.name === "Typo"));
  });

  it("two vaults with the same name: asks for the id", async () => {
    fake.vaults.push({ id: "vault:a", name: "Dup", kind: "org", role: "owner" }, { id: "vault:b", name: "Dup", kind: "org", role: "member" });
    await assert.rejects(init(opts({ vault: "Dup" })), /2 vaults are named Dup: pass --vault with one of their ids \(vault:a, vault:b\)/);
    assert.equal(await init(opts({ vault: "vault:b" })), 0);
    assert.match(toml(), /id = "vault:b"/);
  });
});

describe("project tokens", () => {
  it("another machine's init does not revoke this machine's token", async () => {
    await init(opts({ vault: "Acme" }));
    const first = loadCredentials(URL_, vaultId("Acme"), env())!;
    const otherHome = join(home, "devcontainer");
    await init(opts({ vault: "Acme", env: { HOME: otherHome } }));
    assert.ok(fake.tokens.some((t) => t.id === first.tokenId), "first machine's token survives");
    assert.equal(projectTokens().length, 2);
  });

  it("mints the new token before revoking the old one, and revokes only that id", async () => {
    await init(opts({ vault: "Acme" }));
    const old = loadCredentials(URL_, vaultId("Acme"), env())!;
    fake.blind.add(old.token); // the stored token no longer reaches the vault
    fake.failMint = true;
    await assert.rejects(init(opts({ vault: "Acme" })));
    assert.ok(fake.tokens.some((t) => t.id === old.tokenId), "a failed mint leaves the old token alone");
    assert.equal(loadCredentials(URL_, vaultId("Acme"), env())!.token, old.token);

    fake.failMint = false;
    fake.tokens.push({ id: "api_token:elsewhere", name: projectTokens()[0].name, token: "eu_elsewhere", scopes: ["memory:write"], vault_id: vaultId("Acme") });
    assert.equal(await init(opts({ vault: "Acme" })), 0);
    assert.ok(!fake.tokens.some((t) => t.id === old.tokenId), "old token revoked");
    assert.ok(fake.tokens.some((t) => t.id === "api_token:elsewhere"), "a same-named token from elsewhere is kept");
    assert.notEqual(loadCredentials(URL_, vaultId("Acme"), env())!.token, old.token);
  });
});

describe("sync", () => {
  it("creates, leaves unchanged, updates, reports errors without stopping, and never deletes", async () => {
    await init(opts({ vault: "Acme" }));
    const vault = vaultId("Acme");
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
    const saves = fake.calls.filter((c) => c.path === "/api/tools/skill_save" && c.body.name === "db-migrate");
    assert.equal(saves.length, 1, "unchanged skills are not saved again");
  });

  it("reads <name>/SKILL.md folders and skips symlinks, reporting them", async () => {
    await init(opts({ vault: "Acme" }));
    const dir = join(repo, ".eunomia", "skills");
    mkdirSync(join(dir, "release-notes"));
    writeFileSync(join(dir, "release-notes", "SKILL.md"), "---\ndescription: Write release notes.\n---\nSteps.\n");
    const outside = join(home, "secret.md");
    writeFileSync(outside, "---\nname: leaked\ndescription: outside the repo\n---\nprivate\n");
    symlinkSync(outside, join(dir, "linked.md"));
    mkdirSync(join(dir, "linked-dir"));
    symlinkSync(outside, join(dir, "linked-dir", "SKILL.md"));
    mkdirSync(join(dir, "empty-folder"));
    lines = [];
    assert.equal(await sync({ cwd: repo, env: env(), log: (l) => lines.push(l) }), 1);
    const out = lines.join("\n");
    assert.match(out, /created\s+release-notes\s+\.eunomia\/skills\/release-notes\/SKILL\.md/);
    assert.match(out, /error\s+-\s+\.eunomia\/skills\/linked\.md: is a symlink: skipped/);
    assert.match(out, /error\s+-\s+\.eunomia\/skills\/linked-dir\/SKILL\.md: is not a regular file/);
    assert.ok(!fake.skills.some((s) => s.name === "leaked" || s.body.includes("private")));
  });

  it("a revoked token stops the sync with one clear message", async () => {
    await init(opts({ vault: "Acme" }));
    skill("a", "x");
    skill("b", "y");
    fake.tokens = fake.tokens.filter((t) => !t.name.startsWith("create-eunomia:"));
    await assert.rejects(sync({ cwd: repo, env: env(), log: (l) => lines.push(l) }), /rejected the token: run `npx create-eunomia` again/);
    assert.ok(!lines.some((l) => l.includes("Not authenticated")));
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

  it("works from the command line with a stored token, or EUNOMIA_TOKEN plus EUNOMIA_URL", async () => {
    await init(opts({ vault: "Acme" }));
    skill("cli-skill", "From the CLI.");
    const cli = join(PKG, "dist", "cli.js");
    const run = promisify(execFile);
    const ok = await run(process.execPath, [cli, "sync"], { cwd: repo, env: env() });
    assert.match(ok.stdout, /created\s+cli-skill/);
    const nobody = { HOME: join(home, "nobody") };
    await assert.rejects(run(process.execPath, [cli, "sync"], { cwd: repo, env: nobody }), /no token for/);
    const token = projectTokens()[0].token;
    await assert.rejects(run(process.execPath, [cli, "sync"], { cwd: repo, env: { ...nobody, EUNOMIA_TOKEN: token } }), /also set EUNOMIA_URL/);
    const viaEnv = await run(process.execPath, [cli, "sync"], { cwd: repo, env: { ...nobody, EUNOMIA_TOKEN: token, EUNOMIA_URL: URL_ } });
    assert.match(viaEnv.stdout, /unchanged\s+cli-skill/);
  });

  it("interactive init with stdin at EOF exits non-zero with a message", async () => {
    const child = spawn(process.execPath, [join(PKG, "dist", "cli.js")], { cwd: repo, env: env() });
    child.stdin.end();
    let err = "";
    child.stderr.on("data", (d) => (err += d));
    const code = await new Promise<number | null>((r) => child.on("close", r));
    assert.equal(code, 1);
    assert.match(err, /input ended before setup finished/);
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

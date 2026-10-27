/** `create-eunomia` (init) and `create-eunomia sync`. */

import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { hostname } from "node:os";
import { basename, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { Api, ApiError, type Vault } from "./api.js";
import {
  credentialsPath,
  ensureSkeleton,
  loadCredentials,
  readConfig,
  saveCredentials,
  skillsDir,
  writeConfig,
  type Credentials,
} from "./project.js";
import type { Prompter } from "./prompt.js";
import { formatSync, syncSkills, type SyncResult } from "./skills.js";

export const DEFAULT_URL = "http://localhost:8001";

export interface InitOptions {
  cwd: string;
  url?: string;
  email?: string;
  password?: string;
  token?: string;
  vault?: string;
  /** connect-agents.sh `--agents` value; `none` skips. Asked when unset and interactive. */
  agents?: string;
  yes: boolean;
  env: NodeJS.ProcessEnv;
  prompt: Prompter;
  log: (line: string) => void;
  /** Runs connect-agents.sh; returns false when it could not run (no bash). Replaceable in tests. */
  connectAgents?: (args: string[], env: NodeJS.ProcessEnv) => boolean;
}

export function normalizeUrl(u: string): string {
  const url = u.trim().replace(/\/+$/, "").replace(/\/(mcp|api)$/, "");
  if (!/^https?:\/\/[^\s"'\\]+$/.test(url)) throw new Error(`server URL must start with http:// or https:// (got ${JSON.stringify(u)})`);
  return url;
}

/** The token sync uses: EUNOMIA_TOKEN (for CI), else the one init stored for this server and vault. */
function syncToken(url: string, vaultId: string, env: NodeJS.ProcessEnv): string | undefined {
  return env.EUNOMIA_TOKEN || loadCredentials(url, vaultId, env)?.token;
}

/** Whether `token` still reaches `vaultId`. */
async function tokenWorks(url: string, token: string, vaultId: string): Promise<boolean> {
  try {
    return (await new Api(url, token).listVaults()).some((v) => v.id === vaultId);
  } catch {
    return false;
  }
}

async function signIn(api: Api, o: InitOptions): Promise<{ email?: string; password?: string; token?: string }> {
  let token = o.token ?? o.env.EUNOMIA_TOKEN;
  let email = o.email ?? o.env.EUNOMIA_EMAIL;
  let password = o.password ?? o.env.EUNOMIA_PASSWORD;
  if (!token) {
    const mode = await api.authMode().catch(() => "password");
    if (mode === "oidc") {
      throw new Error(
        "this server signs in through single sign-on only, so password sign-in is off. In the Eunomia app open Settings, API tokens, create a token, and run again with --token <token> (or EUNOMIA_TOKEN).",
      );
    }
    if (!email && !o.yes) {
      const how = await o.prompt.ask("Sign in with (1) email and password or (2) an API token", "1");
      if (how === "2") token = await o.prompt.secret("API token (Settings, API tokens)");
    }
  }
  if (token) {
    return { token };
  }
  email ||= await o.prompt.ask("Email");
  password ||= await o.prompt.secret("Password");
  if (!email || !password) throw new Error("need --email and --password, or --token");
  await api.login(email, password).catch((e) => {
    throw e instanceof ApiError && e.status === 401 ? new Error(`could not sign in as ${email}: wrong email or password`) : e;
  });
  return { email, password };
}

async function pickVault(api: Api, o: InitOptions, current?: string): Promise<Vault> {
  const vaults = await api.listVaults();
  const want = o.vault ?? current;
  if (want) {
    const hit = vaults.find((v) => v.id === want) ?? vaults.find((v) => v.name === want);
    if (hit) return hit;
    if (current && !o.vault) {
      throw new Error(`this project's vault (${current}) is not one of yours: ask its owner to invite you (Vaults, Members), then run again`);
    }
    o.log(`  creating vault ${want}`);
    return api.createVault(want);
  }
  const fallbackName = basename(o.cwd);
  if (o.yes) {
    return vaults.find((v) => v.name === fallbackName) ?? api.createVault(fallbackName);
  }
  o.log("Vaults:");
  vaults.forEach((v, i) => o.log(`  ${i + 1}) ${v.name} (${v.kind}${v.role ? `, ${v.role}` : ""})`));
  o.log(`  n) a new vault`);
  const a = await o.prompt.ask("Vault for this project", "n");
  const n = Number(a);
  if (Number.isInteger(n) && n >= 1 && n <= vaults.length) return vaults[n - 1];
  const name = a === "n" ? await o.prompt.ask("New vault name", fallbackName) : a;
  return vaults.find((v) => v.name === name) ?? api.createVault(name);
}

/** A `memory:write` token for this vault only, named so a re-run replaces it rather than piling up. */
async function projectToken(api: Api, vault: Vault, userToken: string | undefined, o: InitOptions): Promise<{ token: string; tokenId?: string }> {
  const name = `create-eunomia: ${vault.name} (${hostname().split(".")[0] || "host"})`.slice(0, 80);
  try {
    for (const t of await api.listTokens()) {
      if (t.name === name && t.vault_id === vault.id) await api.deleteToken(t.id);
    }
    const t = await api.mintProjectToken(name, vault.id);
    return { token: t.token, tokenId: t.id };
  } catch (e) {
    // a vault-restricted or expiring token cannot mint a narrower never-expiring one: use it as it is
    if (userToken && e instanceof ApiError && e.status === 403) {
      o.log(`  ! could not mint a project token (${e.message}); using the token you gave`);
      return { token: userToken };
    }
    throw e;
  }
}

export function connectAgentsScript(): string {
  return fileURLToPath(new URL("../connect-agents.sh", import.meta.url));
}

function runScript(args: string[], env: NodeJS.ProcessEnv): boolean {
  if (process.platform === "win32") return false;
  const r = spawnSync("bash", [connectAgentsScript(), ...args], { stdio: "inherit", env });
  return !r.error;
}

function manualAgentSteps(url: string, log: (l: string) => void): void {
  log("  Connect agents by hand (bash was not available):");
  log("    1. In the Eunomia app open Settings, API tokens, and create a token for each agent.");
  log(`    2. Add an HTTP MCP server named eunomia at ${url}/mcp with the header Authorization: Bearer <token>.`);
  log(`       Claude Code: claude mcp add --transport http eunomia ${url}/mcp --header "Authorization: Bearer <token>"`);
  log("    See docs/agents.md for the other agents.");
}

export async function init(o: InitOptions): Promise<number> {
  const existing = readConfig(o.cwd);
  if (existing) o.log(`Found .eunomia/eunomia.toml: ${existing.vaultName} on ${existing.url}`);

  // 1. server
  let url = o.url ?? o.env.EUNOMIA_URL ?? existing?.url;
  url = normalizeUrl(url ?? (await o.prompt.ask("Eunomia server URL", DEFAULT_URL)));
  const api = new Api(url);
  if (!(await api.healthy())) throw new Error(`no Eunomia server answers at ${url}/healthz: is it running?`);
  o.log(`  ✓ ${url} is up`);

  // a re-run keeps the vault unless asked to change it
  let changeVault = Boolean(o.vault && existing && o.vault !== existing.vaultId && o.vault !== existing.vaultName);
  if (existing && !o.vault && !o.yes) {
    changeVault = !(await o.prompt.confirm(`Keep the vault ${existing.vaultName}?`, true));
  }
  const sameServer = existing && existing.url === url;
  const stored = sameServer && !changeVault ? loadCredentials(url, existing.vaultId, o.env) : undefined;
  const reuse = stored && (await tokenWorks(url, stored.token, existing!.vaultId)) && !(o.agents && o.agents !== "none");

  let vault: Vault;
  let creds: Credentials;
  let user: { email?: string; password?: string; token?: string } | undefined;
  if (reuse) {
    vault = { id: existing!.vaultId, name: existing!.vaultName, kind: "" };
    creds = stored!;
    o.log(`  ✓ signed in already (token in ${relativeHome(o.env, credentialsPath(url, vault.id, o.env))})`);
  } else {
    // 2. sign in
    user = await signIn(api, o);
    if (user.token) api.useToken(user.token);
    o.log("  ✓ signed in");
    // 4. vault
    vault = await pickVault(api, o, changeVault ? undefined : sameServer ? existing?.vaultId : undefined);
    o.log(`  ✓ vault ${vault.name} (${vault.id})`);
    // 2b, 3. project token, stored per user
    const t = await projectToken(api, vault, user.token, o);
    creds = { url, vaultId: vault.id, ...t };
    const path = saveCredentials(creds, o.env);
    o.log(`  ✓ project token saved to ${relativeHome(o.env, path)} (mode 600, outside the repo)`);
  }

  // 5. .eunomia/
  if (!existing || changeVault || existing.url !== url || existing.vaultId !== vault.id || existing.vaultName !== vault.name) {
    writeConfig(o.cwd, { url, vaultId: vault.id, vaultName: vault.name }, existing?.raw);
    o.log("  ✓ wrote .eunomia/eunomia.toml");
  }
  for (const f of ensureSkeleton(o.cwd)) o.log(`  ✓ wrote ${relative(o.cwd, f)}`);

  // 6. agents: connect-agents.sh mints one token per agent from the account, so it gets the
  // account credentials, never the vault-only project token
  if (user) {
    let agents = o.agents;
    if (!agents) agents = o.yes || (await o.prompt.confirm("Connect your AI agents (Claude Code, Codex, Cursor, ...) to Eunomia?", true)) ? "auto" : "none";
    if (agents !== "none") {
      const env = { ...o.env, EUNOMIA_TOKEN: user.token ?? "", EUNOMIA_EMAIL: user.email ?? "", EUNOMIA_PASSWORD: user.password ?? "" };
      o.log("Connecting agents (connect-agents.sh):");
      const ran = (o.connectAgents ?? runScript)(["--url", url, "--agents", agents], env);
      if (!ran) manualAgentSteps(url, o.log);
    }
  } else if (o.agents === undefined) {
    o.log(`  agents: unchanged (run again with --agents auto and sign in to connect them)`);
  }
  await api.logout();

  // first sync
  o.log("Syncing .eunomia/skills:");
  const result = await syncSkills(new Api(url, creds.token), skillsDir(o.cwd), vault.id);
  o.log(formatSync(result, o.cwd));

  // 7. summary
  o.log("");
  o.log(`Eunomia project ready: vault ${vault.name} on ${url}.`);
  o.log("Commit the folder (it holds no secrets):");
  o.log('  git add .eunomia && git commit -m "Add Eunomia project"');
  o.log("Teammates run `npm create eunomia` in their clone and sign in with their own account (share the vault with them first, under Vaults, Members).");
  o.log("Sync skills after editing them: npx create-eunomia sync");
  return failed(result) ? 1 : 0;
}

const failed = (r: SyncResult) => r.results.some((x) => x.outcome === "error");

function relativeHome(env: NodeJS.ProcessEnv, p: string): string {
  const home = env.HOME;
  return home && p.startsWith(home + "/") ? "~/" + p.slice(home.length + 1) : p;
}

export async function sync(o: { cwd: string; env: NodeJS.ProcessEnv; log: (l: string) => void }): Promise<number> {
  const cfg = readConfig(o.cwd);
  if (!cfg) throw new Error("no .eunomia/eunomia.toml here: run `npm create eunomia` first");
  const token = syncToken(cfg.url, cfg.vaultId, o.env);
  if (!token) throw new Error(`no token for ${cfg.url} on this machine: run \`npx create-eunomia\` to sign in (or set EUNOMIA_TOKEN)`);
  if (!existsSync(skillsDir(o.cwd))) o.log("  no .eunomia/skills folder yet");
  o.log(`Syncing .eunomia/skills to ${cfg.vaultName} on ${cfg.url}:`);
  const result = await syncSkills(new Api(cfg.url, token), skillsDir(o.cwd), cfg.vaultId);
  o.log(formatSync(result, o.cwd));
  const counts = ["created", "updated", "unchanged", "error"].map((k) => `${result.results.filter((r) => r.outcome === k).length} ${k}`);
  o.log(`  ${counts.join(", ")}`);
  return failed(result) ? 1 : 0;
}

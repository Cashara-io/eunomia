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
  isLocalUrl,
  loadCredentials,
  normalizeUrl,
  readConfig,
  saveCredentials,
  skillsDir,
  writeConfig,
  type Credentials,
  type ProjectConfig,
} from "./project.js";
import type { Prompter } from "./prompt.js";
import { formatSync, syncSkills, TokenRejected, type SyncResult } from "./skills.js";

export { normalizeUrl };
export const DEFAULT_URL = "http://localhost:8001";

export interface InitOptions {
  cwd: string;
  url?: string;
  email?: string;
  password?: string;
  token?: string;
  vault?: string;
  /** Allow `--vault <name>` (or the folder name with `--yes`) to create a vault that does not exist. */
  createVault?: boolean;
  /** connect-agents.sh `--agents` value; `none` skips. Unset: asked when interactive, nothing with `--yes`. */
  agents?: string;
  yes: boolean;
  env: NodeJS.ProcessEnv;
  prompt: Prompter;
  log: (line: string) => void;
  /** Runs connect-agents.sh: its exit status, or null when it could not run (no bash). Replaceable in tests. */
  connectAgents?: (args: string[], env: NodeJS.ProcessEnv) => number | null;
}

type User = { email?: string; password?: string; token?: string };

/** Whether `token` still reaches `vaultId`. */
async function tokenWorks(url: string, token: string, vaultId: string): Promise<boolean> {
  try {
    return (await new Api(url, token).listVaults()).some((v) => v.id === vaultId);
  } catch {
    return false;
  }
}

const rejected = (url: string) => `${url} rejected the token: check it, or make a new one in Settings, API tokens`;

async function signIn(api: Api, o: InitOptions): Promise<User> {
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
    api.useToken(token);
    await api.me().catch((e) => {
      throw e instanceof ApiError && e.status === 401 ? new Error(rejected(api.url)) : e;
    });
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
  if (current && !o.vault) {
    const hit = vaults.find((v) => v.id === current);
    if (hit) return hit;
    throw new Error(`this project's vault (${current}) is not one of yours: ask its owner to invite you (Vaults, Members), then run again`);
  }
  /** An id, or a name that only one vault has. */
  const find = (ref: string): Vault | undefined => {
    const byId = vaults.find((v) => v.id === ref);
    if (byId) return byId;
    const named = vaults.filter((v) => v.name === ref);
    if (named.length > 1) throw new Error(`${named.length} vaults are named ${ref}: pass --vault with one of their ids (${named.map((v) => v.id).join(", ")})`);
    return named[0];
  };
  const create = async (name: string, chosen = false): Promise<Vault> => {
    if (o.yes) {
      if (!o.createVault) throw new Error(`you have no vault named ${name}: check the name, or add --create-vault to create it`);
    } else if (!chosen && !o.createVault && !(await o.prompt.confirm(`You have no vault named ${name}. Create it?`, false))) {
      throw new Error("stopped: no vault chosen");
    }
    o.log(`  creating vault ${name}`);
    return api.createVault(name);
  };
  const shared = (v: Vault): Vault => {
    if (v.kind === "personal") o.log("  ! that is your personal vault: teammates cannot be invited to it");
    return v;
  };
  if (o.vault) return shared(find(o.vault) ?? (await create(o.vault)));
  const folder = basename(o.cwd);
  if (o.yes) {
    if (!o.createVault) throw new Error("pass --vault <name or id> (add --create-vault to make a new one named after the folder)");
    return find(folder) ?? create(folder);
  }
  // the personal vault cannot be shared, so it is not offered for a team project
  const offered = vaults.filter((v) => v.kind !== "personal");
  o.log("Vaults you can share with the team:");
  offered.forEach((v, i) => o.log(`  ${i + 1}) ${v.name} (${v.kind}${v.role ? `, ${v.role}` : ""})`));
  o.log("  n) a new vault");
  const a = await o.prompt.ask("Vault for this project", offered.length ? "1" : "n");
  const n = Number(a);
  if (Number.isInteger(n) && n >= 1 && n <= offered.length) return offered[n - 1];
  if (a === "n") {
    const name = await o.prompt.ask("New vault name", folder);
    return find(name) ?? create(name, true);
  }
  return shared(find(a) ?? (await create(a)));
}

/**
 * A `memory:write` token for this vault only. The new one is minted first; then only the token this
 * machine stored before (by id) is revoked, so other machines' copies keep working and a failed mint
 * never leaves a dead token behind.
 */
async function projectToken(api: Api, vault: Vault, user: User, oldId: string | undefined, o: InitOptions): Promise<{ token: string; tokenId?: string }> {
  const name = `create-eunomia: ${vault.name} (${hostname().split(".")[0] || "host"})`.slice(0, 80);
  let t: { id: string; token: string };
  try {
    t = await api.mintProjectToken(name, vault.id);
  } catch (e) {
    // a vault-restricted or expiring token cannot mint a narrower never-expiring one: use it as it is
    if (user.token && e instanceof ApiError && e.status === 403) {
      o.log(`  ! could not mint a project token (${e.message}); using the token you gave`);
      return { token: user.token };
    }
    throw e;
  }
  if (oldId && oldId !== t.id) await api.deleteToken(oldId).catch(() => undefined);
  return { token: t.token, tokenId: t.id };
}

export function connectAgentsScript(): string {
  return fileURLToPath(new URL("../connect-agents.sh", import.meta.url));
}

function runScript(args: string[], env: NodeJS.ProcessEnv): number | null {
  if (process.platform === "win32") return null;
  const r = spawnSync("bash", [connectAgentsScript(), ...args], { stdio: "inherit", env });
  return r.error ? null : (r.status ?? 1);
}

function manualAgentSteps(url: string, log: (l: string) => void): void {
  log("  Connect agents by hand (bash was not available):");
  log("    1. In the Eunomia app open Settings, API tokens, and create a token for each agent.");
  log(`    2. Add an HTTP MCP server named eunomia at ${url}/mcp with the header Authorization: Bearer <token>.`);
  log(`       Claude Code: claude mcp add --transport http eunomia ${url}/mcp --header "Authorization: Bearer <token>"`);
  log("    See docs/agents.md for the other agents.");
}

/**
 * connect-agents.sh writes each chosen agent's global config with its own new token, minted from the
 * account and with the account's full scopes (not limited to this vault). So it only runs when asked:
 * `--agents <list>`, or a yes to the interactive question (agents found on this machine only). It gets
 * the email and password, never the project token, and never a token the user passed in. Returns 1 if
 * the script failed.
 */
async function connectAgents(user: User | undefined, url: string, o: InitOptions): Promise<number> {
  const how = `npm create eunomia -- --url ${url} --email <you> --agents claude,codex`;
  if (o.agents === "none") return 0;
  if (!user) {
    o.log(`  agents: not changed. To connect some: ${how}`);
    return 0;
  }
  if (user.token) {
    o.log("  agents: not connected, because you signed in with a token and init does not copy your token into agent configs.");
    o.log(`    To give each agent its own token: ${how}, or see docs/agents.md`);
    return 0;
  }
  let args: string[];
  if (o.agents) args = ["--agents", o.agents];
  else if (o.yes) {
    o.log(`  agents: none connected (--yes connects none). To connect some: ${how}`);
    return 0;
  } else {
    const yes = await o.prompt.confirm(
      "Connect the AI agents installed on this machine? Each gets its own account-wide token in its global config",
      false,
    );
    if (!yes) {
      o.log(`  agents: none connected. Later: ${how}`);
      return 0;
    }
    args = ["--agents", "all", "--detected-only"];
  }
  o.log("Connecting agents (connect-agents.sh):");
  const env = { ...o.env, EUNOMIA_TOKEN: "", EUNOMIA_EMAIL: user.email ?? "", EUNOMIA_PASSWORD: user.password ?? "" };
  const status = (o.connectAgents ?? runScript)(["--url", url, ...args], env);
  if (status === null) {
    manualAgentSteps(url, o.log);
    return 0;
  }
  if (status !== 0) {
    o.log(`  ✗ connect-agents.sh failed (exit ${status}); see its message above`);
    return 1;
  }
  return 0;
}

export async function init(o: InitOptions): Promise<number> {
  const existing = readConfig(o.cwd);
  if (existing) o.log(`Found .eunomia/eunomia.toml: ${existing.vaultName} on ${existing.url}`);

  // 1. server. Credentials only go to a URL named this run (flag, EUNOMIA_URL or the prompt), or to
  // the committed file's URL once the user confirms it: a repo must not be able to steer them.
  const named = o.url ?? o.env.EUNOMIA_URL;
  let url: string;
  let fromRepo = false;
  if (named) {
    url = normalizeUrl(named);
    if (existing && existing.url !== url) {
      throw new Error(
        `.eunomia/eunomia.toml says the server is ${existing.url}, but you named ${url}. Nothing was sent. If the project really moved, edit the url in .eunomia/eunomia.toml (or delete it) and run again.`,
      );
    }
  } else if (existing) {
    url = existing.url;
    fromRepo = true;
  } else {
    url = normalizeUrl(await o.prompt.ask("Eunomia server URL", DEFAULT_URL));
  }
  if (url.startsWith("http://") && !isLocalUrl(url)) {
    o.log(`  ! ${url} is plain http: your password and tokens would cross the network unencrypted. Use https:// for a server on another machine.`);
  }
  const api = new Api(url);
  if (!(await api.healthy())) throw new Error(`no Eunomia server answers at ${url}/healthz: is it running?`);
  o.log(`  ✓ ${url} is up`);

  // a re-run keeps the vault unless asked to change it
  let changeVault = Boolean(o.vault && existing && o.vault !== existing.vaultId && o.vault !== existing.vaultName);
  if (existing && !o.vault && !o.yes) {
    changeVault = !(await o.prompt.confirm(`Keep the vault ${existing.vaultName}?`, true));
  }
  // a token this user stored for exactly this URL and vault may go back to it without asking
  const stored = existing && !changeVault ? loadCredentials(url, existing.vaultId, o.env) : undefined;
  const storedOk = stored && (await tokenWorks(url, stored.token, existing!.vaultId));
  const needSignIn = !storedOk || Boolean(o.agents && o.agents !== "none");

  let vault: Vault;
  let creds: Credentials;
  let user: User | undefined;
  if (!needSignIn) {
    vault = { id: existing!.vaultId, name: existing!.vaultName, kind: "" };
    creds = stored!;
    o.log(`  ✓ signed in already (token in ${relativeHome(o.env, credentialsPath(url, vault.id, o.env))})`);
  } else {
    if (fromRepo) {
      if (o.yes) {
        throw new Error(
          `signing in needs the server named explicitly: run again with --url ${url} (or EUNOMIA_URL), so a committed .eunomia/eunomia.toml cannot send your credentials somewhere else`,
        );
      }
      if (!(await o.prompt.confirm(`Sign in to ${url}, the server named in this repo's .eunomia/eunomia.toml?`, false))) {
        throw new Error("stopped: nothing was sent. Pass --url to name the server yourself.");
      }
    }
    try {
      // 2. sign in
      user = await signIn(api, o);
      o.log("  ✓ signed in");
      // 4. vault
      vault = await pickVault(api, o, changeVault ? undefined : existing?.vaultId);
      o.log(`  ✓ vault ${vault.name} (${vault.id})`);
      // 2b, 3. the project token, stored per user; an existing working one is kept
      const mine = loadCredentials(url, vault.id, o.env);
      if (mine && (await tokenWorks(url, mine.token, vault.id))) {
        creds = mine;
        o.log(`  ✓ kept your project token (${relativeHome(o.env, credentialsPath(url, vault.id, o.env))})`);
      } else {
        creds = { url, vaultId: vault.id, ...(await projectToken(api, vault, user, mine?.tokenId, o)) };
        const path = saveCredentials(creds, o.env);
        o.log(`  ✓ project token saved to ${relativeHome(o.env, path)} (mode 600, outside the repo)`);
      }
    } finally {
      await api.logout();
    }
  }

  // 5. .eunomia/
  if (!existing || existing.vaultId !== vault.id || existing.vaultName !== vault.name) {
    writeConfig(o.cwd, { url, vaultId: vault.id, vaultName: vault.name }, existing?.raw);
    o.log("  ✓ wrote .eunomia/eunomia.toml");
  }
  for (const f of ensureSkeleton(o.cwd)) o.log(`  ✓ wrote ${relative(o.cwd, f)}`);

  // 6. agents
  const agentsFailed = await connectAgents(user, url, o);

  // first sync
  o.log("Syncing .eunomia/skills:");
  const result = await runSync(new Api(url, creds.token), o.cwd, vault.id);
  o.log(formatSync(result, o.cwd));

  // 7. summary
  o.log("");
  o.log(agentsFailed ? `Eunomia project set up (vault ${vault.name} on ${url}), but connecting agents failed.` : `Eunomia project ready: vault ${vault.name} on ${url}.`);
  o.log("Commit the folder (it holds no secrets):");
  o.log('  git add .eunomia && git commit -m "Add Eunomia project"');
  o.log("Teammates run `npm create eunomia` in their clone and sign in with their own account (share the vault with them first, under Vaults, Members).");
  o.log("Sync skills after editing them: npx create-eunomia sync");
  return failed(result) || agentsFailed ? 1 : 0;
}

const failed = (r: SyncResult) => r.results.some((x) => x.outcome === "error");

/** One clear message when the token is refused, instead of one per skill. */
async function runSync(api: Api, cwd: string, vaultId: string): Promise<SyncResult> {
  const again = `${api.url} rejected the token: run \`npx create-eunomia\` again to sign in`;
  await api.me().catch((e) => {
    throw e instanceof ApiError && e.status === 401 ? new Error(again) : e;
  });
  try {
    return await syncSkills(api, skillsDir(cwd), vaultId);
  } catch (e) {
    throw e instanceof TokenRejected ? new Error(again) : e;
  }
}

function relativeHome(env: NodeJS.ProcessEnv, p: string): string {
  const home = env.HOME;
  return home && p.startsWith(home + "/") ? "~/" + p.slice(home.length + 1) : p;
}

/**
 * The project's config and the token for it: the one stored for this exact server and vault, or
 * EUNOMIA_TOKEN (CI). EUNOMIA_TOKEN only goes out when EUNOMIA_URL (or --url) names the same server
 * as the committed file. Sync and watch both start here.
 */
export function storedToken(o: { cwd: string; url?: string; env: NodeJS.ProcessEnv }): { cfg: ProjectConfig; token: string } {
  const cfg = readConfig(o.cwd);
  if (!cfg) throw new Error("no .eunomia/eunomia.toml here: run `npm create eunomia` first");
  const named = o.url ?? o.env.EUNOMIA_URL;
  if (named && normalizeUrl(named) !== cfg.url) {
    throw new Error(`.eunomia/eunomia.toml says the server is ${cfg.url}, but you named ${normalizeUrl(named)}. Nothing was sent.`);
  }
  let token: string | undefined;
  if (o.env.EUNOMIA_TOKEN) {
    if (!named) {
      throw new Error(`EUNOMIA_TOKEN is set: also set EUNOMIA_URL=${cfg.url} (this project's server) so the token only goes where you meant. Nothing was sent.`);
    }
    token = o.env.EUNOMIA_TOKEN;
  } else {
    token = loadCredentials(cfg.url, cfg.vaultId, o.env)?.token;
  }
  if (!token) throw new Error(`no token for ${cfg.url} on this machine: run \`npx create-eunomia\` to sign in (or set EUNOMIA_URL and EUNOMIA_TOKEN)`);
  return { cfg, token };
}

/** Saves .eunomia/skills into the project's vault (see [`storedToken`] for the credential). */
export async function sync(o: { cwd: string; url?: string; env: NodeJS.ProcessEnv; log: (l: string) => void }): Promise<number> {
  const { cfg, token } = storedToken(o);
  if (!existsSync(skillsDir(o.cwd))) o.log("  no .eunomia/skills folder yet");
  o.log(`Syncing .eunomia/skills to ${cfg.vaultName} on ${cfg.url}:`);
  const result = await runSync(new Api(cfg.url, token), o.cwd, cfg.vaultId);
  o.log(formatSync(result, o.cwd));
  const counts = ["created", "updated", "unchanged", "error"].map((k) => `${result.results.filter((r) => r.outcome === k).length} ${k}`);
  o.log(`  ${counts.join(", ")}`);
  return failed(result) ? 1 : 0;
}

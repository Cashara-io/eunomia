/** The `.eunomia/` folder (committed, never secret) and the per-user token store (~/.config/eunomia). */

import { createHash } from "node:crypto";
import { chmodSync, existsSync, lstatSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

export const DOCS_URL = "https://github.com/Cashara-io/eunomia/blob/main/docs/projects.md";

export interface ProjectConfig {
  url: string;
  vaultId: string;
  vaultName: string;
}

// ---------------------------------------------------------------------------
// TOML: the tiny subset eunomia.toml uses. Comments, blank lines, [section]
// headers and `key = "string"`. Anything else is an error, not a guess.
// ---------------------------------------------------------------------------

export type Toml = Record<string, Record<string, string>>;

const ESCAPES: Record<string, string> = { '"': '"', "\\": "\\", n: "\n", t: "\t", r: "\r" };

function unquote(raw: string, line: number): string {
  let out = "";
  for (let i = 0; i < raw.length; i++) {
    const c = raw[i];
    if (c === "\\") {
      const e = raw[++i];
      if (e === "u" && /^[0-9a-fA-F]{4}$/.test(raw.slice(i + 1, i + 5))) {
        out += String.fromCharCode(parseInt(raw.slice(i + 1, i + 5), 16));
        i += 4;
      } else if (e !== undefined && e in ESCAPES) out += ESCAPES[e];
      else throw new Error(`line ${line}: unsupported escape \\${e ?? ""}`);
    } else if (c === '"') throw new Error(`line ${line}: unescaped quote in string`);
    else out += c;
  }
  return out;
}

export function parseToml(text: string): Toml {
  const out: Toml = {};
  let section = "";
  text.split(/\r?\n/).forEach((l, i) => {
    const n = i + 1;
    const line = l.trim();
    if (!line || line.startsWith("#")) return;
    const head = /^\[([A-Za-z0-9_-]+)\]\s*(#.*)?$/.exec(line);
    if (head) {
      section = head[1];
      if (out[section]) throw new Error(`line ${n}: [${section}] appears twice`);
      out[section] = {};
      return;
    }
    const kv = /^([A-Za-z0-9_-]+)\s*=\s*"((?:[^"\\]|\\.)*)"\s*(#.*)?$/.exec(line);
    if (!kv) throw new Error(`line ${n}: expected [section] or key = "string"`);
    if (!section) throw new Error(`line ${n}: key outside a [section]`);
    if (kv[1] in out[section]) throw new Error(`line ${n}: ${kv[1]} appears twice in [${section}]`);
    out[section][kv[1]] = unquote(kv[2], n);
  });
  return out;
}

function quote(s: string): string {
  return '"' + s.replace(/[\\"]/g, (c) => "\\" + c).replace(/\n/g, "\\n").replace(/\t/g, "\\t").replace(/\r/g, "\\r").replace(/[\x00-\x1f\x7f]/g, (c) => "\\u" + c.charCodeAt(0).toString(16).padStart(4, "0")) + '"';
}

export function stringifyToml(data: Toml, header: string[] = []): string {
  const lines = header.map((h) => `# ${h}`);
  for (const [section, keys] of Object.entries(data)) {
    if (lines.length) lines.push("");
    lines.push(`[${section}]`);
    for (const [k, v] of Object.entries(keys)) lines.push(`${k} = ${quote(v)}`);
  }
  return lines.join("\n") + "\n";
}

// ---------------------------------------------------------------------------
// .eunomia/
// ---------------------------------------------------------------------------

/** `http(s)://host[:port][/path]` without a trailing slash, `/mcp` or `/api`. Never echoes the input, which may hold a password. */
export function normalizeUrl(u: string): string {
  const raw = u.trim();
  const bad = new Error("the server URL must start with http:// or https://");
  let parsed: URL;
  try {
    parsed = new URL(raw);
  } catch {
    throw bad;
  }
  if (parsed.username || parsed.password) throw new Error("the server URL must not contain a user name or password");
  const url = raw.replace(/\/+$/, "").replace(/\/(mcp|api)$/, "");
  if (!/^https?:$/.test(parsed.protocol) || !/^https?:\/\/[^\s"'\\]+$/.test(url)) throw bad;
  return url;
}

/** A host where plain http never leaves the machine. */
export function isLocalUrl(url: string): boolean {
  const h = new URL(url).hostname;
  return h === "localhost" || h.endsWith(".localhost") || /^127\./.test(h) || h === "[::1]";
}

export const projectDir = (root: string) => join(root, ".eunomia");
export const skillsDir = (root: string) => join(projectDir(root), "skills");
const configPath = (root: string) => join(projectDir(root), "eunomia.toml");

const HEADER = [
  "Eunomia project: every agent working in this repo shares the vault below.",
  `Docs: ${DOCS_URL}`,
  "Commit this folder. No token or password ever goes here: each person's token",
  "lives in ~/.config/eunomia (run `npx create-eunomia` to sign in).",
];

/** The project's config, or `undefined` when there is none yet. Throws on a malformed file. */
export function readConfig(root: string): (ProjectConfig & { raw: Toml }) | undefined {
  refuseSymlinks(root);
  const p = configPath(root);
  if (!existsSync(p)) return undefined;
  let raw: Toml;
  try {
    raw = parseToml(readFileSync(p, "utf8"));
  } catch (e) {
    throw new Error(`${p}: ${(e as Error).message}`);
  }
  const vaultId = raw.vault?.id;
  if (!raw.server?.url || !vaultId) throw new Error(`${p}: needs [server] url and [vault] id`);
  let url: string;
  try {
    url = normalizeUrl(raw.server.url);
  } catch (e) {
    throw new Error(`${p}: ${(e as Error).message}`);
  }
  return { url, vaultId, vaultName: raw.vault?.name ?? vaultId, raw };
}

/** Writes eunomia.toml, keeping any other sections and keys already in it. */
/** `.eunomia` and `.eunomia/skills` must be real folders in the repo, never symlinks to elsewhere. */
function refuseSymlinks(root: string): void {
  for (const dir of [projectDir(root), skillsDir(root)]) {
    let link = false;
    try {
      link = lstatSync(dir).isSymbolicLink();
    } catch {
      // not there yet
    }
    if (link) throw new Error(`${dir} is a symlink: the project folder must live in the repo`);
  }
}

export function writeConfig(root: string, cfg: ProjectConfig, previous: Toml = {}): void {
  refuseSymlinks(root);
  const data: Toml = { ...previous };
  data.server = { ...previous.server, url: cfg.url };
  data.vault = { ...previous.vault, id: cfg.vaultId, name: cfg.vaultName };
  // server and vault first, then whatever else was there
  const ordered: Toml = { server: data.server, vault: data.vault };
  for (const [k, v] of Object.entries(data)) if (!(k in ordered)) ordered[k] = v;
  mkdirSync(projectDir(root), { recursive: true });
  // ponytail: a rewrite drops hand-written comments; edit lines in place if people start annotating it
  writeFileSync(configPath(root), stringifyToml(ordered, HEADER));
}

const SKILLS_README = `# Project skills

Each \`*.md\` file here (except this README), or \`<name>/SKILL.md\` folder, is one skill,
in SKILL.md style:

\`\`\`markdown
---
name: deploy-rollback
description: How to roll back a bad deploy of this service. Use when a release breaks production.
---

1. Find the last good tag: \`git tag --sort=-creatordate | head\`.
2. ...
\`\`\`

\`name\` is lowercase letters, digits and hyphens (at most 64); without it the file
name is used. \`description\` says when to use the skill. Run \`npx create-eunomia sync\`
to save them into this project's vault, where every agent finds them with \`skill_find\`.
`;

/** Creates the folder skeleton, never overwriting a file that is already there. Returns what it created. */
export function ensureSkeleton(root: string): string[] {
  refuseSymlinks(root);
  const created: string[] = [];
  const put = (path: string, text: string) => {
    if (existsSync(path)) return;
    writeFileSync(path, text);
    created.push(path);
  };
  mkdirSync(skillsDir(root), { recursive: true });
  put(join(skillsDir(root), "README.md"), SKILLS_README);
  put(join(projectDir(root), ".gitignore"), "# local cache, never committed\n/cache/\n");
  return created;
}

// ---------------------------------------------------------------------------
// Per-user token store: the same ~/.config/eunomia folder and KEY=value format
// connect-agents.sh uses for its hook env, one file per (server, vault).
// ---------------------------------------------------------------------------

export interface Credentials {
  url: string;
  vaultId: string;
  token: string;
  tokenId?: string;
}

export function configHome(env: NodeJS.ProcessEnv = process.env): string {
  return join(env.XDG_CONFIG_HOME || join(env.HOME || homedir(), ".config"), "eunomia");
}

export function credentialsPath(url: string, vaultId: string, env: NodeJS.ProcessEnv = process.env): string {
  const key = createHash("sha256").update(`${url}\n${vaultId}`).digest("hex").slice(0, 16);
  return join(configHome(env), "projects", `${key}.env`);
}

export function saveCredentials(c: Credentials, env: NodeJS.ProcessEnv = process.env): string {
  const p = credentialsPath(c.url, c.vaultId, env);
  mkdirSync(join(configHome(env), "projects"), { recursive: true, mode: 0o700 });
  // tighten folders that already existed (connect-agents.sh makes ~/.config/eunomia with the umask)
  chmodSync(configHome(env), 0o700);
  chmodSync(join(configHome(env), "projects"), 0o700);
  const lines = [`EUNOMIA_URL=${c.url}`, `EUNOMIA_VAULT=${c.vaultId}`, `EUNOMIA_TOKEN=${c.token}`];
  if (c.tokenId) lines.push(`EUNOMIA_TOKEN_ID=${c.tokenId}`);
  writeFileSync(p, lines.join("\n") + "\n", { mode: 0o600 });
  chmodSync(p, 0o600); // an existing file keeps its old mode otherwise
  return p;
}

export function loadCredentials(url: string, vaultId: string, env: NodeJS.ProcessEnv = process.env): Credentials | undefined {
  const p = credentialsPath(url, vaultId, env);
  if (!existsSync(p)) return undefined;
  const kv: Record<string, string> = {};
  for (const line of readFileSync(p, "utf8").split("\n")) {
    const i = line.indexOf("=");
    if (i > 0) kv[line.slice(0, i)] = line.slice(i + 1);
  }
  if (!kv.EUNOMIA_TOKEN) return undefined;
  return { url, vaultId, token: kv.EUNOMIA_TOKEN, tokenId: kv.EUNOMIA_TOKEN_ID };
}

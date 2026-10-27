/** `.eunomia/skills/*.md` (SKILL.md style) and their sync into the project's vault. */

import { existsSync, readdirSync, readFileSync } from "node:fs";
import { basename, join } from "node:path";
import { Api, ApiError } from "./api.js";

export interface Skill {
  file: string;
  name: string;
  description: string;
  body: string;
}

// The server's rules (backend/src/tools/registry/skills.rs).
const NAME = /^[a-z0-9][a-z0-9-]{0,63}$/;
const MAX_DESCRIPTION = 1024;
const MAX_BODY = 64 * 1024;

export function validName(name: string): boolean {
  return NAME.test(name);
}

/**
 * Splits a SKILL.md into frontmatter fields and body. The frontmatter is the YAML subset SKILL.md
 * files use: `key: value` (optionally quoted), `key: >` or `key: |` block scalars, and nested or
 * listed values under keys this tool does not read (skipped). Anything else is an error.
 */
export function parseFrontmatter(text: string): { fields: Record<string, string>; body: string } {
  const lines = text.replace(/^﻿/, "").split(/\r?\n/);
  if (lines[0]?.trim() !== "---") throw new Error("must start with a --- frontmatter block (name, description)");
  const end = lines.findIndex((l, i) => i > 0 && l.trim() === "---");
  if (end < 0) throw new Error("frontmatter has no closing ---");
  const fields: Record<string, string> = {};
  let i = 1;
  while (i < end) {
    const line = lines[i];
    if (!line.trim() || line.trimStart().startsWith("#")) {
      i++;
      continue;
    }
    const m = /^([A-Za-z0-9_-]+):(?:\s+(.*?))?\s*$/.exec(line);
    if (!m) throw new Error(`frontmatter line ${i + 1}: expected "key: value", got ${JSON.stringify(line)}`);
    const [, key, value = ""] = m;
    if (key in fields) throw new Error(`frontmatter line ${i + 1}: ${key} appears twice`);
    i++;
    // indented (and blank) lines that follow belong to this key
    const block: string[] = [];
    while (i < end && (!lines[i].trim() || /^\s/.test(lines[i]))) block.push(lines[i++]);
    const rows = block.filter((l) => l.trim());
    if (/^[>|][-+]?$/.test(value)) {
      const indent = Math.min(...rows.map((l) => l.length - l.trimStart().length));
      const text = block.map((l) => l.slice(indent));
      fields[key] = (value[0] === ">" ? text.join(" ").replace(/\s+/g, " ") : text.join("\n")).trim();
    } else if (rows.length && value) {
      // a plain value carried onto indented lines folds into one line, as in YAML
      if (/^["']/.test(value)) throw new Error(`frontmatter line ${i - block.length}: a quoted ${key} must fit on one line`);
      fields[key] = [value, ...rows.map((l) => l.trim())].join(" ");
    } else if (rows.length) {
      // a nested map or list (metadata:, allowed-tools:): not ours, skipped
    } else {
      fields[key] = unquoteYaml(value, i);
    }
  }
  const body = lines.slice(end + 1).join("\n").replace(/^\s*\n/, "").trimEnd();
  return { fields, body };
}

function unquoteYaml(v: string, line: number): string {
  if (v.startsWith('"')) {
    if (!/^"(?:[^"\\]|\\.)*"$/.test(v)) throw new Error(`frontmatter line ${line}: unterminated "string"`);
    try {
      return JSON.parse(v);
    } catch {
      throw new Error(`frontmatter line ${line}: unsupported escape in "string"`);
    }
  }
  if (v.startsWith("'")) {
    if (!/^'(?:[^']|'')*'$/.test(v)) throw new Error(`frontmatter line ${line}: unterminated 'string'`);
    return v.slice(1, -1).replace(/''/g, "'");
  }
  return v.replace(/\s+#.*$/, "");
}

/** Reads one skill file and checks it against the server's rules. */
export function readSkill(path: string): Skill {
  const { fields, body } = parseFrontmatter(readFileSync(path, "utf8"));
  const name = fields.name || basename(path).replace(/\.md$/i, "");
  const description = (fields.description ?? "").trim();
  if (!validName(name)) throw new Error(`name ${JSON.stringify(name)} must be 1 to 64 lowercase letters, digits and hyphens, starting with a letter or digit`);
  if (!description) throw new Error("needs a description: when to use the skill");
  if ([...description].length > MAX_DESCRIPTION) throw new Error(`description is longer than ${MAX_DESCRIPTION} characters`);
  if (!body.trim()) throw new Error("the body (after the frontmatter) is empty");
  if (Buffer.byteLength(body) > MAX_BODY) throw new Error("the body is larger than 64 KiB");
  return { file: path, name, description, body };
}

export type Outcome = "created" | "updated" | "unchanged" | "error";
export interface SyncResult {
  results: { file: string; name?: string; outcome: Outcome; error?: string }[];
  /** Skill names in the vault with no file in the folder. Never deleted. */
  serverOnly: string[];
}

/** Common letter pairs: `skill_find` has no list-all, but its keyword arm matches any name or description holding one. */
// ponytail: best effort, at most 50 skills; a server-side skill_list would make "only on the server" exact.
const LIST_QUERY = "an ar at de ed en er es in it le nd on or re se st te th ti to";

/** Upserts every skill file into `vaultId`. One bad file is reported and the rest carry on. */
export async function syncSkills(api: Api, dir: string, vaultId: string): Promise<SyncResult> {
  const files = existsSync(dir)
    ? readdirSync(dir)
        .filter((f) => f.toLowerCase().endsWith(".md") && f.toLowerCase() !== "readme.md")
        .sort()
    : [];
  const results: SyncResult["results"] = [];
  const seen = new Map<string, string>();
  for (const f of files) {
    const file = join(dir, f);
    let skill: Skill;
    try {
      skill = readSkill(file);
    } catch (e) {
      results.push({ file, outcome: "error", error: (e as Error).message });
      continue;
    }
    if (seen.has(skill.name)) {
      results.push({ file, name: skill.name, outcome: "error", error: `name ${skill.name} is already used by ${basename(seen.get(skill.name)!)}` });
      continue;
    }
    seen.set(skill.name, file);
    try {
      results.push({ file, name: skill.name, outcome: await upsert(api, skill, vaultId) });
    } catch (e) {
      results.push({ file, name: skill.name, outcome: "error", error: (e as Error).message });
    }
  }
  let serverOnly: string[] = [];
  try {
    const found = await api.tool<{ skills: { name: string }[] }>("skill_find", { query: LIST_QUERY, vault_id: vaultId, limit: 50 });
    serverOnly = [...new Set(found.skills.map((s) => s.name))].filter((n) => !seen.has(n)).sort();
  } catch {
    // only a report; the sync itself is done
  }
  return { results, serverOnly };
}

async function upsert(api: Api, s: Skill, vaultId: string): Promise<Outcome> {
  let existing: { description: string; body: string } | undefined;
  try {
    existing = await api.tool("skill_get", { name: s.name, vault_id: vaultId });
  } catch (e) {
    if (!(e instanceof ApiError && e.code === "resource.not_found")) throw e;
  }
  if (existing && existing.description === s.description && existing.body === s.body) return "unchanged";
  await api.tool("skill_save", { name: s.name, description: s.description, body: s.body, vault_id: vaultId });
  return existing ? "updated" : "created";
}

export function formatSync(r: SyncResult, cwd: string): string {
  const rel = (f: string) => (f.startsWith(cwd + "/") ? f.slice(cwd.length + 1) : f);
  const lines = r.results.map((x) => `  ${x.outcome.padEnd(9)} ${x.name ?? "-"}  ${rel(x.file)}${x.error ? `: ${x.error}` : ""}`);
  if (!r.results.length) lines.push("  no skills yet: add .eunomia/skills/<name>.md and run sync again");
  if (r.serverOnly.length) lines.push(`  on the server only (kept, not deleted): ${r.serverOnly.join(", ")}`);
  return lines.join("\n");
}

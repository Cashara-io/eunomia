#!/usr/bin/env node
/** Entry point: `npm create eunomia` (init) and `npx create-eunomia sync`. */

import { init, sync } from "./init.js";
import { defaultsPrompter, terminalPrompter } from "./prompt.js";

const HELP = `create-eunomia: turn this repo into a Eunomia project (one shared vault for every agent)

  npm create eunomia [-- flags]        set up .eunomia/ here (also bun create / pnpm create)
  npx create-eunomia sync              save .eunomia/skills/*.md into the project's vault

Flags (init):
  --url <url>        Eunomia server (or EUNOMIA_URL; default http://localhost:8001)
  --email <email>    sign in with email and password (or EUNOMIA_EMAIL)
  --password <pw>    the password (prefer EUNOMIA_PASSWORD, or the hidden prompt)
  --token <token>    sign in with an API token instead (or EUNOMIA_TOKEN)
  --vault <id|name>  the project's vault; a name that does not exist is created
  --agents <list>    agents to connect: auto (default), all, none, or claude,codex,...
  -y, --yes          no questions: take the defaults
  -h, --help         this help

Sync uses the token init stored in ~/.config/eunomia, or EUNOMIA_TOKEN.
Docs: https://github.com/Cashara-io/eunomia/blob/main/docs/projects.md`;

const VALUE_FLAGS = ["url", "email", "password", "token", "vault", "agents"] as const;

export function parseArgs(argv: string[]): { command: string; flags: Record<string, string>; yes: boolean; help: boolean } {
  const flags: Record<string, string> = {};
  let command = "init";
  let yes = false;
  let help = false;
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "-y" || a === "--yes") yes = true;
    else if (a === "-h" || a === "--help") help = true;
    else if (a.startsWith("--")) {
      const [k, inline] = a.slice(2).split(/=(.*)/s, 2);
      if (!(VALUE_FLAGS as readonly string[]).includes(k)) throw new Error(`unknown flag ${a} (see --help)`);
      const v = inline ?? argv[++i];
      if (v === undefined) throw new Error(`${a} needs a value`);
      flags[k] = v;
    } else if (a === "sync" || a === "init") command = a;
    else throw new Error(`unknown command ${a} (see --help)`);
  }
  return { command, flags, yes, help };
}

async function main(): Promise<number> {
  const args = parseArgs(process.argv.slice(2));
  if (args.help) {
    console.log(HELP);
    return 0;
  }
  const log = (l: string) => console.log(l);
  if (args.command === "sync") return sync({ cwd: process.cwd(), env: process.env, log });
  const prompt = args.yes ? defaultsPrompter() : terminalPrompter();
  try {
    return await init({ cwd: process.cwd(), ...args.flags, yes: args.yes, env: process.env, prompt, log });
  } finally {
    prompt.close();
  }
}

main().then(
  (code) => process.exit(code),
  (e) => {
    console.error(`  ✗ ${(e as Error).message}`);
    process.exit(1);
  },
);

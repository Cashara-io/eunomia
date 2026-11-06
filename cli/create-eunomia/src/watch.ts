/** `create-eunomia watch`: one line per change to the project's vault, live, resuming where it stopped. */

import { lstatSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { Api, ApiError, type Change } from "./api.js";
import { storedToken } from "./init.js";
import { projectDir } from "./project.js";

const PAGE = 1000;

export interface WatchOptions {
  cwd: string;
  url?: string;
  env: NodeJS.ProcessEnv;
  log: (l: string) => void;
  /** Ends the watch (Ctrl-C in the CLI). */
  signal?: AbortSignal;
  /** The WebSocket class: the runtime's own by default (Node 22+, Bun); `null` polls instead (Node 18). */
  WebSocket?: typeof globalThis.WebSocket | null;
  /** How often to poll without a WebSocket, and how long to wait before reconnecting one. */
  pollMs?: number;
}

/** The cursor: the last seq printed, per vault, in the project's git-ignored cache. */
const cacheDir = (cwd: string) => join(projectDir(cwd), "cache");
const cursorPath = (cwd: string) => join(cacheDir(cwd), "watch.json");

/**
 * A repo can commit `.eunomia/cache` or `watch.json` as a symlink (to `~/.zshrc`, say): refuse to read
 * or write through one, so a hostile repo cannot make watch overwrite a file outside it.
 */
function refuseLinks(cwd: string): void {
  for (const p of [cacheDir(cwd), cursorPath(cwd)]) {
    let link = false;
    try {
      link = lstatSync(p).isSymbolicLink();
    } catch {
      // not there yet
    }
    if (link) throw new Error(`${p} is a symlink: remove it (watch keeps its cursor there and will not follow a link)`);
  }
}

export function readCursor(cwd: string, vaultId: string): number {
  refuseLinks(cwd);
  try {
    const c = JSON.parse(readFileSync(cursorPath(cwd), "utf8"));
    return c.vault === vaultId && Number.isSafeInteger(c.after) && c.after > 0 ? c.after : 0;
  } catch {
    return 0;
  }
}

/** A new file beside the cursor (never an existing one or a link), then renamed over it: a crash leaves the old or the new cursor. */
function saveCursor(cwd: string, vaultId: string, after: number): void {
  mkdirSync(cacheDir(cwd), { recursive: true });
  refuseLinks(cwd);
  const tmp = `${cursorPath(cwd)}.${process.pid}.tmp`;
  rmSync(tmp, { force: true });
  writeFileSync(tmp, JSON.stringify({ vault: vaultId, after }) + "\n", { flag: "wx" });
  renameSync(tmp, cursorPath(cwd));
}

export function formatChange(c: Change): string {
  return `#${c.seq}  ${c.op.padEnd(7)}  ${c.kind.padEnd(8)}  ${c.id}  by ${c.actor}  ${c.at}`;
}

const sleep = (ms: number, signal?: AbortSignal) =>
  new Promise<void>((resolve) => {
    const t = setTimeout(resolve, ms);
    signal?.addEventListener("abort", () => (clearTimeout(t), resolve()), { once: true });
  });

export async function watch(o: WatchOptions): Promise<number> {
  const { cfg, token } = storedToken(o);
  const api = new Api(cfg.url, token);
  const Ws = o.WebSocket === undefined ? globalThis.WebSocket : o.WebSocket;
  const pollMs = o.pollMs ?? 3000;
  let after = readCursor(o.cwd, cfg.vaultId);
  const print = (c: Change) => {
    // the socket and a catch-up page can overlap: print each seq once, in order
    if (c.seq <= after) return;
    o.log(formatChange(c));
    after = c.seq;
    saveCursor(o.cwd, cfg.vaultId, after);
  };
  o.log(`Watching ${cfg.vaultName} on ${cfg.url} after change ${after} (${Ws ? "live" : `polling every ${pollMs / 1000}s`}). Ctrl-C stops.`);

  while (!o.signal?.aborted) {
    // REST first on every (re)connect: it catches up, and a refused token ends the watch here
    // (a refused WebSocket handshake only says "closed")
    try {
      let page;
      do {
        page = await api.changes(cfg.vaultId, after, PAGE, o.signal);
        page.changes.forEach(print);
      } while (page.changes.length === PAGE);
    } catch (e) {
      if (o.signal?.aborted) break;
      if (e instanceof ApiError && (e.status === 401 || e.status === 403 || e.code === "client.redirect")) {
        throw new Error(`${cfg.url} refused the request (${e.message}): run \`npx create-eunomia\` again to sign in`);
      }
      // the server no longer keeps the changes after our cursor (retention), or the cursor is past its
      // newest change (a restored database): say so, and read on from the oldest change it keeps
      if (e instanceof ApiError && e.code === "changes.resync") {
        o.log(`  the server can no longer list the changes after #${after} (${e.message}).`);
        o.log("  Changes in between are not shown: re-check what you need (for skills, `npx create-eunomia sync`). Reading on from the oldest change it keeps.");
        after = 0;
        saveCursor(o.cwd, cfg.vaultId, 0);
        continue;
      }
      o.log(`  ${(e as Error).message}; retrying`);
      await sleep(pollMs, o.signal);
      continue;
    }
    if (Ws) await follow(Ws, cfg.url, token, cfg.vaultId, after, print, o.signal);
    await sleep(pollMs, o.signal);
  }
  return 0;
}

/** Prints changes from the WebSocket until it closes (or `signal` aborts). */
function follow(
  Ws: typeof globalThis.WebSocket,
  url: string,
  token: string,
  vaultId: string,
  after: number,
  print: (c: Change) => void,
  signal?: AbortSignal,
): Promise<void> {
  return new Promise((resolve) => {
    const wsUrl = `${url.replace(/^http/, "ws")}/api/vaults/${encodeURIComponent(vaultId)}/changes/ws?after=${after}`;
    // the token goes only to the configured server; Node's and Bun's WebSocket take headers (browsers' cannot)
    const ws = new Ws(wsUrl, { headers: { authorization: `Bearer ${token}` } } as unknown as string[]);
    const stop = () => ws.close();
    signal?.addEventListener("abort", stop, { once: true });
    ws.onmessage = (ev) => {
      try {
        print(JSON.parse(String(ev.data)));
      } catch {
        // not a change: ignore
      }
    };
    ws.onerror = () => undefined; // a close follows
    ws.onclose = () => {
      signal?.removeEventListener("abort", stop);
      resolve();
    };
  });
}

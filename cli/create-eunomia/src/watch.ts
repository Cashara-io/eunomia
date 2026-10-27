/** `create-eunomia watch`: one line per change to the project's vault, live, resuming where it stopped. */

import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
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
const cursorPath = (cwd: string) => join(projectDir(cwd), "cache", "watch.json");

export function readCursor(cwd: string, vaultId: string): number {
  try {
    const c = JSON.parse(readFileSync(cursorPath(cwd), "utf8"));
    return c.vault === vaultId && Number.isSafeInteger(c.after) && c.after > 0 ? c.after : 0;
  } catch {
    return 0;
  }
}

function saveCursor(cwd: string, vaultId: string, after: number): void {
  mkdirSync(join(projectDir(cwd), "cache"), { recursive: true });
  writeFileSync(cursorPath(cwd), JSON.stringify({ vault: vaultId, after }) + "\n");
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
        page = await api.changes(cfg.vaultId, after, PAGE);
        page.changes.forEach(print);
      } while (page.changes.length === PAGE);
    } catch (e) {
      if (e instanceof ApiError && (e.status === 401 || e.status === 403 || e.code === "client.redirect")) {
        throw new Error(`${cfg.url} refused the request (${e.message}): run \`npx create-eunomia\` again to sign in`);
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

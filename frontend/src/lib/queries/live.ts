// Live updates (#132): one WebSocket per tab to `/api/events/ws` that says what changed in the user's
// memory spaces -- by an agent over MCP, by chat, in another tab, or by another member -- and turns
// each message into React Query invalidations of only the affected keys. While it is down, the views
// fall back to polling (`livePoll`). Message types are documented in backend/src/routers/changes.rs.
import { z } from "zod";
import { useEffect, useSyncExternalStore } from "react";
import { useQueryClient, type QueryClient } from "@tanstack/react-query";
import { apiOrigin } from "@/lib/api";
import type { Vault } from "@/lib/types";
import { documentKeys } from "./documents";
import { ontologyKeys } from "./ontology";
import { entityKeys } from "./entities";
import { getLiveStatus, setLiveStatus, subscribeLiveStatus, type LiveStatus } from "./liveStatus";
import { vaultKeys } from "./vaults";

const messageSchema = z.discriminatedUnion("type", [
  z.object({ type: z.literal("hello"), vaults: z.record(z.string(), z.number()), ontology: z.record(z.string(), z.number()).optional() }),
  z.object({ type: z.literal("change"), vault: z.string(), seq: z.number(), kind: z.string() }),
  z.object({ type: z.literal("ontology"), vault: z.string(), event: z.object({ seq: z.number().int().nonnegative() }) }),
  z.object({ type: z.literal("ontology_resync"), vault: z.string(), seq: z.number().int().nonnegative() }),
  z.object({ type: z.literal("seq"), vault: z.string(), seq: z.number() }),
  z.object({ type: z.literal("vault"), vault: z.string(), op: z.string() }),
  z.object({ type: z.enum(["vaults", "resync", "heartbeat"]) }),
]);
type Message = z.infer<typeof messageSchema>;

/** Vault ops that change what the graph, memory map and documents can show, not just the vault list. */
const SCOPE_OPS = new Set(["created", "cloned", "merged", "deleted", "invitation_accepted", "left", "member_removed"]);
/** Gather a burst (a clone logs one change per copied item) into one refetch. */
const FLUSH_MS = 400;
/** No message (heartbeats come every 10 s) for this long: the socket is dead even if it looks open. */
const STALE_MS = 35_000;
const MAX_BACKOFF_MS = 30_000;

type Pending = { all: boolean; vaults: boolean; entities: boolean; documents: boolean; members: Set<string>; lost: Set<string>; ontology: Set<string> };
const fresh = (): Pending => ({ all: false, vaults: false, entities: false, documents: false, members: new Set(), lost: new Set(), ontology: new Set() });

/** Applies a batch of invalidations: vault list first, so members are refetched only for vaults the user still has. */
async function flush(qc: QueryClient, p: Pending) {
  if (p.all || p.vaults) {
    for (const id of p.lost) {
      await qc.cancelQueries({ queryKey: vaultKeys.members(id) });
      qc.removeQueries({ queryKey: vaultKeys.members(id) });
    }
    await Promise.all([qc.invalidateQueries({ queryKey: vaultKeys.list() }), qc.invalidateQueries({ queryKey: vaultKeys.invitations() })]);
    const still = new Set((qc.getQueryData<Vault[]>(vaultKeys.list()) ?? []).map((v) => v.id));
    const ids = p.all
      ? qc
          .getQueryCache()
          .findAll({ queryKey: [...vaultKeys.all, "members"] })
          .map((q) => String(q.queryKey[2]))
      : [...p.members];
    for (const id of ids) {
      // a member query for a vault you just lost would answer 403
      if (still.has(id)) void qc.invalidateQueries({ queryKey: vaultKeys.members(id) });
      else qc.removeQueries({ queryKey: vaultKeys.members(id) });
    }
  }
  for (const vault of p.ontology) {
    // A first fetch may have read just before hello or the event. Cancel it so
    // its late response cannot hide a proposal whose cursor we already saw.
    const keys = [ontologyKeys.vault(vault), ["ontology-activity", vault], ["objects", vault], ["object-kinds", vault], ["object-profile", vault]];
    await Promise.all(keys.map(queryKey => qc.cancelQueries({ queryKey })));
    await Promise.all(keys.map(queryKey => qc.invalidateQueries({ queryKey })));
  }
  if (p.all || p.entities || p.documents || p.vaults) void qc.invalidateQueries({ queryKey: ontologyKeys.all });
  if (p.all || p.entities) {
    void qc.invalidateQueries({ queryKey: entityKeys.all });
    for (const key of ["objects", "object-kinds", "object-profile"]) void qc.invalidateQueries({ queryKey: [key] });
  }
  if (p.all || p.documents) void qc.invalidateQueries({ queryKey: documentKeys.all });
}

/** Opens the stream for as long as the calling component is mounted. Mount once, inside the signed-in layout. */
export function useLiveUpdates() {
  const qc = useQueryClient();
  useEffect(() => {
    let ws: WebSocket | null = null;
    let stopped = false;
    let attempts = 0;
    let everConnected = false;
    let lastSeen = Date.now();
    let reconnectTimer: ReturnType<typeof setTimeout> | undefined;
    let flushTimer: ReturnType<typeof setTimeout> | undefined;
    // the last seq seen per vault, so a reconnect can tell which vaults changed while it was away
    const cursors = new Map<string, number>();
    const ontologyCursors = new Map<string, number>();
    let pending = fresh();

    const schedule = () => {
      if (flushTimer) return;
      flushTimer = setTimeout(() => {
        flushTimer = undefined;
        const batch = pending;
        pending = fresh();
        void flush(qc, batch);
      }, FLUSH_MS);
    };

    const onMessage = (m: Message) => {
      switch (m.type) {
        case "hello": {
          setLiveStatus("live");
          attempts = 0;
          if (everConnected) {
            // vault events are not replayed: refetch the list; content only where the log moved on
            pending.vaults = true;
            for (const [vault, seq] of Object.entries(m.vaults)) {
              const known = cursors.get(vault);
              if (known === undefined || seq > known) pending.entities = pending.documents = true;
            }
          }
          const available = new Set(Object.keys(m.vaults));
          for (const vault of ontologyCursors.keys()) if (!available.has(vault)) ontologyCursors.delete(vault);
          for (const [vault, seq] of Object.entries(m.ontology ?? {})) {
            ontologyCursors.set(vault, seq);
            pending.ontology.add(vault);
          }
          everConnected = true;
          cursors.clear();
          for (const [vault, seq] of Object.entries(m.vaults)) cursors.set(vault, seq);
          break;
        }
        case "ontology":
          if (m.event.seq <= (ontologyCursors.get(m.vault) ?? 0)) return;
          ontologyCursors.set(m.vault, m.event.seq);
          pending.ontology.add(m.vault);
          break;
        case "ontology_resync":
          ontologyCursors.set(m.vault, m.seq);
          pending.ontology.add(m.vault);
          break;
        case "change":
          cursors.set(m.vault, Math.max(cursors.get(m.vault) ?? 0, m.seq));
          if (m.kind === "document") pending.documents = true;
          else if (m.kind === "memory" || m.kind === "entity" || m.kind === "relation") pending.entities = true;
          break;
        case "seq":
          cursors.set(m.vault, m.seq);
          pending.entities = pending.documents = true;
          break;
        case "vault":
          pending.vaults = true;
          pending.members.add(m.vault);
          if (m.op === "deleted") pending.lost.add(m.vault);
          if (SCOPE_OPS.has(m.op)) pending.entities = pending.documents = true;
          break;
        case "vaults":
          pending.vaults = pending.entities = pending.documents = true;
          break;
        case "resync":
          pending.all = true;
          break;
        default:
          return;
      }
      schedule();
    };

    const connect = () => {
      if (stopped) return;
      clearTimeout(reconnectTimer);
      if (getLiveStatus() !== "live") setLiveStatus("connecting");
      let socket: WebSocket;
      try {
        const resume = new URLSearchParams({ ontology_after: JSON.stringify(Object.fromEntries(ontologyCursors)) });
        socket = new WebSocket(`${apiOrigin().replace(/^http/, "ws")}/api/events/ws?${resume}`);
      } catch {
        return retry(false);
      }
      ws = socket;
      lastSeen = Date.now();
      socket.onmessage = (e) => {
        lastSeen = Date.now();
        try {
          const parsed = messageSchema.safeParse(JSON.parse(String(e.data)));
          if (parsed.success) onMessage(parsed.data);
        } catch {
          // not ours to understand; ignore
        }
      };
      socket.onclose = (e) => {
        if (ws !== socket) return;
        ws = null;
        // 1008: the session ended or lost access; wait longer, the session check will send the user to sign in
        retry(e.code === 1008);
      };
    };

    const retry = (denied: boolean) => {
      if (stopped) return;
      const wasLive = getLiveStatus() === "live";
      setLiveStatus("offline");
      // polling resumes after a view's next fetch: start one now, and catch up on what was missed
      if (wasLive) {
        pending.all = true;
        schedule();
      }
      attempts += 1;
      const wait = denied ? 60_000 : Math.min(MAX_BACKOFF_MS, 1000 * 2 ** Math.min(attempts - 1, 5));
      reconnectTimer = setTimeout(connect, wait + Math.random() * 500);
    };

    const watchdog = setInterval(() => {
      if (ws && Date.now() - lastSeen > STALE_MS) ws.close();
    }, 10_000);
    const onVisible = () => {
      if (document.visibilityState === "visible" && !ws) connect();
    };
    document.addEventListener("visibilitychange", onVisible);
    connect();

    return () => {
      stopped = true;
      clearTimeout(reconnectTimer);
      clearTimeout(flushTimer);
      clearInterval(watchdog);
      document.removeEventListener("visibilitychange", onVisible);
      const socket = ws;
      ws = null;
      socket?.close();
      setLiveStatus("connecting");
    };
  }, [qc]);
}

/** The live stream's state, for a status word next to Refresh. */
export const useLiveStatus = (): LiveStatus => useSyncExternalStore(subscribeLiveStatus, getLiveStatus, () => "connecting");

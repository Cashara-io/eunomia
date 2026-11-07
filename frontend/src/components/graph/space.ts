"use client";

// The memory space People & topics shows, shared by its List, Connections and Memory map views, so
// switching tabs never switches space. `undefined` is Personal (the graph endpoint's default).
// An explicit pick is remembered in this browser; until then the page opens on the newest shared
// space that has anything in it, else Personal.

import { useSyncExternalStore } from "react";
import { useVaults } from "@/lib/queries/vaults";
import { useEntityCounts } from "@/lib/queries/entities";

const SPACE_KEY = "eunomia.graph.space"; // "" is Personal
const listeners = new Set<() => void>();
let loaded = false;
let chosen: string | null = null; // null: never picked

function load() {
  if (loaded) return;
  loaded = true;
  try {
    chosen = window.localStorage.getItem(SPACE_KEY);
  } catch {}
}
function subscribe(l: () => void) {
  listeners.add(l);
  return () => listeners.delete(l);
}
function snapshot() {
  load();
  return chosen;
}

/** Pick the space every People & topics view shows (`undefined` is Personal). */
export function chooseSpace(id: string | undefined) {
  chosen = id ?? "";
  loaded = true;
  try {
    window.localStorage.setItem(SPACE_KEY, chosen);
  } catch {}
  listeners.forEach((l) => l());
}

export function useSharedSpace(enabled = true): { vaultId: string | undefined; ready: boolean } {
  const stored = useSyncExternalStore(subscribe, snapshot, () => null);
  const vaultsQuery = useVaults();
  const vaults = vaultsQuery.data ?? [];
  const storedValid = stored !== null && (stored === "" || vaults.some((v) => v.id === stored));
  const shared = vaults.filter((v) => v.kind !== "personal").sort((a, b) => (b.created_at ?? "").localeCompare(a.created_at ?? ""));
  const counts = useEntityCounts(
    shared.map((v) => v.id),
    enabled && vaultsQuery.isSuccess && !storedValid,
  );
  if (storedValid) return { vaultId: stored || undefined, ready: true };
  return {
    vaultId: shared.find((v) => (counts.counts.get(v.id) ?? 0) > 0)?.id,
    ready: vaultsQuery.isFetched && !counts.pending,
  };
}

// ---- the Memory map's layers -------------------------------------------------------------------
// The map can lay several spaces over each other. Its set always holds the space the other views
// show: pick another space there and the map starts again from that one, unless it is already
// layered. Remembered in this browser, as ids (Personal by its real id).

const MAP_KEY = "eunomia.map.spaces";
const mapListeners = new Set<() => void>();
let mapLoaded = false;
let mapRaw: string | null = null; // JSON array of vault ids, as stored

function mapSubscribe(l: () => void) {
  mapListeners.add(l);
  return () => mapListeners.delete(l);
}
function mapSnapshot() {
  if (!mapLoaded) {
    mapLoaded = true;
    try {
      mapRaw = window.localStorage.getItem(MAP_KEY);
    } catch {}
  }
  return mapRaw;
}
function saveMap(ids: string[]) {
  mapRaw = JSON.stringify(ids);
  mapLoaded = true;
  try {
    window.localStorage.setItem(MAP_KEY, mapRaw);
  } catch {}
  mapListeners.forEach((l) => l());
}

/**
 * The spaces the Memory map shows, in the order of the space list. `toggle` adds or removes one (the
 * last one stays), and keeps the other views on a space that is still shown.
 */
export function useMapSpaces(): { ids: string[]; ready: boolean; toggle: (id: string) => void } {
  const shared = useSharedSpace();
  const raw = useSyncExternalStore(mapSubscribe, mapSnapshot, () => null);
  const vaults = useVaults().data ?? [];
  const personal = vaults.find((v) => v.kind === "personal");
  const current = shared.vaultId ?? personal?.id;
  let stored: string[] = [];
  try {
    const parsed: unknown = raw ? JSON.parse(raw) : [];
    if (Array.isArray(parsed)) stored = parsed.filter((x): x is string => typeof x === "string");
  } catch {}
  const live = new Set(vaults.map((v) => v.id));
  const valid = stored.filter((id) => live.has(id));
  const set = current && valid.includes(current) ? valid : current ? [current] : [];
  const ids = vaults.map((v) => v.id).filter((id) => set.includes(id));

  function toggle(id: string) {
    const on = ids.includes(id);
    if (on && ids.length === 1) return; // the map always shows something
    const next = on ? ids.filter((x) => x !== id) : [...ids, id];
    saveMap(next);
    // the other views follow a space the map still shows
    if (current && !next.includes(current)) {
      const keep = next[0];
      chooseSpace(keep === personal?.id ? undefined : keep);
    }
  }
  return { ids, ready: shared.ready && vaults.length > 0, toggle };
}

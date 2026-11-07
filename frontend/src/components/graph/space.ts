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

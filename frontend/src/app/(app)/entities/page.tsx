"use client";

import { useSyncExternalStore } from "react";
import { useIsFetching, useQueryClient } from "@tanstack/react-query";
import { RefreshCw, Users } from "lucide-react";
import { entityKeys } from "@/lib/queries/entities";
import EntityGraph from "@/components/EntityGraph";
import LiveStatusBadge from "@/components/live/LiveStatusBadge";
import VectorCloud from "@/components/VectorCloud";
import PageHeader from "@/components/ui/PageHeader";

const VIEWS = [
  { id: "list", label: "List", blurb: "Everyone and everything your AI apps remember, newest first." },
  { id: "graph", label: "Connections", blurb: "People, organisations, places and projects, and how they connect." },
  { id: "cloud", label: "Memory map", blurb: "Every saved memory, placed so that similar memories sit close together." },
] as const;
type ViewId = (typeof VIEWS)[number]["id"];

// The view last picked here is remembered in this browser. Until then: List, the first tab, everywhere.
const VIEW_KEY = "eunomia.entities.view";
const viewListeners = new Set<() => void>();
let viewFallback: ViewId | null = null; // when storage is blocked, remembered for this visit only
function subscribeView(l: () => void) {
  viewListeners.add(l);
  return () => {
    viewListeners.delete(l);
  };
}
function readView(): ViewId {
  let stored: string | null = viewFallback;
  try {
    stored = window.localStorage.getItem(VIEW_KEY) ?? viewFallback;
  } catch {}
  if (VIEWS.some((v) => v.id === stored)) return stored as ViewId;
  return "list";
}
function storeView(id: ViewId) {
  viewFallback = id;
  try {
    window.localStorage.setItem(VIEW_KEY, id);
  } catch {}
  viewListeners.forEach((l) => l());
}

export default function EntitiesPage() {
  const view = useSyncExternalStore(subscribeView, readView, () => "list" as ViewId);
  const setView = storeView;
  const qc = useQueryClient();
  const fetching = useIsFetching({ queryKey: entityKeys.all }) > 0;
  const current = VIEWS.find((v) => v.id === view)!;

  // Same header in both views, so switching tabs never moves it.
  const header = (
    <div className="w-full">
      <PageHeader
        icon={Users}
        title="People & topics"
        intro={current.blurb}
        actions={
          <>
            <LiveStatusBadge />
            <button
              className="btn btn-ghost btn-sm btn-icon"
              aria-label="Refresh people and topics"
              title="Refresh"
              onClick={() => void qc.invalidateQueries({ queryKey: entityKeys.all })}
            >
              <RefreshCw size={16} strokeWidth={1.75} className={fetching ? "animate-spin" : undefined} aria-hidden />
            </button>
          </>
        }
      >
        <div className="tabs mt-3" role="tablist" aria-label="View">
          {VIEWS.map((v) => (
            <button key={v.id} role="tab" aria-selected={view === v.id} className="tab" onClick={() => setView(v.id)}>
              {v.label}
            </button>
          ))}
        </div>
      </PageHeader>
    </div>
  );

  return (
    <div className="relative -mx-4 -my-6 sm:-mx-8 lg:-mx-12 lg:-my-[38px] h-[calc(100dvh-3.5rem)] md:h-dvh">
      {view !== "cloud" ? (
        <EntityGraph header={header} mode={view === "list" ? "list" : "graph"} />
      ) : (
        <div className="absolute inset-0 px-4 py-6 sm:px-8 lg:px-12 lg:py-[38px] flex flex-col gap-4 overflow-y-auto">
          {header}
          <VectorCloud />
        </div>
      )}
    </div>
  );
}

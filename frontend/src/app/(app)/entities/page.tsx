"use client";

import { useSyncExternalStore } from "react";
import { Users } from "lucide-react";
import EntityGraph from "@/components/EntityGraph";
import LiveStatusBadge from "@/components/live/LiveStatusBadge";
import VectorCloud from "@/components/VectorCloud";
import PageHeader from "@/components/ui/PageHeader";

// `id` is what this browser remembers; `param` is the `?view=` in the URL, so a view can be linked.
const VIEWS = [
  { id: "list", param: "list", label: "List", blurb: "Everyone and everything your AI apps remember, newest first." },
  { id: "graph", param: "connections", label: "Connections", blurb: "People, organisations, places and projects, and how they connect." },
  { id: "cloud", param: "map", label: "Memory map", blurb: "Every saved memory, placed so that similar memories sit close together." },
] as const;
type ViewId = (typeof VIEWS)[number]["id"];

// `?view=` in the URL wins; otherwise the view last picked here, remembered in this browser; otherwise
// List, the first tab.
const VIEW_KEY = "eunomia.entities.view";
const viewListeners = new Set<() => void>();
let viewFallback: ViewId | null = null; // when storage is blocked, remembered for this visit only
function subscribeView(l: () => void) {
  viewListeners.add(l);
  window.addEventListener("popstate", l); // Back/Forward can change `?view=`
  return () => {
    viewListeners.delete(l);
    window.removeEventListener("popstate", l);
  };
}
function readView(): ViewId {
  const linked = VIEWS.find((v) => v.param === new URLSearchParams(window.location.search).get("view"));
  if (linked) return linked.id;
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
  // keeps `id` and the rest of the query; replace, not push, so Back leaves the page
  const url = new URL(window.location.href);
  url.searchParams.set("view", VIEWS.find((v) => v.id === id)!.param);
  window.history.replaceState(null, "", url.pathname + url.search);
  viewListeners.forEach((l) => l());
}

export default function EntitiesPage() {
  const view = useSyncExternalStore(subscribeView, readView, () => "list" as ViewId);
  const setView = storeView;
  const current = VIEWS.find((v) => v.id === view)!;

  // Same header in both views, so switching tabs never moves it.
  const header = (
    <div className="w-full">
      <PageHeader
        icon={Users}
        title="People & topics"
        intro={current.blurb}
        actions={
          // no Refresh button: the data refreshes on its own; the badge shows only when offline
          <LiveStatusBadge />
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

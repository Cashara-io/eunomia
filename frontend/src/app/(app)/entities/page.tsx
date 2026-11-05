"use client";

import { useState } from "react";
import { useIsFetching, useQueryClient } from "@tanstack/react-query";
import { RefreshCw } from "lucide-react";
import { entityKeys } from "@/lib/queries/entities";
import EntityGraph from "@/components/EntityGraph";
import VectorCloud from "@/components/VectorCloud";

const VIEWS = [
  { id: "graph", label: "Graph", blurb: "People, organisations and places, and how they relate." },
  { id: "cloud", label: "Vector cloud", blurb: "Every memory, projected from vector space to 3D." },
] as const;

export default function EntitiesPage() {
  const [view, setView] = useState<(typeof VIEWS)[number]["id"]>("graph");
  const qc = useQueryClient();
  const fetching = useIsFetching({ queryKey: entityKeys.all }) > 0;
  const current = VIEWS.find((v) => v.id === view)!;

  // Same header in both views, so switching tabs never moves it.
  const header = (
    <div className="flex flex-col gap-3 items-start">
      <div className="flex items-center gap-3 flex-wrap">
        <h1 className="page-title">Entities</h1>
        <div className="panel p-0.5 flex gap-0.5" role="tablist" aria-label="View">
          {VIEWS.map((v) => (
            <button key={v.id} role="tab" aria-selected={view === v.id} className="pill" onClick={() => setView(v.id)}>
              {v.label}
            </button>
          ))}
        </div>
        <button
          className="btn"
          aria-label="Refresh entities"
          title="Refresh"
          onClick={() => void qc.invalidateQueries({ queryKey: entityKeys.all })}
        >
          <RefreshCw size={14} strokeWidth={1.75} className={fetching ? "animate-spin" : undefined} />
        </button>
      </div>
      <p className="text-[13px] -mt-1.5" style={{ color: "var(--ink-dim)" }}>
        {current.blurb}
      </p>
    </div>
  );

  return (
    <div className="relative -mx-4 -my-6 md:-mx-10 md:-my-8 h-[calc(100dvh-3rem)] md:h-dvh">
      {view === "graph" ? (
        <EntityGraph header={header} />
      ) : (
        <div className="absolute inset-0 px-4 py-6 md:px-10 md:py-8 flex flex-col gap-4 overflow-y-auto">
          {header}
          <VectorCloud />
        </div>
      )}
    </div>
  );
}

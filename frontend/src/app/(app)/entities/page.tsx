"use client";

import { useState } from "react";
import { useIsFetching, useQueryClient } from "@tanstack/react-query";
import { RefreshCw, Users } from "lucide-react";
import { entityKeys } from "@/lib/queries/entities";
import EntityGraph from "@/components/EntityGraph";
import LiveStatusBadge from "@/components/live/LiveStatusBadge";
import VectorCloud from "@/components/VectorCloud";
import PageHeader from "@/components/ui/PageHeader";

const VIEWS = [
  { id: "graph", label: "Connections", blurb: "People, organisations, places and projects, and how they connect." },
  { id: "cloud", label: "Memory map", blurb: "Every saved memory, placed so that similar memories sit close together." },
] as const;

export default function EntitiesPage() {
  const [view, setView] = useState<(typeof VIEWS)[number]["id"]>("graph");
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
      {view === "graph" ? (
        <EntityGraph header={header} />
      ) : (
        <div className="absolute inset-0 px-4 py-6 sm:px-8 lg:px-12 lg:py-[38px] flex flex-col gap-4 overflow-y-auto">
          {header}
          <VectorCloud />
        </div>
      )}
    </div>
  );
}

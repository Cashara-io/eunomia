"use client";

import { useMemo, useState } from "react";
import { XIcon } from "@phosphor-icons/react/ssr";
import ErrorLine, { failure } from "./ErrorLine";
import MemoryScatter from "./graph/MemoryScatter";
import MemoryRows, { MAX_COMPARE } from "./MemoryRows";
import { pointKindLabel } from "@/lib/memoryKinds";
import VectorInsights from "./VectorInsights";
import { useEntityCloud } from "@/lib/queries/entities";
import { useVaults } from "@/lib/queries/vaults";
import { useMapSpaces } from "./graph/space";
import { spaceColor, spaceName } from "./graph/SpaceSelect";

const PANELS = [
  { id: "rows", label: "Memories" },
  { id: "insights", label: "Insights" },
] as const;
export { pointKindLabel };

export default function VectorCloud() {
  const vaultsQuery = useVaults();
  const vaults = vaultsQuery.data ?? [];
  // The map lays one or more spaces over each other; it always includes the space the List and
  // Connections views show (graph/space.ts, useMapSpaces).
  const layers = useMapSpaces();
  const pickedKey = layers.ready ? layers.ids.join(",") : "";
  const picked = useMemo(() => (pickedKey ? pickedKey.split(",") : []), [pickedKey]);
  const cloudQuery = useEntityCloud(picked);
  const data = cloudQuery.data ?? null;
  const error = vaultsQuery.isError
    ? failure(vaultsQuery.error, "Could not load your memory spaces.", " Reload the page to try again.")
    : cloudQuery.isError
      ? failure(cloudQuery.error, "Could not load the memory map.", " Reload the page to try again.")
      : null;
  const [selectedId, setSelectedId] = useState<string | null>(null);

  // each point wears its space's colour, the same one Memory spaces shows beside the space's name
  const colorOf = (vaultId: string) => spaceColor(vaults.find((v) => v.id === vaultId));
  const points = useMemo(
    () => (picked.length ? (data?.points ?? []).filter((p) => picked.includes(p.vault)) : []).map((p) => ({ ...p, color: colorOf(p.vault) })),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [data, picked, vaults]
  );
  const selected = points.find((p) => p.id === selectedId);
  const nameOf = (id: string) => {
    const v = vaults.find((x) => x.id === id);
    return v ? spaceName(v) : id;
  };

  // Rows ticked for comparison, in tick order (these are the numbers drawn on the points).
  const [rawSelection, setSelection] = useState<string[]>([]);
  const [tab, setTab] = useState<(typeof PANELS)[number]["id"]>("rows");
  const selection = useMemo(() => {
    const live = new Set(points.map((p) => p.id));
    return rawSelection.filter((id) => live.has(id));
  }, [rawSelection, points]);
  function toggleRow(id: string) {
    setSelection(selection.includes(id) ? selection.filter((x) => x !== id) : selection.length < MAX_COMPARE ? [...selection, id] : selection);
    setSelectedId(id);
  }
  // A click on a point in the graph ticks (or unticks) its row.
  function pickPoint(id: string) {
    if (selection.includes(id)) {
      setSelection(selection.filter((x) => x !== id));
      setSelectedId(null);
    } else toggleRow(id);
  }

  function toggleSpace(id: string) {
    setSelectedId(null);
    setSelection([]);
    layers.toggle(id);
  }

  return (
    <div className="flex flex-col gap-3 flex-1 min-h-0">
      <div className="flex items-center gap-x-4 gap-y-2 flex-wrap">
        {/* one chip per space: toggles it on the map, and doubles as the legend (its colour, its count) */}
        {vaults.length > 1 && (
          <div className="flex gap-2 flex-wrap items-center" role="group" aria-label="Spaces on the map">
            {vaults.map((v) => {
              const on = picked.includes(v.id);
              const only = on && picked.length === 1;
              const count = on && data ? points.filter((p) => p.vault === v.id).length : null;
              return (
                <button
                  key={v.id}
                  type="button"
                  className="pill min-h-10"
                  aria-pressed={on}
                  aria-disabled={only || undefined}
                  title={only ? "The map always shows at least one space" : on ? `Hide ${spaceName(v)}` : `Show ${spaceName(v)} too`}
                  onClick={() => toggleSpace(v.id)}
                >
                  <span className="dot" style={{ background: on ? spaceColor(v) : "var(--border-strong)" }} aria-hidden />
                  {spaceName(v)}
                  {count !== null && (
                    <span className="tabular-nums" style={{ color: "var(--ink-dim)" }}>
                      {count}
                    </span>
                  )}
                </button>
              );
            })}
          </div>
        )}
        {data && picked.length > 0 && (
          <span className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
            {points.length === 1 ? "1 memory" : `${points.length} memories`}
            {" · "}
            <span
              title={
                data.space === "semantic"
                  ? "Placed by an AI model: nearby points mean similar meaning."
                  : "No AI model set up, so points are placed by shared words. Add one in Settings to place them by meaning."
              }
            >
              {data.space === "semantic" ? "placed by meaning" : "placed by shared words"}
            </span>
            {/* the page's tab blurb already says what the map is; this line only adds how to use it */}
            {points.length > 1 && " · click points to compare them"}
          </span>
        )}
      </div>

      <div className="flex flex-col lg:flex-row gap-3 flex-1 min-h-0">
      <div className="ledger relative flex-1 min-h-0 overflow-hidden min-w-0" style={{ minHeight: 420 }}>
        {error ? (
          <div className="m-4">
            <ErrorLine error={error} />
          </div>
        ) : !data ? (
          <div className="absolute inset-4 skeleton" aria-busy="true" aria-label="Loading the memory map" />
        ) : points.length === 0 ? (
          <div className="p-10 text-center text-[13px]" style={{ color: "var(--ink-dim)" }}>
            {picked.length > 1 ? "Nothing saved in these memory spaces yet." : "Nothing saved in this memory space yet."}
          </div>
        ) : (
          <MemoryScatter points={points} selectedId={selectedId} highlightIds={selection} onSelect={pickPoint} ariaLabel="Memory map" />
        )}
        {selected && (
          <aside
            key={selected.id}
            aria-label="Selected point"
            className="panel pop-in absolute top-3 right-3 left-3 md:left-auto md:w-72 p-4 flex flex-col gap-2"
            style={{ transformOrigin: "top right" }}
          >
            <div className="flex items-center gap-2 label">
              <span className="dot" style={{ background: selected.color }} aria-hidden />
              <span className="truncate">
                {nameOf(selected.vault)} · {pointKindLabel(selected.kind)}
              </span>
              <button
                type="button"
                className="btn btn-ghost btn-sm btn-icon ml-auto"
                aria-label="Close"
                onClick={() => setSelectedId(null)}
              >
                <XIcon weight="duotone" size={16} />
              </button>
            </div>
            <p className="text-[14px] leading-snug" style={{ color: "var(--ink)" }}>{selected.label}</p>
          </aside>
        )}
      </div>
      {data && points.length > 0 && !error && (
        <aside className="ledger max-lg:shrink-0 lg:w-[340px] lg:shrink-0 lg:min-h-0 flex flex-col overflow-hidden" aria-label="Analysis">
          <div className="p-2 pb-0 shrink-0">
            <div className="tabs px-2" role="tablist" aria-label="Analysis view">
              {PANELS.map((t) => (
                <button key={t.id} role="tab" aria-selected={tab === t.id} className="tab" onClick={() => setTab(t.id)}>
                  {t.label}
                  {t.id === "rows" && selection.length > 0 && <span className="tabular-nums">{selection.length}</span>}
                </button>
              ))}
            </div>
          </div>
          {tab === "rows" ? (
            <MemoryRows
              points={points}
              vaultIds={picked}
              nameOf={nameOf}
              selection={selection}
              focusId={selectedId}
              onToggle={toggleRow}
              onClear={() => setSelection([])}
            />
          ) : (
            <div className="lg:overflow-y-auto lg:min-h-0">
              <VectorInsights
                points={points}
                vaultOrder={picked}
                nameOf={nameOf}
                colorOf={colorOf}
                selectedId={selectedId}
                onSelect={setSelectedId}
                space={data.space}
              />
            </div>
          )}
        </aside>
      )}
      </div>
    </div>
  );
}

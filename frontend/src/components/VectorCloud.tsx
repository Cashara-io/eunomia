"use client";

import { useMemo, useState } from "react";
import { X } from "lucide-react";
import ErrorLine, { failure } from "./ErrorLine";
import Scene3D from "./Scene3D";
import VectorInsights from "./VectorInsights";
import { useEntityCloud } from "@/lib/queries/entities";
import { useVaults } from "@/lib/queries/vaults";

// One colour per layered vault, in pick order.
// Series colours, never the accent: the accent marks only what is pressable or selected.
const LAYER_COLORS = ["var(--series-2)", "var(--series-1)", "var(--series-3)", "var(--series-4)", "var(--kind-repository)", "var(--kind-location)"];
const AXES: [string, string, string] = ["PC1", "PC2", "PC3"];

export default function VectorCloud() {
  const vaultsQuery = useVaults();
  const vaults = vaultsQuery.data ?? [];
  // Until the user toggles a layer, the personal vault (or the first one) is picked.
  const [userPicked, setPicked] = useState<string[] | null>(null);
  const defaultVault = vaults.find((v) => v.kind === "personal") ?? vaults[0];
  const picked = userPicked ?? (defaultVault ? [defaultVault.id] : []);
  const cloudQuery = useEntityCloud(picked);
  const data = cloudQuery.data ?? null;
  const error = vaultsQuery.isError
    ? failure(vaultsQuery.error, "Could not load your vaults.", " Reload the page to try again.")
    : cloudQuery.isError
      ? failure(cloudQuery.error, "Could not load the vector cloud.", " Reload the page to try again.")
      : null;
  const [selectedId, setSelectedId] = useState<string | null>(null);

  const colorOf = (vaultId: string) => LAYER_COLORS[Math.max(0, picked.indexOf(vaultId)) % LAYER_COLORS.length];
  const points = useMemo(
    () => (picked.length ? data?.points ?? [] : []).map((p) => ({ ...p, color: colorOf(p.vault) })),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [data]
  );
  const selected = points.find((p) => p.id === selectedId);
  const nameOf = (id: string) => {
    const v = vaults.find((x) => x.id === id);
    return v ? (v.kind === "personal" ? "Personal" : v.name) : id;
  };

  function toggle(id: string) {
    setSelectedId(null);
    setPicked(picked.includes(id) ? picked.filter((x) => x !== id) : [...picked, id]);
  }

  return (
    <div className="flex flex-col gap-3 flex-1 min-h-0">
      <div className="flex items-center justify-between gap-2 flex-wrap">
        <div className="flex gap-2 flex-wrap items-center" role="group" aria-label="Vault layers">
          {vaults.map((v) => {
            const on = picked.includes(v.id);
            const count = on ? points.filter((p) => p.vault === v.id).length : null;
            return (
              <button key={v.id} type="button" className="pill" aria-pressed={on} onClick={() => toggle(v.id)}>
                <span className="dot" style={{ background: on ? colorOf(v.id) : "var(--border-strong)" }} aria-hidden />
                {nameOf(v.id)}
                {count !== null && <span className="font-mono" style={{ color: "var(--ink-faint)" }}>{count}</span>}
              </button>
            );
          })}
        </div>
        {data && picked.length > 0 && (
          <span
            className="label"
            style={{ color: "var(--ink-faint)" }}
            title={
              data.space === "semantic"
                ? "Model embeddings: nearby points mean similar meaning."
                : "No model key: points are placed by shared words. Add a key in Settings for a semantic space."
            }
          >
            {data.space === "semantic" ? "Semantic space" : "Lexical space"}, PCA to 3D
          </span>
        )}
      </div>
      <p className="text-[12px] -mt-1" style={{ color: "var(--ink-faint)" }}>
        Each point is a memory or synced record. Layer vaults into one shared space, drag to orbit, click a point to read it.
      </p>

      <div className="flex flex-col lg:flex-row gap-3 flex-1 min-h-0">
      <div className="ledger relative flex-1 min-h-0 overflow-hidden min-w-0" style={{ minHeight: 420 }}>
        {error ? (
          <div className="m-4">
            <ErrorLine error={error} />
          </div>
        ) : picked.length === 0 ? (
          <div className="p-10 text-center text-[13px]" style={{ color: "var(--ink-dim)" }}>
            Pick at least one vault above to plot its memories.
          </div>
        ) : !data ? (
          <div className="absolute inset-4 skeleton" aria-busy="true" aria-label="Loading the vector cloud" />
        ) : points.length === 0 ? (
          <div className="p-10 text-center text-[13px]" style={{ color: "var(--ink-dim)" }}>
            Nothing stored in these vaults yet.
          </div>
        ) : (
          <Scene3D points={points} axes={AXES} selectedId={selectedId} onSelect={setSelectedId} ariaLabel="Vector cloud" />
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
                {nameOf(selected.vault)} · {selected.kind}
              </span>
              <button
                type="button"
                className="btn btn-ghost btn-sm ml-auto"
                style={{ width: 22, height: 22, padding: 0 }}
                aria-label="Close"
                onClick={() => setSelectedId(null)}
              >
                <X size={13} strokeWidth={1.75} />
              </button>
            </div>
            <p className="text-[13px] leading-snug" style={{ color: "var(--ink)" }}>{selected.label}</p>
          </aside>
        )}
      </div>
      {data && points.length > 0 && !error && (
        <aside
          className="ledger lg:w-[340px] lg:shrink-0 lg:overflow-y-auto lg:min-h-0"
          aria-label="Insights"
        >
          <VectorInsights
            points={points}
            vaultOrder={picked}
            nameOf={nameOf}
            colorOf={colorOf}
            selectedId={selectedId}
            onSelect={setSelectedId}
            space={data.space}
          />
        </aside>
      )}
      </div>
    </div>
  );
}

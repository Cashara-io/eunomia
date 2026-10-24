"use client";

import { useEffect, useMemo, useRef, useState } from "react";
import { Search, X } from "lucide-react";
import ErrorLine, { failure } from "./ErrorLine";
import { useCloudSimilarity } from "@/lib/queries/entities";
import type { InsightPoint } from "@/lib/vectorInsights";

export const MAX_COMPARE = 10;

type Row = InsightPoint & { color: string };

type Props = {
  points: Row[];
  vaultIds: string[];
  nameOf: (vaultId: string) => string;
  /** ids of the compared rows, in selection order (the numbers in the graph) */
  selection: string[];
  focusId: string | null;
  onToggle: (id: string) => void;
  onClear: () => void;
};

const fmt = (n: number, d = 2) => (Number.isFinite(n) ? n.toFixed(d) : "-");
const MODES = [
  { id: "cosine", label: "Cosine", hint: "1 = same direction, 0 = unrelated" },
  { id: "distance", label: "Distance", hint: "euclidean distance between the full vectors; smaller is closer" },
  { id: "correlation", label: "Pearson", hint: "correlation of the vectors' components, -1 to 1" },
  { id: "projected", label: "3D", hint: "euclidean distance in the 3D picture only" },
] as const;
type Mode = (typeof MODES)[number]["id"];

export default function MemoryRows({ points, vaultIds, nameOf, selection, focusId, onToggle, onClear }: Props) {
  const [filter, setFilter] = useState("");
  const [mode, setMode] = useState<Mode>("cosine");
  const rowRefs = useRef(new Map<string, HTMLElement>());
  const rowsBox = useRef<HTMLDivElement>(null);
  const q = filter.trim().toLowerCase();
  const rows = useMemo(() => (q ? points.filter((p) => p.label.toLowerCase().includes(q) || p.kind.includes(q)) : points), [points, q]);
  const byId = useMemo(() => new Map(points.map((p) => [p.id, p])), [points]);
  const numberOf = (id: string) => selection.indexOf(id) + 1;
  const full = selection.length >= MAX_COMPARE;

  // A point picked in the graph brings its row into view.
  useEffect(() => {
    const el = focusId ? rowRefs.current.get(focusId) : undefined;
    const box = rowsBox.current;
    if (!el || !box) return;
    const r = el.getBoundingClientRect();
    const b = box.getBoundingClientRect();
    if (r.top < b.top || r.bottom > b.bottom) box.scrollTop += r.top - b.top - (b.height - r.height) / 2;
  }, [focusId]);

  const sim = useCloudSimilarity(vaultIds, selection);
  const n = selection.length;
  // Matrices follow the server's id order, which is the selection order we sent.
  const ready = sim.data && sim.data.ids.length === n && sim.data.ids.every((id, i) => id === selection[i]);
  const matrix = useMemo(() => {
    if (n < 2) return null;
    if (mode === "projected") {
      const ps = selection.map((id) => byId.get(id)).filter((p): p is Row => !!p);
      return ps.map((a) => ps.map((b) => Math.hypot(a.x - b.x, a.y - b.y, a.z - b.z)));
    }
    return ready && sim.data ? sim.data[mode] : null;
  }, [n, mode, selection, byId, ready, sim.data]);
  const projected = useMemo(() => {
    const ps = selection.map((id) => byId.get(id)).filter((p): p is Row => !!p);
    return ps.map((a) => ps.map((b) => Math.hypot(a.x - b.x, a.y - b.y, a.z - b.z)));
  }, [selection, byId]);

  const pairs = useMemo(() => {
    const out: { i: number; j: number; cos: number }[] = [];
    if (ready && sim.data) for (let i = 0; i < n; i++) for (let j = i + 1; j < n; j++) out.push({ i, j, cos: sim.data.cosine[i][j] });
    return out.sort((a, b) => b.cos - a.cos);
  }, [ready, sim.data, n]);
  const meanCos = pairs.length ? pairs.reduce((s, p) => s + p.cos, 0) / pairs.length : null;

  const isDistance = mode === "distance" || mode === "projected";
  const maxOff = matrix ? Math.max(1e-9, ...matrix.flatMap((r, i) => r.filter((_, j) => i !== j))) : 1;
  // 0..1 strength: closer / more similar is stronger
  const strength = (v: number) => (isDistance ? 1 - Math.min(1, v / maxOff) : Math.max(0, Math.min(1, mode === "cosine" ? v : (v + 1) / 2)));

  const label = (id: string) => byId.get(id)?.label ?? id;
  const pairLine = (i: number, j: number) => `${i + 1} and ${j + 1}`;

  return (
    <div className="flex flex-col min-h-0 flex-1">
      <div className="p-3 pb-2 flex flex-col gap-2">
        <div className="field flex items-center gap-2 h-8 px-2.5">
          <Search size={13} strokeWidth={1.75} style={{ color: "var(--ink-faint)" }} aria-hidden />
          <input
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder="Filter memories"
            aria-label="Filter memories"
            className="bg-transparent outline-none text-[13px] flex-1 min-w-0"
          />
        </div>
        <div className="flex items-center justify-between gap-2 min-h-[22px]">
          <span className="label">
            {n === 0 ? "Select two or more rows to compare them." : full && n === MAX_COMPARE ? `${n} selected (the most you can compare)` : `${n} selected`}
          </span>
          {n > 0 && (
            <button type="button" className="btn btn-ghost btn-sm" style={{ height: 22, padding: "0 6px" }} onClick={onClear}>
              Clear
            </button>
          )}
        </div>
      </div>

      <div ref={rowsBox} className="hairline-rows overflow-y-auto min-h-0 flex-1 max-h-[340px] lg:max-h-none" role="group" aria-label="Memories" style={{ borderTop: "var(--hair) solid var(--border)" }}>
        {rows.length === 0 && (
          <p className="p-4 text-[13px]" style={{ color: "var(--ink-dim)" }}>
            No memories match.
          </p>
        )}
        {rows.map((p) => {
          const num = numberOf(p.id);
          const on = num > 0;
          const blocked = !on && full;
          return (
            <button
              key={p.id}
              ref={(el) => {
                if (el) rowRefs.current.set(p.id, el);
                else rowRefs.current.delete(p.id);
              }}
              type="button"
              role="checkbox"
              aria-checked={on}
              aria-disabled={blocked}
              title={blocked ? `Compare up to ${MAX_COMPARE} at a time` : undefined}
              onClick={() => !blocked && onToggle(p.id)}
              className="flex items-start gap-2.5 w-full text-left px-3 py-2 hover:bg-[var(--surface-hover)]"
              style={{
                background: on ? "var(--accent-soft)" : undefined,
                opacity: blocked ? 0.5 : 1,
                boxShadow: focusId === p.id ? "inset 2px 0 0 var(--accent)" : undefined,
              }}
            >
              <span
                className="shrink-0 mt-px inline-flex items-center justify-center rounded-[4px] font-mono text-[10px] font-semibold"
                style={{
                  width: 18,
                  height: 18,
                  border: `var(--hair) solid ${on ? "var(--accent)" : "var(--border-strong)"}`,
                  background: on ? "var(--accent)" : "transparent",
                  color: "var(--on-accent)",
                }}
                aria-hidden
              >
                {on ? num : ""}
              </span>
              <span className="min-w-0 flex-1 flex flex-col gap-0.5">
                <span className="text-[12.5px] leading-snug line-clamp-2" style={{ color: "var(--ink)" }}>
                  {p.label}
                </span>
                <span className="label flex items-center gap-1.5 min-w-0">
                  <span className="dot shrink-0" style={{ background: p.color }} aria-hidden />
                  <span className="truncate">
                    {vaultIds.length > 1 ? `${nameOf(p.vault)} · ` : ""}
                    {p.kind}
                  </span>
                </span>
              </span>
            </button>
          );
        })}
      </div>

      {n >= 2 && (
        <section
          className="shrink-0 flex flex-col gap-3 p-3 lg:max-h-[58%] lg:overflow-y-auto"
          style={{ borderTop: "var(--hair) solid var(--border-strong)" }}
          aria-label="Comparison"
        >
          <div className="flex items-baseline justify-between gap-2">
            <h3 className="text-[13px] font-semibold" style={{ color: "var(--ink)" }}>
              Comparing {n}
            </h3>
            {sim.data && <span className="label">{sim.data.space === "semantic" ? "Full embedding vectors" : "Word-hash vectors"}</span>}
          </div>

          {sim.isError ? (
            <ErrorLine error={failure(sim.error, "Could not compare these memories.", " Try again.")} />
          ) : (
            <>
              <dl className="grid grid-cols-3 gap-2">
                {[
                  [n === 2 ? "Cosine" : "Mean cosine", ready && n === 2 ? fmt(sim.data!.cosine[0][1]) : meanCos !== null ? fmt(meanCos) : "-"],
                  [n === 2 ? "Pearson" : "Mean Pearson", ready ? fmt(n === 2 ? sim.data!.correlation[0][1] : avgOff(sim.data!.correlation)) : "-"],
                  [n === 2 ? "3D distance" : "Mean 3D gap", n === 2 ? fmt(projected[0]?.[1] ?? NaN) : fmt(avgOff(projected))],
                ].map(([l, v]) => (
                  <div key={l} className="flex flex-col">
                    <dd className="font-mono text-[17px] leading-tight order-1" style={{ color: "var(--ink)" }}>
                      {v}
                    </dd>
                    <dt className="label order-2">{l}</dt>
                  </div>
                ))}
              </dl>
              {n === 2 && ready && (
                <div className="flex flex-col gap-1">
                  <div className="h-1.5 rounded-full overflow-hidden" style={{ background: "var(--surface-raised)" }} role="img" aria-label={`Cosine similarity ${fmt(sim.data!.cosine[0][1])}`}>
                    <div className="h-full rounded-full" style={{ width: `${Math.max(0, sim.data!.cosine[0][1]) * 100}%`, background: "var(--accent)" }} />
                  </div>
                  <p className="label">
                    Embedding distance <span className="font-mono">{fmt(sim.data!.distance[0][1], 3)}</span>; the 3D distance only measures the picture.
                  </p>
                </div>
              )}
              {n > 2 && pairs.length > 0 && (
                <ul className="flex flex-col gap-1 text-[12.5px]">
                  <li className="flex justify-between gap-2">
                    <span style={{ color: "var(--ink-dim)" }}>Closest: {pairLine(pairs[0].i, pairs[0].j)}</span>
                    <span className="font-mono">{fmt(pairs[0].cos)}</span>
                  </li>
                  <li className="flex justify-between gap-2">
                    <span style={{ color: "var(--ink-dim)" }}>Furthest: {pairLine(pairs[pairs.length - 1].i, pairs[pairs.length - 1].j)}</span>
                    <span className="font-mono">{fmt(pairs[pairs.length - 1].cos)}</span>
                  </li>
                </ul>
              )}

              <div className="flex gap-1 flex-wrap" role="group" aria-label="Matrix measure">
                {MODES.map((m) => (
                  <button key={m.id} type="button" className="pill" aria-pressed={mode === m.id} title={m.hint} onClick={() => setMode(m.id)}>
                    {m.label}
                  </button>
                ))}
              </div>
              <div
                className="grid gap-px text-[11px] font-mono"
                style={{ gridTemplateColumns: `18px repeat(${n}, minmax(0, 1fr))` }}
                role="table"
                aria-label={`${MODES.find((m) => m.id === mode)!.label} matrix`}
              >
                <span />
                {selection.map((id, i) => (
                  <span key={id} className="text-center" style={{ color: "var(--ink-faint)" }} role="columnheader" title={label(id)}>
                    {i + 1}
                  </span>
                ))}
                {selection.map((rid, i) => (
                  <MatrixRow key={rid} i={i} n={n} row={matrix?.[i]} id={rid} label={label(rid)} strength={strength} />
                ))}
              </div>
              <p className="label leading-snug">{MODES.find((m) => m.id === mode)!.hint}. Cell shade: stronger means closer.</p>

              <ol className="flex flex-col gap-1">
                {selection.map((id, i) => (
                  <li key={id} className="flex items-start gap-2 text-[12px]">
                    <span className="font-mono shrink-0 w-4 text-right" style={{ color: "var(--accent)" }}>
                      {i + 1}
                    </span>
                    <span className="flex-1 min-w-0 line-clamp-1" style={{ color: "var(--ink-dim)" }}>
                      {label(id)}
                    </span>
                    <button type="button" className="shrink-0 mt-0.5" aria-label={`Remove ${label(id)} from the comparison`} onClick={() => onToggle(id)} style={{ color: "var(--ink-faint)" }}>
                      <X size={12} strokeWidth={1.75} />
                    </button>
                  </li>
                ))}
              </ol>
            </>
          )}
        </section>
      )}
    </div>
  );
}

function avgOff(m: number[][]) {
  let s = 0;
  let c = 0;
  m.forEach((r, i) => r.forEach((v, j) => i !== j && ((s += v), c++)));
  return c ? s / c : NaN;
}

function MatrixRow({ i, n, row, id, label, strength }: { i: number; n: number; row?: number[]; id: string; label: string; strength: (v: number) => number }) {
  return (
    <>
      <span className="flex items-center justify-end pr-1" style={{ color: "var(--ink-faint)" }} role="rowheader" title={label}>
        {i + 1}
      </span>
      {Array.from({ length: n }, (_, j) => {
        const v = row?.[j];
        const diag = i === j;
        return (
          <span
            key={id + j}
            role="cell"
            className="flex items-center justify-center h-6 rounded-[3px]"
            title={v === undefined ? undefined : `${i + 1} and ${j + 1}: ${fmt(v, 3)}`}
            style={{
              background: diag || v === undefined ? "var(--surface-raised)" : `color-mix(in srgb, var(--accent) ${Math.round(strength(v) * 55)}%, var(--surface-raised))`,
              color: diag ? "var(--ink-faint)" : "var(--ink)",
            }}
          >
            {v === undefined ? "" : diag ? "–" : fmt(v, n > 6 ? 1 : 2)}
          </span>
        );
      })}
    </>
  );
}

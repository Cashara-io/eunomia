"use client";

import { useMemo } from "react";
import type { InsightPoint } from "@/lib/vectorInsights";
import { analyse } from "@/lib/vectorInsights";

type Props = {
  points: InsightPoint[];
  vaultOrder: string[];
  nameOf: (vaultId: string) => string;
  colorOf: (vaultId: string) => string;
  selectedId: string | null;
  onSelect: (id: string) => void;
  space: "semantic" | "lexical";
};

const fmt = (n: number) => n.toFixed(2);
const pct = (n: number) => `${Math.round(n * 100)}%`;
const OPACITY = [1, 0.62, 0.4, 0.26];

function Section({ title, hint, children }: { title: string; hint?: string; children: React.ReactNode }) {
  return (
    <section className="flex flex-col gap-2 py-3">
      <div className="flex items-baseline justify-between gap-2">
        <h3 className="text-[13px] font-semibold" style={{ color: "var(--ink)" }}>
          {title}
        </h3>
        {hint && <span className="label">{hint}</span>}
      </div>
      {children}
    </section>
  );
}

// A row that selects a point in the cloud.
function PointButton({ p, color, active, onSelect, meta }: { p: InsightPoint; color: string; active: boolean; onSelect: (id: string) => void; meta?: string }) {
  return (
    <button
      type="button"
      onClick={() => onSelect(p.id)}
      aria-pressed={active}
      className="flex items-start gap-2 text-left w-full rounded-md px-1.5 py-1 -mx-1.5 hover:bg-[var(--surface-hover)]"
      style={active ? { background: "var(--surface-hover)" } : undefined}
    >
      <span className="dot mt-[5px] shrink-0" style={{ background: color }} aria-hidden />
      <span className="text-[12.5px] leading-snug min-w-0 flex-1 line-clamp-2" style={{ color: "var(--ink)" }}>
        {p.label}
      </span>
      {meta && (
        <span className="font-mono text-[11.5px] shrink-0 mt-px" style={{ color: "var(--ink-dim)" }}>
          {meta}
        </span>
      )}
    </button>
  );
}

export default function VectorInsights({ points, vaultOrder, nameOf, colorOf, selectedId, onSelect, space }: Props) {
  const orderKey = vaultOrder.join(",");
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const ins = useMemo(() => analyse(points, vaultOrder), [points, orderKey]);
  const colors = (p: InsightPoint) => colorOf(p.vault);

  if (ins.total === 0) return null;

  const dense = ins.clusters[0];
  const typeOf = (kinds: { kind: string; count: number }[]) => kinds.map((k) => `${k.count} ${k.kind}`).join(" · ");

  return (
    <div className="hairline-rows px-4 text-[13px]" aria-label="Vector insights">
      <Section title="Overview">
        <dl className="grid grid-cols-3 gap-2">
          {[
            [String(ins.total), ins.total === 1 ? "point" : "points"],
            [String(ins.vaults.length), ins.vaults.length === 1 ? "vault" : "vaults"],
            [String(ins.clusters.length), ins.clusters.length === 1 ? "group" : "groups"],
          ].map(([v, l]) => (
            <div key={l} className="flex flex-col">
              <dd className="font-mono text-[18px] leading-tight order-1" style={{ color: "var(--ink)" }}>
                {v}
              </dd>
              <dt className="label order-2">{l}</dt>
            </div>
          ))}
        </dl>
        <p className="label leading-snug">
          {ins.total < 2
            ? "Add more memories to see how they relate."
            : `Nearest neighbours sit ${fmt(ins.meanNearest)} apart on average. ${
                space === "lexical" ? "Placed by shared words, not meaning." : "Closer means more similar in meaning."
              }`}
        </p>
      </Section>

      <Section title="By vault" hint="kind mix">
        <ul className="flex flex-col gap-3">
          {ins.vaults.map((v) => (
            <li key={v.vault} className="flex flex-col gap-1.5">
              <div className="flex items-center gap-2">
                <span className="dot shrink-0" style={{ background: colorOf(v.vault) }} aria-hidden />
                <span className="truncate" style={{ color: "var(--ink)" }}>
                  {nameOf(v.vault)}
                </span>
                <span className="font-mono text-[12px] ml-auto" style={{ color: "var(--ink-dim)" }}>
                  {v.count}
                </span>
              </div>
              <div
                className="flex h-1.5 rounded-full overflow-hidden gap-px"
                role="img"
                aria-label={`${nameOf(v.vault)} kinds: ${typeOf(v.kinds)}`}
                style={{ background: "var(--surface-raised)" }}
              >
                {v.kinds.map((k, i) => (
                  <span key={k.kind} style={{ flex: k.count, background: colorOf(v.vault), opacity: OPACITY[Math.min(i, OPACITY.length - 1)] }} />
                ))}
              </div>
              <p className="label font-mono">{typeOf(v.kinds)}</p>
              {v.count > 1 && (
                <p className="label">
                  Spread <span className="font-mono">{fmt(v.spread)}</span> from the vault centre
                </p>
              )}
            </li>
          ))}
        </ul>
      </Section>

      {ins.clusters.length > 0 && (
        <Section title="Dense groups" hint={`${ins.isolated} on their own`}>
          <ul className="flex flex-col gap-1.5">
            {ins.clusters.map((c, i) => (
              <li key={i}>
                <PointButton
                  p={c.representative}
                  color={colors(c.representative)}
                  active={selectedId === c.representative.id}
                  onSelect={onSelect}
                  meta={`${c.size} pts`}
                />
              </li>
            ))}
          </ul>
          {dense && (
            <p className="label leading-snug">
              Each group is memories lying close together; the row shows the one nearest its centre
              {dense.vaults > 1 ? ", and the largest spans several vaults" : ""}.
            </p>
          )}
        </Section>
      )}

      {ins.closestPairs.length > 0 && (
        <Section title="Most similar pairs" hint="distance">
          <ul className="flex flex-col gap-2.5">
            {ins.closestPairs.map((pair) => (
              <li key={pair.a.id + pair.b.id} className="flex flex-col gap-0.5">
                <PointButton p={pair.a} color={colors(pair.a)} active={selectedId === pair.a.id} onSelect={onSelect} meta={fmt(pair.distance)} />
                <PointButton p={pair.b} color={colors(pair.b)} active={selectedId === pair.b.id} onSelect={onSelect} />
              </li>
            ))}
          </ul>
        </Section>
      )}

      {ins.outliers.length > 0 && (
        <Section title="Outliers" hint="vs vault spread">
          <ul className="flex flex-col gap-1.5">
            {ins.outliers.map((o) => (
              <li key={o.point.id}>
                <PointButton p={o.point} color={colors(o.point)} active={selectedId === o.point.id} onSelect={onSelect} meta={`${o.ratio.toFixed(1)}×`} />
              </li>
            ))}
          </ul>
          <p className="label leading-snug">Memories far from the rest of their own vault: likely off-topic, or the only note on a subject.</p>
        </Section>
      )}

      {ins.gaps.length > 0 && (
        <Section title="Vault overlap" hint="centre gap · mixed">
          <ul className="flex flex-col gap-2">
            {ins.gaps.map((g) => (
              <li key={g.a + g.b} className="flex items-center gap-2 min-w-0">
                <span className="dot shrink-0" style={{ background: colorOf(g.a) }} aria-hidden />
                <span className="dot shrink-0 -ml-1" style={{ background: colorOf(g.b) }} aria-hidden />
                <span className="truncate" style={{ color: "var(--ink)" }}>
                  {nameOf(g.a)} / {nameOf(g.b)}
                </span>
                <span className="font-mono text-[11.5px] ml-auto shrink-0" style={{ color: "var(--ink-dim)" }}>
                  {fmt(g.distance)} · {pct(g.mixed)}
                </span>
              </li>
            ))}
          </ul>
          <p className="label leading-snug">Centre gap is the distance between the vaults&apos; centres; mixed is how many points have their closest neighbour in the other vault.</p>
        </Section>
      )}
    </div>
  );
}

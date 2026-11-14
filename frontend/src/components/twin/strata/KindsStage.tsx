"use client";
import { useMemo } from "react";
import { useKindColor } from "@/lib/kindColors";
import type { OntologyOut } from "@/lib/queries/ontology";
import { humanLabel, plural } from "../overview-format";
import { kindPairs, kindRanks, kindRows, stageKinds } from "./strata-model";
import type { Wire } from "./Wires";
import styles from "./strata.module.css";

const MAX_BAND = 3;

/** Everything the Kinds level draws, computed from the link graph: which kinds, in which column, joined by which lines. */
export function useKindsLayout(data: OntologyOut | undefined, selected: string, mobile: boolean) {
  return useMemo(() => {
    if (!data) return null;
    const rows = kindRows(data);
    const every = kindPairs(data.links, new Set(rows.map((r) => r.kind)));
    const focusKind = selected || (mobile ? (rows[0]?.kind ?? "") : "");
    let shown: string[];
    let hidden: number;
    if (mobile) {
      const near = new Set([
        focusKind,
        ...every.flatMap((p) => (p.from === focusKind ? [p.to] : p.to === focusKind ? [p.from] : [])),
      ]);
      shown = rows.filter((r) => near.has(r.kind)).map((r) => r.kind);
      hidden = rows.length - shown.length;
    } else ({ shown, hidden } = stageKinds(rows, every, selected));
    const pairs = kindPairs(data.links, new Set(shown));
    const ranks = kindRanks(shown, pairs);
    const heaviest = Math.max(1, ...pairs.map((p) => p.count));
    const wires: Wire[] = pairs.map((p) => {
      const same = ranks.get(p.from) === ranks.get(p.to);
      const forward = (ranks.get(p.from) ?? 0) < (ranks.get(p.to) ?? 0);
      return {
        id: `band:${p.from}:${p.to}`,
        from: `kind:${p.from}`,
        to: `kind:${p.to}`,
        fromSide: same || forward ? "r" : "l",
        toSide: same ? "r" : forward ? "l" : "r",
        tone: !selected ? "line" : p.from === selected || p.to === selected ? "on" : "off",
        weight: 1 + (p.count / heaviest) * MAX_BAND,
        arrow: true,
        both: p.reverse > 0,
      };
    });
    const columns = [
      ...rows
        .filter((r) => shown.includes(r.kind))
        .reduce((by, row) => {
          const rank = ranks.get(row.kind) ?? 0;
          return by.set(rank, [...(by.get(rank) ?? []), row]);
        }, new Map<number, typeof rows>())
        .entries(),
    ]
      .sort((a, b) => a[0] - b[0])
      .map((c) => c[1]);
    return { rows, shown, hidden, pairs, wires, columns, focusKind };
  }, [data, selected, mobile]);
}
export type KindsLayout = NonNullable<ReturnType<typeof useKindsLayout>>;

export default function KindsStage({
  layout,
  selected,
  query,
  mobile,
  select,
}: {
  layout: KindsLayout;
  selected: string;
  query: string;
  mobile: boolean;
  select: (kind: string) => void;
}) {
  const color = useKindColor();
  const { rows, shown, hidden, pairs, columns, focusKind } = layout;
  const linked = new Set(
    pairs.flatMap((p) => (p.from === selected ? [p.to] : p.to === selected ? [p.from] : [])),
  );
  const needle = query.trim().toLowerCase();
  const heaviest = Math.max(1, ...rows.map((r) => r.entities));
  return (
    <div className={styles.kinds}>
      <p className={styles.planeNote}>
        {mobile
          ? `${humanLabel(focusKind)} and the kinds it links to. Pick another kind in the list above.`
          : `Kinds, laid out by the direction of recorded links. Line weight is link count.${hidden ? ` Showing ${shown.length} of ${rows.length} kinds. The rest are in the list.` : ""}`}
      </p>
      <div className={styles.columns} data-columns={columns.length}>
        {columns.map((column, i) => (
          <div className={styles.column} key={i}>
            {column.map((row) => {
              const dim =
                (!!selected && selected !== row.kind && !linked.has(row.kind)) ||
                (!!needle && !humanLabel(row.kind).toLowerCase().includes(needle));
              return (
                <button
                  key={row.kind}
                  className={styles.tile}
                  data-wire={`kind:${row.kind}`}
                  data-dim={dim}
                  aria-pressed={selected === row.kind}
                  onClick={() => select(row.kind)}
                >
                  <span className={styles.swatch} style={{ background: color(row.kind) }} aria-hidden />
                  <span className={styles.tileName}>{humanLabel(row.kind)}</span>
                  {row.waiting > 0 && <span className={styles.pending}>{row.waiting} in review</span>}
                  <small className="font-mono">
                    {row.entities.toLocaleString("en-AU")} {plural("object", row.entities)}
                  </small>
                  <span
                    className={styles.bar}
                    style={{ width: `${Math.max(6, Math.sqrt(row.entities / heaviest) * 100)}%`, background: color(row.kind) }}
                    aria-hidden
                  />
                </button>
              );
            })}
          </div>
        ))}
      </div>
    </div>
  );
}

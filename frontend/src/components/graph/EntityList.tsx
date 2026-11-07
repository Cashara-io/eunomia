"use client";

// People & topics as a plain, dense table: name and kind, the latest memory, how many memories, and
// when one was last saved. Click a row (or Enter on it) to open the same inspector as the graph.

import { useState } from "react";
import { ArrowDown, ArrowUp } from "lucide-react";
import type { EntityKind, EntitySummary } from "@/lib/types";
import { useEntityDetails } from "@/lib/queries/entities";

type SortKey = "name" | "memory" | "count" | "updated";
type Row = { id: string; name: string; kind: EntityKind; kindWord: string; latest: string | null; count: number | null; updated: string | null };

// The detail read per row is capped, so a very large space still opens quickly.
const DETAIL_CAP = 200;

function ago(iso: string | null): string {
  if (!iso) return "";
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return "";
  const s = Math.max(0, (Date.now() - t) / 1000);
  if (s < 60) return "Just now";
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  if (s < 86400 * 30) return `${Math.floor(s / 86400)}d ago`;
  return new Date(t).toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" });
}

export default function EntityList({
  entities,
  kindLabel,
  kindColor,
  selectedId,
  onSelect,
  total,
}: {
  entities: EntitySummary[];
  kindLabel: (k: EntityKind) => string;
  kindColor: (k: EntityKind) => string;
  selectedId?: string | null;
  onSelect: (id: string) => void;
  /** how many the space holds, when more than are listed */
  total?: number;
}) {
  const [sort, setSort] = useState<{ key: SortKey; desc: boolean }>({ key: "updated", desc: true });
  const details = useEntityDetails(entities.slice(0, DETAIL_CAP).map((e) => e.id));

  const rows: Row[] = entities.map((e) => {
    const d = details.get(e.id);
    const newest = d?.memory.reduce<(typeof d.memory)[number] | null>(
      (a, m) => (!a || (m.created_at ?? "") > (a.created_at ?? "") ? m : a),
      null,
    );
    return {
      id: e.id,
      name: e.name,
      kind: e.kind,
      kindWord: kindLabel(e.kind),
      latest: newest?.text ?? (d ? null : e.summary || null),
      count: d ? d.memory.length : null,
      updated: newest?.created_at ?? null,
    };
  });
  const dir = sort.desc ? -1 : 1;
  const cmp: Record<SortKey, (a: Row, b: Row) => number> = {
    name: (a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: "base" }),
    memory: (a, b) => (a.latest ?? "").localeCompare(b.latest ?? ""),
    count: (a, b) => (a.count ?? -1) - (b.count ?? -1),
    updated: (a, b) => (a.updated ?? "").localeCompare(b.updated ?? ""),
  };
  rows.sort((a, b) => dir * cmp[sort.key](a, b) || a.name.localeCompare(b.name));

  const head = (key: SortKey, label: string, className = "") => {
    const on = sort.key === key;
    return (
      <th scope="col" aria-sort={on ? (sort.desc ? "descending" : "ascending") : "none"} className={`font-medium text-left py-2 px-3 ${className}`}>
        <button
          type="button"
          className="inline-flex items-center gap-1 hover:underline"
          style={{ color: on ? "var(--ink)" : "var(--ink-dim)" }}
          onClick={() => setSort({ key, desc: on ? !sort.desc : key !== "name" && key !== "memory" })}
        >
          {label}
          {on && (sort.desc ? <ArrowDown size={13} strokeWidth={1.75} aria-hidden /> : <ArrowUp size={13} strokeWidth={1.75} aria-hidden />)}
        </button>
      </th>
    );
  };

  return (
    <div className="flex flex-col gap-2 min-w-0">
      <table className="w-full text-[13px] border-collapse" aria-label="People and topics">
        <thead className="text-[12px]" style={{ borderBottom: "1px solid var(--border)" }}>
          <tr>
            {head("name", "Name", "pl-0 md:w-[26%]")}
            {head("memory", "Latest memory", "max-md:hidden")}
            {head("count", "Memories", "max-md:hidden text-right w-24")}
            {head("updated", "Last updated", "w-28 pr-0 text-right")}
          </tr>
        </thead>
        <tbody>
          {rows.map((r) => {
            const picked = r.id === selectedId;
            return (
              <tr
                key={r.id}
                tabIndex={0}
                aria-selected={picked}
                onClick={() => onSelect(r.id)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    onSelect(r.id);
                  }
                }}
                className="cursor-pointer align-top hover:bg-[var(--surface-raised)] focus-visible:outline-2 focus-visible:outline-offset-[-2px] focus-visible:outline-[var(--focus)]"
                style={{ borderBottom: "1px solid var(--border)" }}
              >
                <td className="py-2.5 pr-3 min-w-0">
                  <div className="font-medium text-[14px] truncate" style={{ color: "var(--ink)" }}>
                    {r.name}
                  </div>
                  <div className="text-[13px] inline-flex items-center gap-1.5" style={{ color: "var(--ink-dim)" }}>
                    <span className="w-2 h-2 rounded-full shrink-0" style={{ background: kindColor(r.kind) }} aria-hidden />
                    {r.kindWord}
                  </div>
                  {r.latest && (
                    <div className="md:hidden text-[13px] line-clamp-2 mt-1" style={{ color: "var(--ink-dim)" }}>
                      {r.latest}
                    </div>
                  )}
                </td>
                <td className="py-2.5 px-3 max-md:hidden" style={{ color: r.latest ? "var(--ink-dim)" : "var(--ink-faint)" }}>
                  <span className="line-clamp-2">{r.latest ?? (r.count === 0 ? "No memories yet" : "")}</span>
                </td>
                <td className="py-2.5 px-3 max-md:hidden text-right font-mono text-[12px]" style={{ color: "var(--ink-dim)" }}>
                  {r.count ?? ""}
                </td>
                <td className="py-2.5 pl-3 text-right whitespace-nowrap text-[13px]" style={{ color: "var(--ink-dim)" }}>
                  {r.updated ? (
                    <time dateTime={r.updated} title={new Date(r.updated).toLocaleString()}>
                      {ago(r.updated)}
                    </time>
                  ) : (
                    ""
                  )}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
      {rows.length === 0 && (
        <p className="text-[13px] py-6 text-center" style={{ color: "var(--ink-dim)" }}>
          Nothing matches the kinds shown above.
        </p>
      )}
      {total !== undefined && total > entities.length && (
        <p className="text-[12px]" style={{ color: "var(--ink-dim)" }}>
          Showing the first {entities.length} of {total}. Search finds the rest.
        </p>
      )}
    </div>
  );
}

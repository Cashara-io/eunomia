"use client";

import ErrorLine, { failure } from "@/components/ErrorLine";
import { useState } from "react";
import { useAudit } from "@/lib/queries/audit";
import { PanelHead, TableSkeleton, relativeTime } from "./shared";

const PAGE = 50;

type ResultFilter = "all" | "ok" | "failed";
const FILTERS: { id: ResultFilter; label: string }[] = [
  { id: "all", label: "All" },
  { id: "ok", label: "Succeeded" },
  { id: "failed", label: "Failed" },
];

const isOk = (outcome: string) => outcome === "ok" || outcome === "success";

/* "memory.create" -> "Memory create": the tool name, readable. */
const actionLabel = (tool: string) => {
  const words = tool.replace(/[._]+/g, " ").trim();
  return words ? words[0].toUpperCase() + words.slice(1) : tool;
};

export function AuditSection() {
  const [offset, setOffset] = useState(0);
  const [result, setResult] = useState<ResultFilter>("all");
  const [search, setSearch] = useState("");
  const query = useAudit(PAGE, offset);
  const page = query.data;
  const q = search.trim().toLowerCase();
  const rows = (page?.results ?? []).filter(
    (r) =>
      (result === "all" || (result === "ok") === isOk(r.outcome)) &&
      (!q || `${r.tool_name} ${r.args_summary} ${r.outcome}`.toLowerCase().includes(q))
  );

  return (
    <div className="flex flex-col gap-5">
      <PanelHead title="Audit log">Changes made with your account through Eunomia&apos;s tools, newest first.</PanelHead>

      <div className="flex flex-wrap items-center gap-2">
        <div className="flex flex-wrap gap-1.5" role="group" aria-label="Filter by result">
          {FILTERS.map((f) => (
            <button key={f.id} type="button" className="pill" aria-pressed={result === f.id} onClick={() => setResult(f.id)}>
              {f.label}
            </button>
          ))}
        </div>
        <input
          type="search"
          aria-label="Search this page of the audit log"
          placeholder="Search action or target"
          className="field h-9 px-3 text-[13px] w-full sm:w-[240px] sm:ml-auto"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
      </div>

      {query.isError ? (
        <ErrorLine error={failure(query.error, "Could not load the audit log. Reload to try again.")} />
      ) : (
        <div className="overflow-x-auto">
          <table className="data-table w-full text-[13px]">
            <thead>
              <tr>
                <th className="text-left">Time</th>
                <th className="text-left">Action</th>
                <th className="text-left">Target</th>
                <th className="text-left">Result</th>
              </tr>
            </thead>
            <tbody>
              {!page ? (
                <TableSkeleton cols={4} />
              ) : rows.length === 0 ? (
                <tr>
                  <td colSpan={4} className="py-6 text-center" style={{ color: "var(--ink-dim)" }}>
                    {page.results.length === 0 ? "Nothing recorded yet." : "Nothing on this page matches."}
                  </td>
                </tr>
              ) : (
                rows.map((r) => (
                  <tr key={r.id}>
                    <td className="whitespace-nowrap" style={{ color: "var(--ink-dim)" }} title={r.created_at ? new Date(r.created_at).toLocaleString() : undefined}>
                      {relativeTime(r.created_at ?? null)}
                    </td>
                    <td className="whitespace-nowrap">{actionLabel(r.tool_name)}</td>
                    <td className="max-w-[360px] truncate font-mono text-[12px]" title={r.args_summary} style={{ color: "var(--ink-dim)" }}>
                      {r.args_summary || "—"}
                    </td>
                    <td className="whitespace-nowrap">
                      <span className="inline-flex items-center gap-1.5">
                        <span className="dot" style={{ background: isOk(r.outcome) ? "var(--good)" : "var(--critical)" }} aria-hidden />
                        {isOk(r.outcome) ? "Succeeded" : "Failed"}
                        {!isOk(r.outcome) && (
                          <span className="font-mono text-[12px]" style={{ color: "var(--ink-faint)" }}>
                            {r.outcome}
                          </span>
                        )}
                      </span>
                    </td>
                  </tr>
                ))
              )}
            </tbody>
          </table>
        </div>
      )}

      {page && page.total > PAGE && (
        <div className="flex items-center justify-between gap-3 text-[13px]" style={{ color: "var(--ink-dim)" }}>
          <span>
            {offset + 1}–{offset + page.results.length} of {page.total}
          </span>
          <span className="flex gap-2">
            <button type="button" className="btn btn-sm" disabled={offset === 0} onClick={() => setOffset(Math.max(0, offset - PAGE))}>
              Newer
            </button>
            <button type="button" className="btn btn-sm" disabled={!page.has_more} onClick={() => setOffset(offset + PAGE)}>
              Older
            </button>
          </span>
        </div>
      )}
    </div>
  );
}

"use client";

import ErrorLine, { failure } from "@/components/ErrorLine";
import { useState } from "react";
import { Download } from "lucide-react";
import { useAudit } from "@/lib/queries/audit";
import { useWorkspaceAudit } from "@/lib/queries/admin";
import type { AuditCategory } from "@/lib/gen";
import { PanelHead, TableSkeleton, relativeTime } from "./shared";

const PAGE = 50;

// Sign-ins are routine and drown out the rest: the log opens on changes, sign-ins are one click away.
const CATEGORIES: { id: AuditCategory; label: string }[] = [
  { id: "changes", label: "Changes" },
  { id: "spaces", label: "Spaces" },
  { id: "tokens", label: "Tokens" },
  { id: "apps", label: "Apps" },
  { id: "admin", label: "Admin" },
  { id: "signins", label: "Sign-ins" },
  { id: "all", label: "Everything" },
];

type View = "workspace" | "mine";

/** One row of either log, ready to show: `failed` is the result in words, set only when it did not work. */
type LogRow = { id: string; at: string | null; who: string; whoNote?: string; what: string; whatMono?: boolean; failed?: string };

/** "6 Nov, 14:32", with the year when it isn't this one. */
function absoluteTime(iso: string | null): string {
  if (!iso) return "—";
  const d = new Date(iso);
  const sameYear = d.getFullYear() === new Date().getFullYear();
  const day = d.toLocaleDateString(undefined, { day: "numeric", month: "short", ...(sameYear ? {} : { year: "numeric" }) });
  const time = d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit", hourCycle: "h23" });
  return `${day}, ${time}`;
}

function Time({ at }: { at: string | null }) {
  return (
    <time dateTime={at ?? undefined} title={at ? relativeTime(at) : undefined} className="tabular-nums whitespace-nowrap">
      {absoluteTime(at)}
    </time>
  );
}

function Failed({ words }: { words?: string }) {
  if (!words) return null;
  return (
    <span className="inline-flex items-center gap-1.5 ml-2 text-[12px] whitespace-nowrap" style={{ color: "var(--critical)" }}>
      <span className="dot" style={{ background: "var(--critical)" }} aria-hidden />
      {words}
    </span>
  );
}

/* A table from 640px up; below that each event is two stacked lines: what happened, then who and when. */
function LogRows({ rows, loading, whoLabel, whatLabel }: { rows: LogRow[]; loading: boolean; whoLabel: string; whatLabel: string }) {
  if (!loading && rows.length === 0)
    return (
      <p className="py-6 text-center text-[13px]" style={{ color: "var(--ink-dim)" }}>
        Nothing recorded yet.
      </p>
    );
  return (
    <>
      <div className="hidden sm:block overflow-x-auto">
        <table className="data-table w-full text-[13px]">
          <thead>
            <tr>
              <th className="text-left w-[130px]">Time</th>
              <th className="text-left">{whoLabel}</th>
              <th className="text-left">{whatLabel}</th>
            </tr>
          </thead>
          <tbody>
            {loading ? (
              <TableSkeleton cols={3} />
            ) : (
              rows.map((r) => (
                <tr key={r.id}>
                  <td style={{ color: "var(--ink-dim)" }}>
                    <Time at={r.at} />
                  </td>
                  <td className="max-w-[220px]">
                    <span className="flex flex-col min-w-0">
                      <span className="truncate" title={r.who}>
                        {r.who}
                      </span>
                      {r.whoNote && (
                        <span className="text-[12px]" style={{ color: "var(--ink-dim)" }}>
                          {r.whoNote}
                        </span>
                      )}
                    </span>
                  </td>
                  <td className="min-w-[220px]">
                    <span className={r.whatMono ? "font-mono text-[12px]" : undefined} style={r.whatMono ? { color: "var(--ink-dim)" } : undefined}>
                      {r.what}
                    </span>
                    <Failed words={r.failed} />
                  </td>
                </tr>
              ))
            )}
          </tbody>
        </table>
      </div>

      <ul className="sm:hidden hairline-rows" aria-busy={loading}>
        {loading
          ? [0, 1, 2].map((i) => (
              <li key={i} className="py-3 flex flex-col gap-1.5" aria-hidden>
                <span className="skeleton h-4 w-3/4" />
                <span className="skeleton h-3 w-1/2" />
              </li>
            ))
          : rows.map((r) => (
              <li key={r.id} className="py-3 flex flex-col gap-0.5 min-w-0">
                <span className="text-[14px] break-words">
                  {r.what}
                  <Failed words={r.failed} />
                </span>
                <span className="text-[12px] truncate" style={{ color: "var(--ink-dim)" }}>
                  {r.who}
                  {r.whoNote ? ` ${r.whoNote}` : ""} · <Time at={r.at} />
                </span>
              </li>
            ))}
      </ul>
    </>
  );
}

/* The rows on screen as a CSV file, built in the browser. */
function downloadCsv(rows: LogRow[], name: string) {
  const cell = (v: string) => (/[",\n\r]/.test(v) ? `"${v.replace(/"/g, '""')}"` : v);
  const lines = [
    ["Time", "Who", "How", "What", "Result"],
    ...rows.map((r) => [r.at ?? "", r.who, r.whoNote ?? "", r.what, r.failed ?? "Done"]),
  ].map((l) => l.map(cell).join(","));
  const url = URL.createObjectURL(new Blob([lines.join("\r\n") + "\r\n"], { type: "text/csv;charset=utf-8" }));
  const a = document.createElement("a");
  a.href = url;
  a.download = `${name}-${new Date().toISOString().slice(0, 10)}.csv`;
  a.click();
  URL.revokeObjectURL(url);
}

function CsvButton({ rows, name }: { rows: LogRow[]; name: string }) {
  return (
    <button type="button" className="btn btn-ghost btn-sm" disabled={rows.length === 0} onClick={() => downloadCsv(rows, name)}>
      <Download size={14} strokeWidth={1.75} aria-hidden />
      Download CSV
    </button>
  );
}

function Pager({ offset, count, hasMore, onMove }: { offset: number; count: number; hasMore: boolean; onMove: (o: number) => void }) {
  if (offset === 0 && !hasMore) return null;
  return (
    <div className="flex items-center justify-between gap-3 text-[13px]" style={{ color: "var(--ink-dim)" }}>
      <span>
        {count ? `${offset + 1}–${offset + count}` : "Nothing here"}
      </span>
      <span className="flex gap-2">
        <button type="button" className="btn btn-sm" disabled={offset === 0} onClick={() => onMove(Math.max(0, offset - PAGE))}>
          Newer
        </button>
        <button type="button" className="btn btn-sm" disabled={!hasMore} onClick={() => onMove(offset + PAGE)}>
          Older
        </button>
      </span>
    </div>
  );
}

export function AuditSection() {
  const [view, setView] = useState<View>("workspace");
  return (
    <div className="flex flex-col gap-5">
      <PanelHead
        title="Audit log"
        action={
          <div className="tabs" role="tablist" aria-label="Which log">
            <button type="button" role="tab" className="tab" aria-selected={view === "workspace"} onClick={() => setView("workspace")}>
              Workspace
            </button>
            <button type="button" role="tab" className="tab" aria-selected={view === "mine"} onClick={() => setView("mine")}>
              Your AI tool calls
            </button>
          </div>
        }
      >
        {view === "workspace"
          ? "Changes to spaces, tokens, apps and admin rights, newest first."
          : "Changes your AI apps made with your account, newest first."}
      </PanelHead>
      {view === "workspace" ? <WorkspaceLog /> : <ToolCallLog />}
    </div>
  );
}

function WorkspaceLog() {
  const [category, setCategory] = useState<AuditCategory>("changes");
  const [offset, setOffset] = useState(0);
  const query = useWorkspaceAudit(category, PAGE, offset);
  const page = query.data;
  const rows: LogRow[] = (page?.results ?? []).map((r) => ({
    id: r.id,
    at: r.created_at ?? null,
    who: r.actor_email || (r.via === "system" ? "Eunomia" : "Unknown"),
    whoNote: r.via && r.via !== "system" ? `with an ${r.via}` : undefined,
    what: r.summary,
    failed: r.ok ? undefined : r.result,
  }));

  return (
    <>
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex flex-wrap gap-1.5" role="group" aria-label="Filter by kind of event">
          {CATEGORIES.map((c) => (
            <button
              key={c.id}
              type="button"
              className="pill"
              aria-pressed={category === c.id}
              onClick={() => {
                setCategory(c.id);
                setOffset(0);
              }}
            >
              {c.label}
            </button>
          ))}
        </div>
        <CsvButton rows={rows} name={`audit-${category}`} />
      </div>

      {query.isError ? (
        <ErrorLine error={failure(query.error, "Could not load the audit log. Reload to try again.")} />
      ) : (
        <LogRows rows={rows} loading={!page} whoLabel="Who" whatLabel="What" />
      )}

      {page && <Pager offset={offset} count={rows.length} hasMore={page.has_more} onMove={setOffset} />}
    </>
  );
}

const isOk = (outcome: string) => outcome === "ok" || outcome === "success";

/* "memory.create" -> "Memory create": the tool name, readable. */
const actionLabel = (tool: string) => {
  const words = tool.replace(/[._]+/g, " ").trim();
  return words ? words[0].toUpperCase() + words.slice(1) : tool;
};

function ToolCallLog() {
  const [offset, setOffset] = useState(0);
  const query = useAudit(PAGE, offset);
  const page = query.data;
  const rows: LogRow[] = (page?.results ?? []).map((r) => ({
    id: r.id,
    at: r.created_at ?? null,
    who: actionLabel(r.tool_name),
    what: r.args_summary || "—",
    whatMono: true,
    failed: isOk(r.outcome) ? undefined : "Failed",
  }));

  return (
    <>
      <div className="flex justify-end">
        <CsvButton rows={rows} name="tool-calls" />
      </div>
      {query.isError ? (
        <ErrorLine error={failure(query.error, "Could not load your tool calls. Reload to try again.")} />
      ) : (
        <LogRows rows={rows} loading={!page} whoLabel="Action" whatLabel="Details" />
      )}
      {page && <Pager offset={offset} count={rows.length} hasMore={page.has_more} onMove={setOffset} />}
    </>
  );
}

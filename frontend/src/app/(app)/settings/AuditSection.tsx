"use client";

import ErrorLine, { failure } from "@/components/ErrorLine";
import { useState } from "react";
import { useAudit } from "@/lib/queries/audit";
import { useWorkspaceAudit } from "@/lib/queries/admin";
import type { AuditCategory } from "@/lib/gen";
import { PanelHead, TableSkeleton, relativeTime } from "./shared";

const PAGE = 50;

const CATEGORIES: { id: AuditCategory; label: string }[] = [
  { id: "all", label: "All" },
  { id: "spaces", label: "Spaces" },
  { id: "signins", label: "Sign-ins" },
  { id: "tokens", label: "Tokens" },
  { id: "apps", label: "Apps" },
];

type View = "workspace" | "mine";

function TimeCell({ at }: { at?: string | null }) {
  return (
    <td className="whitespace-nowrap" style={{ color: "var(--ink-dim)" }} title={at ? new Date(at).toLocaleString() : undefined}>
      {relativeTime(at ?? null)}
    </td>
  );
}

function ResultCell({ ok, words }: { ok: boolean; words: string }) {
  return (
    <td className="whitespace-nowrap">
      <span className="inline-flex items-center gap-1.5">
        <span className="dot" style={{ background: ok ? "var(--good)" : "var(--critical)" }} aria-hidden />
        {words}
      </span>
    </td>
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
          ? "Who signed in, changed tokens, connected apps or changed a space, newest first."
          : "Changes your AI apps made with your account, newest first."}
      </PanelHead>
      {view === "workspace" ? <WorkspaceLog /> : <ToolCallLog />}
    </div>
  );
}

function WorkspaceLog() {
  const [category, setCategory] = useState<AuditCategory>("all");
  const [offset, setOffset] = useState(0);
  const query = useWorkspaceAudit(category, PAGE, offset);
  const page = query.data;
  const rows = page?.results ?? [];

  return (
    <>
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

      {query.isError ? (
        <ErrorLine error={failure(query.error, "Could not load the audit log. Reload to try again.")} />
      ) : (
        <div className="overflow-x-auto">
          <table className="data-table w-full text-[13px]">
            <thead>
              <tr>
                <th className="text-left w-[100px]">Time</th>
                <th className="text-left">Who</th>
                <th className="text-left">What</th>
                <th className="text-left w-[120px]">Result</th>
              </tr>
            </thead>
            <tbody>
              {!page ? (
                <TableSkeleton cols={4} />
              ) : rows.length === 0 ? (
                <tr>
                  <td colSpan={4} className="py-6 text-center" style={{ color: "var(--ink-dim)" }}>
                    Nothing recorded yet.
                  </td>
                </tr>
              ) : (
                rows.map((r) => (
                  <tr key={r.id}>
                    <TimeCell at={r.created_at} />
                    <td className="max-w-[220px]">
                      <span className="flex flex-col min-w-0">
                        <span className="truncate" title={r.actor_email}>
                          {r.actor_email || (r.via === "system" ? "Eunomia" : "Unknown")}
                        </span>
                        {r.via && r.via !== "system" && (
                          <span className="text-[12px]" style={{ color: "var(--ink-dim)" }}>
                            with an {r.via}
                          </span>
                        )}
                      </span>
                    </td>
                    <td className="min-w-[220px]">{r.summary}</td>
                    <ResultCell ok={r.ok} words={r.result} />
                  </tr>
                ))
              )}
            </tbody>
          </table>
        </div>
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
  const rows = page?.results ?? [];

  return (
    <>
      {query.isError ? (
        <ErrorLine error={failure(query.error, "Could not load your tool calls. Reload to try again.")} />
      ) : (
        <div className="overflow-x-auto">
          <table className="data-table w-full text-[13px]">
            <thead>
              <tr>
                <th className="text-left w-[100px]">Time</th>
                <th className="text-left">Action</th>
                <th className="text-left">Details</th>
                <th className="text-left w-[120px]">Result</th>
              </tr>
            </thead>
            <tbody>
              {!page ? (
                <TableSkeleton cols={4} />
              ) : rows.length === 0 ? (
                <tr>
                  <td colSpan={4} className="py-6 text-center" style={{ color: "var(--ink-dim)" }}>
                    Nothing recorded yet.
                  </td>
                </tr>
              ) : (
                rows.map((r) => (
                  <tr key={r.id}>
                    <TimeCell at={r.created_at} />
                    <td className="whitespace-nowrap">{actionLabel(r.tool_name)}</td>
                    <td className="max-w-[360px] truncate font-mono text-[12px]" title={r.args_summary} style={{ color: "var(--ink-dim)" }}>
                      {r.args_summary || "—"}
                    </td>
                    <ResultCell ok={isOk(r.outcome)} words={isOk(r.outcome) ? "Done" : "Failed"} />
                  </tr>
                ))
              )}
            </tbody>
          </table>
        </div>
      )}
      {page && <Pager offset={offset} count={rows.length} hasMore={page.has_more} onMove={setOffset} />}
    </>
  );
}

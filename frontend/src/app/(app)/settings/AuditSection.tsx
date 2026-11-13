"use client";
import LoadingSkeleton from "@/components/ui/LoadingSkeleton";

import ErrorLine, { failure } from "@/components/ErrorLine";
import { useState } from "react";
import { DownloadSimpleIcon, SealCheckIcon } from "@phosphor-icons/react/ssr";
import { useAudit } from "@/lib/queries/audit";
import { downloadAuditExport, useVerifyAudit, useWorkspaceAudit } from "@/lib/queries/admin";
import type { AuditCategory, AuditVerification } from "@/lib/gen";
import { PanelHead, StatusLine, TableSkeleton, relativeTime } from "./shared";

const PAGE = 50;

// Sign-ins are routine and drown out the rest: the log opens on changes, sign-ins are one click away.
const CATEGORIES: { id: AuditCategory; label: string }[] = [
  { id: "changes", label: "Changes" },
  { id: "spaces", label: "Spaces" },
  { id: "tokens", label: "Tokens" },
  { id: "apps", label: "Apps" },
  { id: "sharing", label: "Sharing" },
  { id: "server", label: "Server" },
  { id: "admin", label: "Admin" },
  { id: "signins", label: "Sign-ins" },
  { id: "all", label: "Everything" },
];

type View = "workspace" | "mine";

/** One row of either log, ready to show: `failed` is the result in words, set only when it did not work;
 * `reason` says why, when the server knows. */
type LogRow = { id: string; at: string | null; who: string; whoNote?: string; what: string; whatMono?: boolean; failed?: string; reason?: string };

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

/* Under a failed event: a red dot and why it failed (or just "Failed"), dim. */
function Failed({ r }: { r: LogRow }) {
  if (!r.failed) return null;
  return (
    <span className="flex items-center gap-1.5 text-[12px] min-w-0" style={{ color: "var(--ink-dim)" }}>
      <span className="dot" style={{ background: "var(--critical)" }} aria-hidden />
      <span className="sr-only">{r.failed}: </span>
      <span className="min-w-0 break-words">{r.reason || r.failed}</span>
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
        <table className="data-table w-full table-fixed text-[13px]">
          <thead>
            <tr>
              <th className="text-left w-[120px]">Time</th>
              <th className="text-left w-[30%]">{whoLabel}</th>
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
                  <td>
                    <span className="flex flex-col min-w-0 py-1.5">
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
                  <td>
                    <span className="flex flex-col gap-0.5 min-w-0 py-1.5">
                      <span
                        className={`break-words ${r.whatMono ? "font-mono text-[12px]" : ""}`}
                        style={r.whatMono ? { color: "var(--ink-dim)" } : undefined}
                      >
                        {r.what}
                      </span>
                      <Failed r={r} />
                    </span>
                  </td>
                </tr>
              ))
            )}
          </tbody>
        </table>
      </div>

      <ul className="sm:hidden hairline-rows" aria-busy={loading}>
        {loading
          ? <li><LoadingSkeleton label="Loading activity" rows={3} /></li>
          : rows.map((r) => (
              <li key={r.id} className="py-3 flex flex-col gap-0.5 min-w-0">
                <span className="text-[14px] break-words">{r.what}</span>
                <Failed r={r} />
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
    ["Time", "Who", "How", "What", "Result", "Reason"],
    ...rows.map((r) => [r.at ?? "", r.who, r.whoNote ?? "", r.what, r.failed ?? "Done", r.reason ?? ""]),
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
      <DownloadSimpleIcon weight="duotone" size={14} aria-hidden />
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
      {view === "workspace" && <Integrity />}
      {view === "workspace" ? <WorkspaceLog /> : <ToolCallLog />}
    </div>
  );
}

/* One ledger's check in words: "Intact: events 1 to 240 checked" or where it breaks. */
function LedgerResult({ v }: { v: AuditVerification }) {
  const name = v.ledger === "instance" ? "Sign-ins with no account" : "This workspace";
  if (!v.ok)
    return (
      <StatusLine tone="critical">
        {name}: altered or incomplete. {v.break?.message}
      </StatusLine>
    );
  const range = v.checked ? `events ${v.first_seq} to ${v.last_seq} checked` : "nothing recorded yet";
  const from = v.anchor?.kind === "retention" ? ", from the last retention cut" : "";
  const older = v.before_chain ? `; ${v.before_chain} older events predate the chain` : "";
  return (
    <StatusLine tone="good">
      {name}: intact, {range}
      {from}
      {older}.
    </StatusLine>
  );
}

/* Check the hash chain, and download a date range of the log that verifies offline. */
function Integrity() {
  const verify = useVerifyAudit();
  const [from, setFrom] = useState("");
  const [to, setTo] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ReturnType<typeof failure> | null>(null);
  async function download() {
    setBusy(true);
    setError(null);
    try {
      await downloadAuditExport(from, to);
    } catch (e) {
      setError(failure(e, "Could not download the security log."));
    } finally {
      setBusy(false);
    }
  }
  return (
    <section aria-labelledby="integrity-title" className="flex flex-col gap-3 rounded-[12px] border p-4" style={{ borderColor: "var(--border)" }}>
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="min-w-0">
          <h3 id="integrity-title" className="text-[14px] font-medium">
            Integrity
          </h3>
          <p className="text-[13px] max-w-[60ch]" style={{ color: "var(--ink-dim)" }}>
            Every event is chained to the one before it, so an edited or deleted event shows up here.
          </p>
        </div>
        <button type="button" className="btn btn-sm" disabled={verify.isFetching} onClick={() => void verify.refetch()}>
          <SealCheckIcon weight="duotone" size={14} aria-hidden />
          {verify.isFetching ? "Checking…" : "Check integrity"}
        </button>
      </div>
      {verify.isError && <ErrorLine error={failure(verify.error, "Could not check the security log.")} />}
      {verify.data && (
        <div className="flex flex-col gap-1" data-testid="integrity-result">
          {verify.data.ledgers.map((v) => (
            <LedgerResult key={v.ledger} v={v} />
          ))}
        </div>
      )}
      <div className="flex flex-wrap items-end gap-3">
        <label className="flex flex-col gap-1">
          <span className="label">From</span>
          <input type="date" className="field h-9 px-2 text-[13px]" value={from} onChange={(e) => setFrom(e.target.value)} />
        </label>
        <label className="flex flex-col gap-1">
          <span className="label">Until</span>
          <input type="date" className="field h-9 px-2 text-[13px]" value={to} onChange={(e) => setTo(e.target.value)} />
        </label>
        <button type="button" className="btn btn-ghost btn-sm" disabled={busy} onClick={() => void download()}>
          <DownloadSimpleIcon weight="duotone" size={14} aria-hidden />
          {busy ? "Preparing…" : "Download for auditors"}
        </button>
      </div>
      <p className="text-[12px] max-w-[68ch]" style={{ color: "var(--ink-faint)" }}>
        The file lists who did what and when, with each event&apos;s hash. It holds no memory text, chat content or secrets. Check it
        offline with <code className="font-mono">scripts/audit/verify-audit-export.py</code>.
      </p>
      {error && <ErrorLine error={error} />}
    </section>
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
    // another Eunomia acting on a share code: the sentence names it, so the actor is just "Another Eunomia"
    who: r.via === "another Eunomia" ? "Another Eunomia" : r.actor_email || (r.via === "system" ? "Eunomia" : "Unknown"),
    whoNote: r.via && r.via !== "system" && r.via !== "another Eunomia" ? `with an ${r.via}` : undefined,
    what: r.summary,
    failed: r.ok ? undefined : r.result,
    reason: r.ok ? undefined : r.reason || undefined,
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

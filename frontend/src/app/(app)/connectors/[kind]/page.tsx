"use client";

import { Fragment, use, useRef, useState } from "react";
import Link from "next/link";
import { ChevronRight, ExternalLink, Search, Settings2, Trash2, X } from "lucide-react";
import type { ConnectorKind, SourceRow, ToolHit } from "@/lib/types";
import { useDeleteConnectorData } from "@/lib/queries/connectors";
import { useSources, useSyncSource } from "@/lib/queries/sources";
import { useRecord, useSourceRecords, useSourceSearch } from "@/lib/queries/tools";
import ErrorLine, { failure, type Failure } from "@/components/ErrorLine";
import AuthorTag from "@/components/AuthorTag";
import DigitRoll from "@/components/bits/DigitRoll";
import SyncMark, { type SyncStatus } from "@/components/bits/SyncMark";
import Tooltip from "@/components/bits/Tooltip";
import { ConnectorTile, kindForSource, relativeTime } from "@/lib/connectorMeta";

const DATE_KEY = /(_at|date|time)$/i;
const MONEY_KEY = /(amount|balance|price|cost|total|spent|fee)/i;
const DURATION_KEY = /duration/i;
const ISO_DATE = /^\d{4}-\d{2}-\d{2}T/;

function formatKey(key: string): string {
  return key
    .replace(/_/g, " ")
    .replace(/([a-z])([A-Z])/g, "$1 $2")
    .replace(/\b\w/g, (c) => c.toUpperCase());
}

function formatDate(iso: string | null | undefined): string {
  if (!iso) return "No date";
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
}

function formatPrimitive(key: string, value: unknown): string {
  if (value === null || value === undefined || value === "") return "None";
  if (typeof value === "boolean") return value ? "Yes" : "No";
  if (typeof value === "number") {
    if (MONEY_KEY.test(key)) return `$${value.toFixed(2)}`;
    if (DURATION_KEY.test(key)) return `${value} min`;
    return value.toLocaleString();
  }
  if (typeof value === "string") {
    if (DATE_KEY.test(key) || ISO_DATE.test(value)) {
      const d = new Date(value);
      if (!Number.isNaN(d.getTime())) return d.toLocaleString();
    }
    return value;
  }
  return String(value);
}

/** Renders one payload value, formatted by simple heuristics on its key/type
 * (dates, currency-looking numbers), never a raw JSON dump at the top level. */
function PayloadValue({ k, v }: { k: string; v: unknown }) {
  if (typeof v === "string" && /^https?:\/\//i.test(v)) {
    return (
      <a href={v} target="_blank" rel="noreferrer" className="underline break-all" style={{ color: "var(--accent-text)" }}>
        {v}
      </a>
    );
  }
  if (Array.isArray(v)) {
    if (v.length === 0) return <span style={{ color: "var(--ink-faint)" }}>None</span>;
    if (v.every((item) => item === null || typeof item !== "object")) return <span>{v.map((item) => formatPrimitive(k, item)).join(", ")}</span>;
    return <span style={{ color: "var(--ink-dim)" }}>{v.length} items</span>;
  }
  if (v && typeof v === "object") {
    const entries = Object.entries(v as Record<string, unknown>);
    if (entries.length === 0) return <span style={{ color: "var(--ink-faint)" }}>None</span>;
    return (
      <div className="flex flex-col gap-1 pl-3" style={{ borderLeft: "1px solid var(--border)" }}>
        {entries.map(([nk, nv]) => (
          <div key={nk} className="flex items-baseline gap-2 text-[12px]">
            <span className="shrink-0" style={{ color: "var(--ink-faint)" }}>
              {formatKey(nk)}
            </span>
            <span style={{ color: "var(--ink-dim)" }}>{typeof nv === "object" && nv !== null ? JSON.stringify(nv) : formatPrimitive(nk, nv)}</span>
          </div>
        ))}
      </div>
    );
  }
  return <span>{formatPrimitive(k, v)}</span>;
}

function RecordRow({ hit }: { hit: ToolHit }) {
  const [open, setOpen] = useState(false);
  const record = useRecord(hit.id, open);
  const detail = record.isError ? "error" : (record.data ?? null);

  function toggle() {
    setOpen((o) => !o);
  }

  const payload = detail && detail !== "error" ? Object.entries(detail.payload || {}) : [];

  return (
    <Fragment>
      <tr>
        <td className="max-w-0 w-full">
          <button
            type="button"
            onClick={toggle}
            aria-expanded={open}
            className="flex items-center gap-2 w-full min-w-0 text-left h-10 active:scale-[0.99] transition-transform duration-[120ms] ease-[var(--ease-out)]"
          >
            <ChevronRight
              size={14}
              strokeWidth={1.75}
              color="var(--ink-faint)"
              className="shrink-0"
              style={{ transform: open ? "rotate(90deg)" : undefined }}
              aria-hidden
            />
            <span className="truncate font-medium">{hit.title || "Untitled"}</span>
          </button>
        </td>
        <td className="hidden sm:table-cell whitespace-nowrap">
          <span className="font-mono text-[12px]" style={{ color: "var(--ink-dim)" }}>
            {hit.type}
          </span>
        </td>
        <td className="whitespace-nowrap text-right font-mono text-[12px]" style={{ color: "var(--ink-dim)" }} title={hit.occurred_at ?? undefined}>
          {hit.occurred_at ? relativeTime(hit.occurred_at) : "No date"}
        </td>
      </tr>
      {open && (
        <tr>
          <td colSpan={3} className="!h-auto" style={{ background: "var(--surface-raised)" }}>
            <div className="py-3 pl-6 flex flex-col gap-2 text-[12.5px]">
              {detail === null && (
                <div className="flex flex-col gap-2" aria-label="Loading record">
                  <span className="skeleton h-3 w-64" />
                  <span className="skeleton h-3 w-48" />
                </div>
              )}
              {detail === "error" && (
                <ErrorLine error={failure(record.error, "Could not load this record.", " Collapse and expand the row to retry.")} />
              )}
              {detail && detail !== "error" && (
                <>
                  <div className="flex items-start gap-3">
                    <span className="label w-32 shrink-0 pt-px">Occurred</span>
                    <span className="font-mono">{formatDate(hit.occurred_at)}</span>
                  </div>
                  {payload.map(([k, v]) => (
                    <div key={k} className="flex items-start gap-3">
                      <span className="label w-32 shrink-0 pt-px">{formatKey(k)}</span>
                      <div className="flex-1 min-w-0">
                        <PayloadValue k={k} v={v} />
                      </div>
                    </div>
                  ))}
                  {payload.length === 0 && <p style={{ color: "var(--ink-faint)" }}>This record has no payload fields.</p>}
                  {detail.recording?.transcript && (
                    <details>
                      <summary className="cursor-pointer" style={{ color: "var(--ink-dim)" }}>
                        Full transcript
                      </summary>
                      <pre className="mt-2 whitespace-pre-wrap font-sans max-h-96 overflow-auto" style={{ color: "var(--ink)" }}>
                        {detail.recording.transcript}
                      </pre>
                    </details>
                  )}
                  {hit.url && (
                    <a href={hit.url} target="_blank" rel="noreferrer" className="inline-flex items-center gap-1 self-start" style={{ color: "var(--accent-text)" }}>
                      Open in source <ExternalLink size={12} strokeWidth={1.75} aria-hidden />
                    </a>
                  )}
                </>
              )}
            </div>
          </td>
        </tr>
      )}
    </Fragment>
  );
}

function Breadcrumb({ label }: { label: string }) {
  return (
    <nav aria-label="Breadcrumb" className="flex items-center gap-1.5 text-[12.5px]" style={{ color: "var(--ink-faint)" }}>
      <Link href="/connectors" className="hover:underline" style={{ color: "var(--ink-dim)" }}>
        Connectors
      </Link>
      <ChevronRight size={12} strokeWidth={1.75} aria-hidden />
      <span aria-current="page">{label}</span>
    </nav>
  );
}

function health(row: SourceRow): { tone: string; text: string } {
  const { consecutive_failures: fails, last_ok } = row.sync_status;
  if (fails > 0) return { tone: fails > 2 ? "var(--critical)" : "var(--warning)", text: `${fails} failed sync${fails === 1 ? "" : "s"} in a row` };
  if (!last_ok) return { tone: "var(--ink-faint)", text: "Waiting for first sync" };
  return { tone: "var(--good)", text: "Healthy" };
}

/** "Delete all data" in a modal with two confirmations: what goes, then type the connector's name. */
function DeleteData({ row, kind }: { row: SourceRow; kind: ConnectorKind }) {
  const del = useDeleteConnectorData();
  const dialog = useRef<HTMLDialogElement>(null);
  const [step, setStep] = useState<1 | 2>(1);
  const [typed, setTyped] = useState("");
  const [error, setError] = useState<Failure | null>(null);

  function open() {
    setStep(1);
    setTyped("");
    setError(null);
    dialog.current?.showModal();
  }

  async function run() {
    setError(null);
    try {
      await del.mutateAsync(row.connector ?? kind);
      dialog.current?.close();
    } catch (e) {
      setError(failure(e, `Could not delete the ${row.label} data. Try again.`));
    }
  }

  return (
    <section className="flex flex-col gap-3" aria-labelledby="delete-data-heading">
      <h2 id="delete-data-heading" className="section-title">
        Data
      </h2>
      <button type="button" onClick={open} className="btn btn-danger btn-sm self-start">
        <Trash2 size={14} strokeWidth={1.75} aria-hidden />
        Delete all {row.label} data
      </button>
      <dialog
        ref={dialog}
        aria-labelledby="delete-data-title"
        className="panel pop-in m-auto w-[calc(100%-32px)] max-w-md p-5 backdrop:bg-black/40"
        style={{ color: "var(--ink)" }}
      >
        <h3 id="delete-data-title" className="text-[15px] font-medium mb-2">
          {step === 1 ? `Delete all ${row.label} data?` : "Are you sure?"}
        </h3>
        {step === 1 ? (
          <p className="text-[12.5px] mb-4" style={{ color: "var(--ink-dim)" }}>
            This permanently deletes the {row.record_count.toLocaleString()} {row.label} records Eunomia has synced, their links, and every fact and
            relation extracted from them. The connection itself stays, so later syncs bring in new data only.
          </p>
        ) : (
          <label className="text-[12.5px] flex flex-col gap-1.5 mb-4" style={{ color: "var(--ink-dim)" }}>
            <span>
              Type <span className="font-mono" style={{ color: "var(--ink)" }}>{row.label}</span> to confirm. This can&apos;t be undone.
            </span>
            <input autoFocus className="field h-8 px-2.5 text-[13px] font-mono" value={typed} onChange={(e) => setTyped(e.target.value)} />
          </label>
        )}
        {error && <ErrorLine error={error} />}
        <div className="flex justify-end gap-1.5 mt-2">
          <button type="button" onClick={() => dialog.current?.close()} className="btn btn-ghost btn-sm">
            Cancel
          </button>
          {step === 1 ? (
            <button type="button" onClick={() => setStep(2)} className="btn btn-danger btn-sm">
              Continue
            </button>
          ) : (
            <button type="button" onClick={run} disabled={typed !== row.label || del.isPending} className="btn btn-danger btn-sm">
              <Trash2 size={14} strokeWidth={1.75} aria-hidden />
              {del.isPending ? "Deleting…" : "Delete everything"}
            </button>
          )}
        </div>
      </dialog>
    </section>
  );
}

export default function ConnectorWorkspacePage({ params }: { params: Promise<{ kind: string }> }) {
  const { kind } = use(params);
  const connectorKind = kindForSource(kind);
  const rowsQuery = useSources();
  const listed: SourceRow | null | undefined = rowsQuery.data ? (rowsQuery.data.find((r) => r.key === kind) ?? null) : undefined;
  const [query, setQuery] = useState("");
  // The submitted search; "" shows the latest records.
  const [activeQuery, setActiveQuery] = useState("");
  // An added connection's key is `{source}~{instance}`; its records carry the plain source key.
  const sourceKey = kind.split("~")[0];
  const listQuery = useSourceRecords(sourceKey);
  const searchQuery = useSourceSearch(sourceKey, activeQuery);
  const syncSource = useSyncSource();
  const shown = activeQuery ? (searchQuery.data ?? listQuery.data) : listQuery.data;
  const results: ToolHit[] | null = activeQuery && searchQuery.isError ? [] : (shown ?? (listQuery.isError ? [] : null));
  const searching = activeQuery !== "" && searchQuery.isFetching;
  // Records can outlive their connector (demo data, a removed connector): keep them readable under a read-only row.
  const orphaned = listed === null && (listQuery.data?.length ?? 0) > 0;
  const row: SourceRow | null | undefined = orphaned
    ? { key: kind, label: formatKey(kind), provider: kind, record_types: [], connected: false, record_count: listQuery.data?.length ?? 0, sync_status: { cursor: "", last_run: null, last_ok: null, last_error: "", consecutive_failures: 0 } }
    : listed === null && listQuery.isPending
      ? undefined
      : listed;
  const [syncing, setSyncing] = useState(false);
  // Result mark after a sync: holds 1.2s, then the button goes back to idle.
  const [syncResult, setSyncResult] = useState<"done" | "failed" | null>(null);
  const [syncError, setSyncError] = useState<Failure | null>(null);

  function loadRecords() {
    setActiveQuery("");
    listQuery.refetch();
  }

  function runSearch(e: React.FormEvent) {
    e.preventDefault();
    const q = query.trim();
    if (!q) {
      loadRecords();
    } else if (q === activeQuery) {
      searchQuery.refetch();
    } else {
      setActiveQuery(q);
    }
  }

  function clearSearch() {
    setQuery("");
    loadRecords();
  }

  async function sync() {
    setSyncing(true);
    setSyncResult(null);
    setSyncError(null);
    let result: "done" | "failed" = "done";
    try {
      await syncSource.mutateAsync(kind);
    } catch (e) {
      result = "failed";
      setSyncError(failure(e, "The sync could not start."));
    } finally {
      setSyncing(false);
    }
    setSyncResult(result);
    setTimeout(() => setSyncResult((r) => (r === result ? null : r)), 1200);
  }

  if (rowsQuery.isError) {
    return (
      <div className="max-w-5xl flex flex-col gap-6">
        <Breadcrumb label={kind} />
        <ErrorLine error={failure(rowsQuery.error, "Could not load this source.", " Check that the backend is running, then reload.")} />
      </div>
    );
  }

  if (row === undefined) {
    return (
      <div className="max-w-5xl flex flex-col gap-6" aria-busy>
        <span className="skeleton h-3 w-40" />
        <div className="flex items-center gap-3">
          <span className="skeleton w-10 h-10" />
          <span className="skeleton h-5 w-40" />
        </div>
        <span className="skeleton h-20 w-full" />
        <span className="skeleton h-64 w-full" />
      </div>
    );
  }

  if (row === null) {
    return (
      <div className="max-w-5xl flex flex-col gap-6">
        <Breadcrumb label={kind} />
        <h1 className="page-title">{formatKey(kind)}</h1>
        <div className="ledger px-5 py-4 flex items-center justify-between gap-4 flex-wrap text-[13px]">
          <span style={{ color: "var(--ink-dim)" }}>This source was removed: &ldquo;{kind}&rdquo; is not a connector any more and has no records left.</span>
          <Link href="/connectors" className="btn btn-sm">
            Back to connectors
          </Link>
        </div>
      </div>
    );
  }

  const hasRecords = row.record_count > 0;
  const settingsHref = connectorKind ? `/connectors/setup/${connectorKind}${row.connector ? `?connector=${encodeURIComponent(row.connector)}` : ""}` : null;

  if (!row.connected && !hasRecords) {
    return (
      <div className="max-w-5xl flex flex-col gap-6">
        <Breadcrumb label={row.label} />
        <div className="flex items-center gap-3">
          <ConnectorTile kind={connectorKind ?? null} size={40} />
          <h1 className="page-title">{row.label}</h1>
        </div>
        <div className="ledger px-5 py-4 flex items-center justify-between gap-4 flex-wrap text-[13px]">
          <span style={{ color: "var(--ink-dim)" }}>{row.label} is not connected, so it has no records yet.</span>
          {settingsHref ? (
            <Link href={settingsHref} className="btn btn-primary btn-sm">
              Connect {row.label}
            </Link>
          ) : (
            <Link href="/connectors" className="btn btn-sm">
              Back to connectors
            </Link>
          )}
        </div>
      </div>
    );
  }

  const h = health(row);
  const syncStatus: SyncStatus = syncing ? "running" : syncResult ?? "idle";

  return (
    <div className="max-w-5xl flex flex-col gap-8">
      <header className="flex flex-col gap-3">
        <Breadcrumb label={row.label} />
        <div className="flex items-center gap-3 flex-wrap">
          <ConnectorTile kind={connectorKind ?? null} size={40} />
          <div className="flex flex-col gap-0.5 min-w-0 flex-1">
            <h1 className="page-title">{row.account ? `${row.label}: ${row.account}` : row.label}</h1>
            <span className="text-[12.5px] flex items-center gap-1.5" style={{ color: "var(--ink-dim)" }}>
              <span className="dot" style={{ background: h.tone }} aria-hidden />
              {row.connected ? "Connected" : orphaned ? "Source no longer connected, records kept" : "Seeded data, no live connection"}
            </span>
          </div>
          <div className="flex items-center gap-2">
            {settingsHref && (
              <Link href={settingsHref} className="btn">
                <Settings2 size={14} strokeWidth={1.75} aria-hidden /> Setup
              </Link>
            )}
            {!orphaned && (
              <button type="button" onClick={sync} disabled={syncing} className="btn btn-primary" aria-live="polite">
                <SyncMark status={syncStatus} />
                {syncing ? "Syncing…" : "Sync now"}
              </button>
            )}
          </div>
        </div>
      </header>

      <dl className="ledger grid grid-cols-2 md:grid-cols-4">
        {[
          { label: "Health", value: <span style={{ color: h.tone === "var(--good)" ? "var(--ink)" : h.tone }}>{h.text}</span> },
          { label: "Records", value: <span className="font-mono"><DigitRoll value={row.record_count} /></span> },
          {
            label: "Last sync",
            value: (
              <span className="font-mono" title={row.sync_status.last_ok ? formatDate(row.sync_status.last_ok) : undefined}>
                {relativeTime(row.sync_status.last_ok)}
              </span>
            ),
          },
          { label: "Writes as", value: <AuthorTag name={sourceKey} title={`Records from this connector carry the source key ${sourceKey}`} /> },
        ].map((s, i) => (
          <div
            key={s.label}
            className="px-4 py-3 flex flex-col gap-1.5 min-w-0 text-[13px]"
            style={{ borderLeft: i % 2 ? "var(--hair) solid var(--border)" : undefined, borderTop: i > 1 ? "var(--hair) solid var(--border)" : undefined }}
          >
            <dt className="label">{s.label}</dt>
            <dd className="min-w-0 truncate">{s.value}</dd>
          </div>
        ))}
      </dl>

      {(syncError || row.sync_status.last_error) && (
        <div className="flex flex-col gap-1.5">
          <ErrorLine error={syncError}>
            <span className="font-medium">{syncError ? "Sync could not start." : "The last sync failed."}</span>{" "}
            <span className="font-mono text-[12px] break-words">{syncError?.message ?? row.sync_status.last_error}</span>
          </ErrorLine>
          <p className="text-[12.5px]" style={{ color: "var(--ink-dim)" }}>
            {settingsHref ? (
              <>
                Check the credentials in the <Link href={settingsHref} className="underline" style={{ color: "var(--accent-text)" }}>connector setup</Link>, then sync again.
              </>
            ) : (
              "Try syncing again in a moment."
            )}
          </p>
        </div>
      )}

      <section className="flex flex-col gap-3" aria-labelledby="records-heading">
        {(activeQuery ? searchQuery.isError : listQuery.isError) && (
          <ErrorLine error={failure(activeQuery ? searchQuery.error : listQuery.error, activeQuery ? "Could not search records." : "Could not load records.")} />
        )}
        <div className="flex items-center justify-between gap-3 flex-wrap">
          <h2 id="records-heading" className="section-title">
            Records{" "}
            <span className="font-mono font-normal" style={{ color: "var(--ink-faint)" }}>
              {results && (activeQuery ? `${results.length} matching` : results.length > 0 ? `latest ${results.length} of ${row.record_count.toLocaleString()}` : "")}
            </span>
          </h2>
          <form onSubmit={runSearch} className="field flex items-center gap-2 h-8 px-2.5 w-full sm:w-72" role="search">
            <Search size={14} strokeWidth={1.75} color="var(--ink-faint)" aria-hidden />
            <input
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={(e) => e.key === "Escape" && query && clearSearch()}
              placeholder={`Search ${row.label}…`}
              aria-label={`Search ${row.label} records`}
              className="flex-1 min-w-0 text-[13px] bg-transparent outline-none"
            />
            {searching ? (
              <span className="label">Searching…</span>
            ) : (
              query && (
                <Tooltip label="Clear search">
                  <button type="button" aria-label="Clear search" className="btn btn-ghost btn-sm -mr-1.5 w-6 px-0" onClick={clearSearch}>
                    <X size={13} strokeWidth={1.75} aria-hidden />
                  </button>
                </Tooltip>
              )
            )}
          </form>
        </div>

        <div className="ledger overflow-hidden">
          <table className="data-table table-fixed">
            <thead>
              <tr>
                <th>Title</th>
                <th className="hidden sm:table-cell w-44">Type</th>
                <th className="w-24 text-right">Occurred</th>
              </tr>
            </thead>
            <tbody>
              {results === null &&
                Array.from({ length: 6 }, (_, i) => (
                  <tr key={i} aria-hidden>
                    <td>
                      <span className="skeleton block h-3 w-56" />
                    </td>
                    <td className="hidden sm:table-cell">
                      <span className="skeleton block h-3 w-24" />
                    </td>
                    <td>
                      <span className="skeleton block h-3 w-12 ml-auto" />
                    </td>
                  </tr>
                ))}
              {results?.map((hit) => <RecordRow key={hit.id} hit={hit} />)}
              {results !== null && results.length === 0 && (
                <tr>
                  <td colSpan={3} className="!h-auto">
                    <div className="py-4 flex items-center gap-3 flex-wrap text-[13px]" style={{ color: "var(--ink-dim)" }}>
                      {activeQuery ? (
                        <>
                          No records match &ldquo;{activeQuery}&rdquo;.
                          <button type="button" className="btn btn-sm" onClick={clearSearch}>
                            Clear search
                          </button>
                        </>
                      ) : (
                        <>No records yet. Run a sync to pull the latest from {row.label}.</>
                      )}
                    </div>
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
      </section>

      {connectorKind && <DeleteData row={row} kind={connectorKind} />}
    </div>
  );
}

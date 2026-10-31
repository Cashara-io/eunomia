"use client";

import { Fragment, useState } from "react";
import Link from "next/link";
import { ChevronRight, Plus, Search, X } from "lucide-react";
import type { Connector, SourceRow } from "@/lib/types";
import { useConnectors } from "@/lib/queries/connectors";
import { useSources } from "@/lib/queries/sources";
import { CONNECTOR_META, CONNECTOR_ORDER, ConnectorTile, connectorStatus, kindForSource, relativeTime } from "@/lib/connectorMeta";
import { isLiveSource, sourceHealth } from "@/lib/sourceState";
import ErrorLine, { failure } from "@/components/ErrorLine";
import Tooltip from "@/components/bits/Tooltip";
import ExtensionsSection from "./ExtensionsSection";
import PasswordManagersSection from "./PasswordManagersSection";

type StatusFilter = "all" | "connected" | "disconnected";

/** One row of the connected table: a connection with credentials, or a source that holds records without one (demo data). */
type ConnectedRow = {
  id: string;
  kind: Connector["kind"] | null;
  label: string;
  /** The name the user gave this connection, shown under the kind's name. */
  account: string;
  /** The last connection of its kind: an "Add another" row follows it. */
  addAfter: boolean;
  status: "connected" | "demo";
  source: SourceRow | undefined;
  href: string;
};

const CONNECTED_COLS = "grid-cols-[minmax(0,1fr)_auto] md:grid-cols-[minmax(0,1.4fr)_120px_76px_96px_minmax(0,1fr)_16px]";
const AVAILABLE_COLS = "grid-cols-[minmax(0,1fr)_auto] md:grid-cols-[minmax(0,1.6fr)_minmax(0,1fr)_96px_16px]";
const ROW_LINK =
  "grid items-center gap-x-4 px-4 min-h-12 py-2 text-[13px] transition-[background-color,transform] duration-[120ms] ease-[var(--ease-out)] hover:bg-[var(--surface-raised)] active:scale-[0.995] focus-visible:outline-offset-[-2px]";

function matches(q: string, label: string, description = "") {
  return !q || label.toLowerCase().includes(q) || description.toLowerCase().includes(q);
}

export default function ConnectorsPage() {
  const connectorsQuery = useConnectors();
  const sourcesQuery = useSources();
  const connectors = connectorsQuery.data ?? null;
  // Sources only add sync health; the page still works if that call fails.
  const sourceRows: SourceRow[] = sourcesQuery.data ?? [];
  const error = connectorsQuery.isError ? failure(connectorsQuery.error, "Could not load connectors.", " Check the backend is running, then retry.") : null;
  const [query, setQuery] = useState("");
  const [statusFilter, setStatusFilter] = useState<StatusFilter>("all");

  const load = () => {
    connectorsQuery.refetch();
    sourcesQuery.refetch();
  };

  const bySource = Object.fromEntries(sourceRows.map((s) => [s.key, s]));
  const q = query.trim().toLowerCase();

  const connected: ConnectedRow[] = [];
  const available: Connector["kind"][] = [];
  for (const kind of CONNECTOR_ORDER) {
    const meta = CONNECTOR_META[kind];
    // A kind can be connected more than once (two Gmail accounts): one row per live connection.
    const live = (connectors ?? []).filter((c) => c.kind === kind && connectorStatus(c) !== "disconnected");
    if (live.length === 0) {
      available.push(kind);
      continue;
    }
    live.forEach((c, i) => {
      const status = connectorStatus(c) as ConnectedRow["status"];
      connected.push({
        id: c.id,
        kind,
        label: meta.label,
        account: c.label,
        addAfter: i === live.length - 1,
        status,
        source: meta.sourceKey ? bySource[c.source_key] : undefined,
        href: meta.sourceKey ? `/connectors/${c.source_key}` : `/connectors/setup/${kind}?connector=${encodeURIComponent(c.id)}`,
      });
    });
  }
  // Sources that hold records but belong to no connector (seeded demo data).
  for (const s of sourceRows) {
    if (!kindForSource(s.key) && isLiveSource(s)) {
      connected.push({ id: s.key, kind: null, label: s.label, account: "", addAfter: false, status: "demo", source: s, href: `/connectors/${s.key}` });
    }
  }

  const shownConnected = connected.filter((r) => matches(q, `${r.label} ${r.account}`, r.kind ? CONNECTOR_META[r.kind].description : ""));
  const shownAvailable = available.filter((k) => matches(q, CONNECTOR_META[k].label, CONNECTOR_META[k].description));
  const loading = connectors === null && !error;

  const filters: { value: StatusFilter; label: string; count: number }[] = [
    { value: "all", label: "All", count: connected.length + available.length },
    { value: "connected", label: "Connected", count: connected.length },
    { value: "disconnected", label: "Not connected", count: available.length },
  ];

  return (
    <div className="max-w-5xl flex flex-col gap-8">
      <header className="flex flex-col gap-1.5">
        <h1 className="page-title">Connectors</h1>
        <p className="text-[13px] max-w-[62ch]" style={{ color: "var(--ink-dim)" }}>
          Sources that write records into your memory. Credentials are encrypted at rest and only ever sent to the provider they belong to.
        </p>
      </header>

      <div className="flex items-center gap-2 flex-wrap">
        <div className="field flex items-center gap-2 h-8 px-2.5 w-full sm:w-64">
          <Search size={14} strokeWidth={1.75} color="var(--ink-faint)" aria-hidden />
          <input
            type="text"
            aria-label="Search connectors"
            className="flex-1 min-w-0 bg-transparent outline-none text-[13px]"
            placeholder="Search connectors"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
          {query && (
            <Tooltip label="Clear search">
              <button type="button" aria-label="Clear search" className="btn btn-ghost btn-sm -mr-1.5 w-6 px-0" onClick={() => setQuery("")}>
                <X size={13} strokeWidth={1.75} aria-hidden />
              </button>
            </Tooltip>
          )}
        </div>
        <div className="flex gap-1.5" role="group" aria-label="Filter by status">
          {filters.map((f) => (
            <button key={f.value} type="button" className="pill" aria-pressed={statusFilter === f.value} onClick={() => setStatusFilter(f.value)}>
              {f.label}
              {!loading && (
                <span className="font-mono text-[11px]" style={{ color: "var(--ink-faint)" }}>
                  {f.count}
                </span>
              )}
            </button>
          ))}
        </div>
      </div>

      {error && (
        <div className="flex items-start gap-3">
          <div className="flex-1 min-w-0">
            <ErrorLine error={error} />
          </div>
          <button type="button" className="btn btn-sm" onClick={load}>
            Retry
          </button>
        </div>
      )}

      {statusFilter !== "disconnected" && (
        <section className="flex flex-col gap-3" aria-labelledby="connected-heading">
          <h2 id="connected-heading" className="section-title">
            Connected
          </h2>
          <div className="ledger overflow-hidden">
            <div className={`hidden md:grid ${CONNECTED_COLS} gap-x-4 px-4 h-8 items-center label border-b`} aria-hidden>
              <span>Connector</span>
              <span>Status</span>
              <span className="text-right">Records</span>
              <span>Last sync</span>
              <span>Writes</span>
              <span />
            </div>
            {loading && <SkeletonRows count={2} />}
            {!loading && shownConnected.length === 0 && (
              <p className="px-4 py-5 text-[13px]" style={{ color: "var(--ink-dim)" }}>
                {q ? `No connected source matches "${query.trim()}".` : "Nothing connected yet. Pick a source from the catalogue below to start syncing records."}
              </p>
            )}
            <div className="hairline-rows">
              {shownConnected.map((r) => {
                const s = r.source;
                const fails = s?.sync_status.consecutive_failures ?? 0;
                const health = sourceHealth(s);
                return (
                  <Fragment key={r.id}>
                  <Link href={r.href} className={`${ROW_LINK} ${CONNECTED_COLS}`}>
                    <span className="flex items-center gap-3 min-w-0">
                      <ConnectorTile kind={r.kind} />
                      <span className="min-w-0 flex flex-col">
                        <span className="font-medium truncate">{r.label}</span>
                        {r.account && (
                          <span className="text-[12px] truncate" style={{ color: "var(--ink-dim)" }}>
                            {r.account}
                          </span>
                        )}
                      </span>
                    </span>
                    <span className="flex items-center gap-2" title={s?.sync_status.last_error || undefined}>
                      <span className="dot" style={{ background: health.tone }} aria-hidden />
                      <span>{health.label}</span>
                    </span>
                    <span className="hidden md:block font-mono text-right">{s ? s.record_count.toLocaleString() : "n/a"}</span>
                    <span className="hidden md:block font-mono text-[12px]" style={{ color: fails ? "var(--critical)" : "var(--ink-dim)" }}>
                      {fails ? `${fails} failed` : s ? relativeTime(s.sync_status.last_ok) : "n/a"}
                    </span>
                    <span className="hidden md:block font-mono text-[12px] truncate" style={{ color: "var(--ink-faint)" }}>
                      {s?.record_types.join(", ") || "Tool calls only"}
                    </span>
                    <ChevronRight size={14} strokeWidth={1.75} color="var(--ink-faint)" className="hidden md:block" aria-hidden />
                  </Link>
                  {r.addAfter && r.kind && (
                    <Link
                      href={`/connectors/setup/${r.kind}?new=1`}
                      className="flex items-center gap-2 px-4 h-9 pl-14 text-[12.5px] transition-colors duration-[120ms] ease-[var(--ease-out)] hover:bg-[var(--surface-raised)]"
                      style={{ color: "var(--ink-dim)" }}
                    >
                      <Plus size={13} strokeWidth={1.75} aria-hidden />
                      Add another {r.label} connection
                    </Link>
                  )}
                  </Fragment>
                );
              })}
            </div>
          </div>
        </section>
      )}

      {statusFilter !== "disconnected" && !q && <ExtensionsSection />}

      {statusFilter !== "disconnected" && !q && <PasswordManagersSection />}

      {statusFilter !== "connected" && (
        <section className="flex flex-col gap-3" aria-labelledby="catalogue-heading">
          <h2 id="catalogue-heading" className="section-title">
            Catalogue
          </h2>
          <div className="ledger overflow-hidden">
            {loading && <SkeletonRows count={6} />}
            {!loading && shownAvailable.length === 0 && (
              <div className="px-4 py-4 text-[13px] flex items-center gap-3" style={{ color: "var(--ink-dim)" }}>
                {q ? (
                  <>
                    No connector matches &ldquo;{query.trim()}&rdquo;.
                    <button type="button" className="btn btn-sm" onClick={() => setQuery("")}>
                      Clear search
                    </button>
                  </>
                ) : (
                  "Every connector is set up."
                )}
              </div>
            )}
            <div className="hairline-rows">
              {shownAvailable.map((kind) => {
                const meta = CONNECTOR_META[kind];
                const types = meta.sourceKey ? bySource[meta.sourceKey]?.record_types : undefined;
                return (
                  <Link key={kind} href={`/connectors/setup/${kind}`} className={`${ROW_LINK} ${AVAILABLE_COLS}`}>
                    <span className="flex items-center gap-3 min-w-0">
                      <ConnectorTile kind={kind} />
                      <span className="min-w-0 flex flex-col">
                        <span className="font-medium truncate">{meta.label}</span>
                        <span className="text-[12px] truncate" style={{ color: "var(--ink-dim)" }}>
                          {meta.description}
                        </span>
                      </span>
                    </span>
                    <span className="hidden md:block font-mono text-[12px] truncate" style={{ color: "var(--ink-faint)" }}>
                      {types?.join(", ") || "Tool calls only"}
                    </span>
                    <span className="text-[12px] whitespace-nowrap" style={{ color: "var(--ink-faint)" }}>
                      Not connected
                    </span>
                    <ChevronRight size={14} strokeWidth={1.75} color="var(--ink-faint)" className="hidden md:block" aria-hidden />
                  </Link>
                );
              })}
            </div>
          </div>
        </section>
      )}
    </div>
  );
}

function SkeletonRows({ count }: { count: number }) {
  return (
    <div className="hairline-rows" aria-hidden>
      {Array.from({ length: count }, (_, i) => (
        <div key={i} className="flex items-center gap-3 px-4 h-12">
          <span className="skeleton w-7 h-7" />
          <span className="skeleton h-3 w-32" />
          <span className="skeleton h-3 w-20 ml-auto" />
        </div>
      ))}
    </div>
  );
}

"use client";

import SyncSettings from "@/components/SyncSettings";
import { Fragment, useState } from "react";
import Link from "next/link";
import { ChevronDown, ChevronRight, Database, Plug, Plus, Search, X } from "lucide-react";
import type { Connector, SourceRow } from "@/lib/types";
import { useConnectors } from "@/lib/queries/connectors";
import { useSources } from "@/lib/queries/sources";
import { CONNECTOR_META, CONNECTOR_ORDER, connectorStatus, kindForSource, relativeTime } from "@/lib/connectorMeta";
import { useMe } from "@/lib/queries/auth";
import { isLiveSource, sourceHealth } from "@/lib/sourceState";
import ErrorLine, { failure } from "@/components/ErrorLine";
import Tooltip from "@/components/bits/Tooltip";
import PageHeader from "@/components/ui/PageHeader";
import StatusLabel, { toneFromVar } from "@/components/ui/StatusLabel";
import ExtensionsSection from "./ExtensionsSection";

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
  /** "paused": disconnected on purpose, its imported items kept */
  status: "connected" | "demo" | "paused";
  source: SourceRow | undefined;
  href: string;
};

// Work tools first, consumer sources last. Any kind missing here keeps its catalogue position at the end.
const PAGE_ORDER: Connector["kind"][] = [
  "gmail",
  "google_calendar",
  "slack",
  "notion",
  "linear",
  "github",
  "todoist",
  "discord",
  "ed",
  "pocketai",
  "stripe",
  "up_bank",
  "spotify",
];
const ORDERED_KINDS = [...PAGE_ORDER.filter((k) => CONNECTOR_ORDER.includes(k)), ...CONNECTOR_ORDER.filter((k) => !PAGE_ORDER.includes(k))];

/* "up.transaction", "gcal.event" -> "transactions, events": what a source brings in, in words. */
function typeWords(types: string[]): string {
  const words = types.map((t) => {
    const w = (t.split(".").pop() ?? t).replace(/_/g, " ").trim();
    if (/s$/.test(w)) return w;
    if (/[^aeiou]y$/.test(w)) return `${w.slice(0, -1)}ies`;
    return `${w}s`;
  });
  const out = [...new Set(words)].join(", ");
  return out ? out[0].toUpperCase() + out.slice(1) : out;
}

const CONNECTED_COLS = "grid-cols-[minmax(0,1fr)_auto] md:grid-cols-[minmax(0,1.4fr)_130px_76px_110px_minmax(0,1fr)_16px]";
const ROW_LINK =
  "grid items-center gap-x-4 px-4 min-h-14 py-2 text-[14px] transition-[background-color] duration-[120ms] ease-[var(--ease-out)] hover:bg-[var(--bauhaus-app-bg)] focus-visible:outline-offset-[-2px]";

/** A source's icon in plain ink, no tinted square: the name beside it identifies the source. */
function SourceMark({ kind, size }: { kind: Connector["kind"] | null; size: number }) {
  const icon = kind ? CONNECTOR_META[kind].icon : null;
  return (
    <span aria-hidden className="inline-flex items-center justify-center shrink-0" style={{ width: size, height: size, color: "var(--ink-dim)" }}>
      {icon ?? <Database size={18} strokeWidth={1.75} />}
    </span>
  );
}

function matches(q: string, label: string, description = "") {
  return !q || label.toLowerCase().includes(q) || description.toLowerCase().includes(q);
}

export default function ConnectorsPage() {
  const connectorsQuery = useConnectors();
  const sourcesQuery = useSources();
  const connectors = connectorsQuery.data ?? null;
  // Sources only add sync health; the page still works if that call fails.
  const sourceRows: SourceRow[] = sourcesQuery.data ?? [];
  const error = connectorsQuery.isError ? failure(connectorsQuery.error, "Could not load your sources.", " Check that Eunomia is running, then try again.") : null;
  const [query, setQuery] = useState("");
  const [statusFilter, setStatusFilter] = useState<StatusFilter>("all");
  // Custom integrations are an advanced path: closed until asked for.
  const [showCustom, setShowCustom] = useState(false);
  const isAdmin = useMe().data?.is_admin ?? false;

  const load = () => {
    connectorsQuery.refetch();
    sourcesQuery.refetch();
  };

  const bySource = Object.fromEntries(sourceRows.map((s) => [s.key, s]));
  const q = query.trim().toLowerCase();

  const connected: ConnectedRow[] = [];
  const available: Connector["kind"][] = [];
  for (const kind of ORDERED_KINDS) {
    const meta = CONNECTOR_META[kind];
    // A kind can be connected more than once (two Gmail accounts): one row per live connection.
    // A disconnected connection that still has its key was paused on purpose: list it, so its data stays reachable.
    // One row per source key: two rows for the same key would show the same data twice.
    const live = (connectors ?? []).filter(
      (c, i, all) => c.kind === kind && (connectorStatus(c) !== "disconnected" || c.credentials_set) && all.findIndex((o) => o.source_key === c.source_key) === i,
    );
    if (live.length === 0) {
      available.push(kind);
      continue;
    }
    live.forEach((c, i) => {
      const cs = connectorStatus(c);
      const status: ConnectedRow["status"] = cs === "disconnected" ? "paused" : cs;
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
      <PageHeader
        icon={Plug}
        title="Connected sources"
        intro="Bring emails, meeting notes and other information into Eunomia. Sign-ins are stored encrypted."
      />

      <div className="flex items-center gap-2 flex-wrap">
        <div className="field flex items-center gap-2 h-10 px-3 w-full sm:w-72">
          <Search size={18} strokeWidth={1.75} color="var(--ink-dim)" aria-hidden />
          <input
            type="text"
            aria-label="Search sources"
            className="flex-1 min-w-0 bg-transparent outline-none text-[14px]"
            placeholder="Search sources"
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
        <div className="flex flex-wrap gap-2" role="group" aria-label="Filter by status">
          {filters.map((f) => (
            <button key={f.value} type="button" className="pill" aria-pressed={statusFilter === f.value} onClick={() => setStatusFilter(f.value)}>
              {f.label}
              {!loading && (
                <span className="tabular">{f.count}</span>
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
            Try again
          </button>
        </div>
      )}

      {statusFilter !== "disconnected" && (
        <section className="flex flex-col gap-3" aria-labelledby="connected-heading">
          <h2 id="connected-heading" className="section-title">
            Connected
          </h2>
          {loading && <SkeletonRows count={2} />}
          {!loading && shownConnected.length === 0 && (
            <p className="text-[14px]" style={{ color: "var(--ink-dim)" }}>
              {q ? `No connected source matches "${query.trim()}".` : "Nothing connected yet. Choose a source under \u201cAdd a source\u201d to bring its information in."}
            </p>
          )}
          {/* No table at all until there is a row to put in it. */}
          {!loading && shownConnected.length > 0 && (
            <div>
              <div
                className={`hidden md:grid ${CONNECTED_COLS} gap-x-4 px-4 pb-2 items-end text-[12px] font-medium border-b`}
                style={{ color: "var(--ink-dim)" }}
                aria-hidden
              >
                <span>Source</span>
                <span>Status</span>
                <span className="text-right">Items</span>
                <span>Last updated</span>
                <span>Brings in</span>
                <span />
              </div>
              <div className="hairline-rows border-b">
                {shownConnected.map((r) => {
                  const s = r.source;
                  const fails = s?.sync_status.consecutive_failures ?? 0;
                  const health = r.status === "paused" ? { tone: "var(--ink-dim)", label: "Disconnected" } : sourceHealth(s);
                  return (
                    <Fragment key={r.id}>
                    <Link href={r.href} className={`${ROW_LINK} ${CONNECTED_COLS}`}>
                      <span className="flex items-center gap-3 min-w-0">
                        <SourceMark kind={r.kind} size={24} />
                        <span className="min-w-0 flex flex-col">
                          <span className="font-medium truncate">{r.label}</span>
                          {r.account && (
                            <span className="text-[13px] truncate" style={{ color: "var(--ink-dim)" }}>
                              {r.account}
                            </span>
                          )}
                        </span>
                      </span>
                      <span title={s?.sync_status.last_error || undefined}>
                        <StatusLabel tone={toneFromVar(health.tone)}>{health.label}</StatusLabel>
                      </span>
                      <span className="hidden md:block font-mono text-right">{s ? s.record_count.toLocaleString() : "n/a"}</span>
                      <span className="hidden md:block text-[13px]" style={{ color: fails ? "var(--critical)" : "var(--ink-dim)" }}>
                        {fails ? `${fails} failed in a row` : s ? relativeTime(s.sync_status.last_ok) : "n/a"}
                      </span>
                      <span
                        className="hidden md:block text-[13px] truncate"
                        style={{ color: "var(--ink-dim)" }}
                        title={s?.record_types.length ? typeWords(s.record_types) : undefined}
                      >
                        {s?.record_types.length ? typeWords(s.record_types) : "Tool calls only"}
                      </span>
                      <ChevronRight size={16} strokeWidth={1.75} color="var(--ink-dim)" className="hidden md:block" aria-hidden />
                    </Link>
                    {r.addAfter && r.kind && (
                      <Link
                        href={`/connectors/setup/${r.kind}?new=1`}
                        className="flex items-center gap-2 px-4 min-h-10 pl-[52px] text-[13px] font-medium underline-offset-2 hover:underline"
                        style={{ color: "var(--accent-text)" }}
                      >
                        <Plus size={14} strokeWidth={1.75} aria-hidden />
                        Add another {r.label} connection
                      </Link>
                    )}
                    </Fragment>
                  );
                })}
              </div>
            </div>
          )}
        </section>
      )}

      {statusFilter !== "connected" && (
        <section className="flex flex-col gap-4" aria-labelledby="catalogue-heading">
          <h2 id="catalogue-heading" className="section-title">
            Add a source
          </h2>
          {loading && <SkeletonRows count={6} />}
          {!loading && shownAvailable.length === 0 && (
            <div className="text-[14px] flex flex-wrap items-center gap-3" style={{ color: "var(--ink-dim)" }}>
              {q ? (
                <>
                  No source matches &ldquo;{query.trim()}&rdquo;.
                  <button type="button" className="btn btn-sm" onClick={() => setQuery("")}>
                    Clear search
                  </button>
                </>
              ) : (
                "Every source is set up. Use \u201cAdd another connection\u201d above to connect a second account."
              )}
            </div>
          )}
          {/* A compact list: one row per source, each row one link. */}
          {shownAvailable.length > 0 && (
            <ul className="hairline-rows border-y" style={{ borderColor: "var(--border)" }}>
              {shownAvailable.map((kind) => {
                const meta = CONNECTOR_META[kind];
                return (
                  <li key={kind} className="min-w-0">
                    <Link
                      href={`/connectors/setup/${kind}`}
                      aria-label={`Connect ${meta.label}`}
                      className="flex items-center gap-3 px-2 py-2.5 min-w-0 transition-colors hover:bg-[var(--surface-hover)]"
                    >
                      <SourceMark kind={kind} size={24} />
                      <span className="flex-1 min-w-0 flex flex-col sm:flex-row sm:items-baseline sm:gap-3">
                        <span className="text-[14px] font-semibold shrink-0">{meta.label}</span>
                        <span className="text-[13px] truncate" style={{ color: "var(--ink-dim)" }}>
                          {meta.description}
                        </span>
                      </span>
                      <span className="btn btn-ghost btn-sm shrink-0" aria-hidden>
                        Connect
                      </span>
                    </Link>
                  </li>
                );
              })}
            </ul>
          )}
        </section>
      )}

      {isAdmin && statusFilter !== "disconnected" && !q && (
        <section className="flex flex-col gap-4">
          <div>
            <button type="button" className="btn btn-ghost" aria-expanded={showCustom} aria-controls="custom-integrations" onClick={() => setShowCustom((v) => !v)}>
              Advanced: custom integrations
              <ChevronDown size={16} strokeWidth={1.75} aria-hidden style={{ transform: showCustom ? "rotate(180deg)" : undefined }} />
            </button>
          </div>
          {showCustom && (
            <div id="custom-integrations">
              <ExtensionsSection />
            </div>
          )}
        </section>
      )}

      <SyncSettings />
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

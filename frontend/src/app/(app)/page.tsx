"use client";

import { kindColor } from "@/lib/kindColors";
import ErrorLine, { failure } from "@/components/ErrorLine";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import Link from "next/link";
import { AlertTriangle, ArrowRight, House, Upload, Users } from "lucide-react";
import type { EntityDetail, EntityKind, EntityMemory, SourceRow } from "@/lib/types";
import { entityKeys, type EntityList } from "@/lib/queries/entities";
import { call } from "@/lib/queries/client";
import { useSources, useSyncSource } from "@/lib/queries/sources";
import { getEntity, listEntities } from "@/lib/gen";
import AuthorTag, { authorColor, CursorGlyph } from "@/components/AuthorTag";
import DigitRoll from "@/components/bits/DigitRoll";
import SyncMark, { type SyncStatus } from "@/components/bits/SyncMark";
import { prefersReducedMotion } from "@/components/bits/motion";
import { kindForSource, sourceRecordLabel } from "@/lib/connectorMeta";
import PageHeader from "@/components/ui/PageHeader";
import StatusLabel from "@/components/ui/StatusLabel";
import { AI_APPS, AppMark, appForConnection, type Connection } from "@/components/connect/apps";
import { useConnections } from "@/components/connect/useConnections";
import { FAILURE_ALERT_THRESHOLD, isLiveSource, sourceHealth, sourceLabel } from "@/lib/sourceState";

/** Compact age: "now", "4m", "3h", "2d". */
function age(iso: string | null | undefined): string {
  if (!iso) return "–";
  const mins = Math.round((Date.now() - new Date(iso).getTime()) / 60000);
  if (mins < 1) return "now";
  if (mins < 60) return `${mins}m`;
  const hours = Math.round(mins / 60);
  if (hours < 24) return `${hours}h`;
  return `${Math.round(hours / 24)}d`;
}

function since(iso: string | null): string {
  if (!iso) return "never";
  const a = age(iso);
  return a === "now" ? "just now" : `${a} ago`;
}

function stamp(iso: string | null | undefined): string {
  if (!iso) return "–";
  return new Date(iso).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
}

// ---- failure banner ----

function FailureBanner({ rows, dismissed, onDismiss }: { rows: SourceRow[]; dismissed: Set<string>; onDismiss: (key: string) => void }) {
  const failing = rows.filter((r) => isLiveSource(r) && r.sync_status.consecutive_failures >= FAILURE_ALERT_THRESHOLD && !dismissed.has(r.key));
  if (failing.length === 0) return null;
  return (
    <section className="flex flex-col gap-2">
      {failing.map((s) => (
        <div
          key={s.key}
          role="alert"
          className="flex flex-wrap items-center gap-3 rounded-[2px] px-4 py-3"
          style={{ background: "var(--critical-soft)", border: "1px solid var(--critical)" }}
        >
          <AlertTriangle size={18} strokeWidth={1.75} color="var(--critical)" className="shrink-0" aria-hidden />
          <div className="flex-1 min-w-[200px]">
            <div className="text-[14px] font-semibold">
              {sourceLabel(s)} could not update {s.sync_status.consecutive_failures} times in a row
            </div>
            {s.sync_status.last_error && (
              <div className="text-[12.5px] font-mono truncate" style={{ color: "var(--ink-dim)" }}>
                {s.sync_status.last_error}
              </div>
            )}
          </div>
          <Link href={`/connectors/${s.key}`} className="btn btn-sm shrink-0">
            Check the connection
          </Link>
          <button onClick={() => onDismiss(s.key)} className="btn btn-ghost btn-sm shrink-0">
            Dismiss
          </button>
        </div>
      ))}
    </section>
  );
}

// ---- live memory ----
// The entity lives in the inspector, not a table column: graph-palette.spec
// asserts entity names are absent from the page behind the palette.

// The API returns more on each memory than lib/api.ts declares; read what is there.
type Memory = EntityMemory & { type?: string; updated_at?: string };
type LiveRow = Memory & { entityName: string; entityKind: EntityKind };

// ponytail: no "recent memories" endpoint exists, so this fans out one
// entities.get per entity on the first page. Ceiling: only memories on the
// first ENTITY_CAP entities show. Upgrade path: a backend
// `GET /api/memories?sort=-created_at&limit=n` endpoint.
const ENTITY_CAP = 24;
const ROW_LIMIT = 12;

async function loadLiveMemory(): Promise<{ rows: LiveRow[]; total: number }> {
  const list = (await call(listEntities())) as EntityList;
  const details = await Promise.all(
    list.results.slice(0, ENTITY_CAP).map((e) => call(getEntity({ path: { entity_id: e.id } })).catch(() => null) as Promise<EntityDetail | null>),
  );
  const rows = details
    .flatMap((d) => (d ? (d.memory as Memory[]).map((m) => ({ ...m, entityName: d.name, entityKind: d.kind })) : []))
    .sort((a, b) => (b.created_at ?? "").localeCompare(a.created_at ?? ""))
    .slice(0, ROW_LIMIT);
  return { rows, total: list.total };
}

const authorOf = (m: Memory) => m.owner_email || m.source || null;

// Arrival: rows new since the previous poll fade in under an author-colour wash,
// and (at most CURSOR_CAP of them) show the author's cursor for a moment.
const ARRIVAL_MS = 2400;
const WASH_MS = 1400;
const CURSOR_CAP = 3;

/** Author tag that can carry the author's cursor glyph at its corner, Figma style, without moving the tag. */
function RowAuthor({ name, id, cursor }: { name: string; id: string; cursor: boolean }) {
  return (
    <span className="relative inline-flex max-w-full">
      {cursor && (
        <span data-arrival-cursor={id} className="absolute -left-1.5 -top-2.5 pointer-events-none">
          <CursorGlyph color={authorColor(name)} size={14} />
        </span>
      )}
      <AuthorTag name={name} />
    </span>
  );
}

function LiveMemory({
  rows,
  arrived,
  selectedId,
  onSelect,
}: {
  rows: LiveRow[] | null;
  arrived: string[];
  selectedId: string | null;
  onSelect: (id: string) => void;
}) {
  const body = useRef<HTMLTableSectionElement>(null);
  // Layout effect: the new rows must start transparent on their first painted frame.
  useLayoutEffect(() => {
    const tbody = body.current;
    if (!tbody || arrived.length === 0) return;
    const reduced = prefersReducedMotion();
    const anims: Animation[] = [];
    for (const id of arrived) {
      const tr = tbody.querySelector(`[data-memory-id="${id}"]`)?.closest("tr");
      if (!tr) continue;
      const author = rows?.find((r) => r.id === id);
      const name = author ? authorOf(author) : null;
      // Movement-free, so it stays under reduced motion; only the fade is dropped.
      if (name)
        anims.push(
          tr.animate(
            [
              { backgroundColor: `color-mix(in oklab, ${authorColor(name)} 16%, transparent)` },
              { backgroundColor: `color-mix(in oklab, ${authorColor(name)} 16%, transparent)`, offset: 0.3 },
              { backgroundColor: "transparent" },
            ],
            { duration: WASH_MS, easing: "ease" },
          ),
        );
      if (!reduced) anims.push(tr.animate([{ opacity: 0.001 }, { opacity: 1 }], { duration: 200, easing: "cubic-bezier(0.23, 1, 0.32, 1)" }));
    }
    for (const glyph of tbody.querySelectorAll<HTMLElement>("[data-arrival-cursor]"))
      anims.push(
        glyph.animate(reduced ? [{ opacity: 1 }, { opacity: 1, offset: 0.92 }, { opacity: 0 }] : [{ opacity: 0 }, { opacity: 1, offset: 0.08 }, { opacity: 1, offset: 0.9 }, { opacity: 0 }], {
          duration: ARRIVAL_MS,
          fill: "forwards",
        }),
      );
    return () => anims.forEach((a) => a.cancel());
    // rows is read for authors only; a new poll always brings a new `arrived`.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [arrived]);

  const cursors = new Set(arrived.slice(0, CURSOR_CAP));
  return (
    <section className="ledger" aria-labelledby="live-memory-title">
      <div className="flex items-center justify-between gap-3 min-h-12 px-4">
        <h2 id="live-memory-title" className="section-title">
          Live memory
        </h2>
        {rows && rows.length > 0 && <span className="label">Latest {rows.length}</span>}
      </div>
      {rows === null ? (
        <div aria-busy="true" aria-label="Loading memories">
          {Array.from({ length: 6 }).map((_, i) => (
            <div key={i} className="flex items-center gap-4 h-10 px-3" style={{ borderTop: i ? "var(--hair) solid var(--border)" : undefined }}>
              <div className="skeleton h-5 w-16" />
              <div className="skeleton h-3.5 flex-1 max-w-[340px]" />
              <div className="skeleton h-3.5 w-8 ml-auto" />
            </div>
          ))}
        </div>
      ) : rows.length === 0 ? (
        <div className="px-3 py-8 text-[13px] flex flex-wrap items-center gap-x-3 gap-y-2" style={{ color: "var(--ink-dim)" }}>
          No memories yet. Ask your AI app to remember something, or add one yourself. New memories show here with who saved them.
          <Link href="/entities" className="btn btn-sm">
            Browse people &amp; topics <ArrowRight size={14} strokeWidth={1.75} aria-hidden />
          </Link>
        </div>
      ) : (
        <table className="data-table" style={{ tableLayout: "fixed" }}>
          <thead>
            <tr>
              <th className="hidden sm:table-cell" style={{ width: 112 }}>
                Author
              </th>
              <th>Memory</th>
              <th className="hidden md:table-cell" style={{ width: 160 }}>About</th>
              <th className="hidden sm:table-cell text-right" style={{ width: 56 }}>
                Age
              </th>
            </tr>
          </thead>
          <tbody
            ref={body}
            onKeyDown={(e) => {
              // Arrow keys walk the selection; no animation on keyboard moves.
              if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
              const i = rows.findIndex((r) => r.id === selectedId);
              const next = rows[Math.min(rows.length - 1, Math.max(0, i + (e.key === "ArrowDown" ? 1 : -1)))];
              if (!next) return;
              e.preventDefault();
              onSelect(next.id);
              (e.currentTarget.querySelector(`[data-memory-id="${next.id}"]`) as HTMLElement | null)?.focus();
            }}
          >
            {rows.map((m) => {
              const selected = m.id === selectedId;
              const author = authorOf(m);
              return (
                <tr
                  key={m.id}
                  onClick={() => onSelect(m.id)}
                  // A row outline, not .frame-selected: its ::after would become an extra table cell.
                  style={selected ? { background: "var(--accent-soft)", outline: "1px solid var(--accent)", outlineOffset: -1 } : undefined}
                >
                  <td className="hidden sm:table-cell overflow-hidden">{author && <RowAuthor name={author} id={m.id} cursor={cursors.has(m.id)} />}</td>
                  <td className="overflow-hidden">
                    <div className="py-2 sm:py-0 flex flex-col gap-1 min-w-0">
                      <button
                        type="button"
                        aria-pressed={selected}
                        data-memory-id={m.id}
                        onClick={(e) => {
                          e.stopPropagation();
                          onSelect(m.id);
                        }}
                        className="block w-full truncate text-left"
                        style={{ color: "var(--ink)" }}
                        title={m.text}
                      >
                        {m.text}
                      </button>
                      {/* Below sm the author and age ride under the text so the memory keeps the full width. */}
                      <span className="sm:hidden flex items-center gap-2 min-w-0">
                        {author && <RowAuthor name={author} id={m.id} cursor={cursors.has(m.id)} />}
                        <time dateTime={m.created_at} className="font-mono text-[11.5px]" style={{ color: "var(--ink-faint)" }}>
                          {age(m.created_at)}
                        </time>
                      </span>
                    </div>
                  </td>
                  <td className="hidden md:table-cell overflow-hidden">
                    <span className="flex items-center gap-1.5 min-w-0 text-[12.5px]" style={{ color: "var(--ink-dim)" }}>
                      <span className="dot" style={{ background: kindColor(m.entityKind) }} aria-hidden />
                      <span className="truncate">{m.entityName}</span>
                    </span>
                  </td>
                  <td className="hidden sm:table-cell text-right font-mono text-[12px]" style={{ color: "var(--ink-faint)" }}>
                    <time dateTime={m.created_at}>{age(m.created_at)}</time>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
    </section>
  );
}

function Inspector({ row }: { row: LiveRow }) {
  const author = authorOf(row);
  const mono = (v: string) => <span className="font-mono text-[12px]">{v}</span>;
  const updated = row.updated_at && row.updated_at.slice(0, 16) !== row.created_at?.slice(0, 16) ? row.updated_at : null;
  const fields: { label: string; value: React.ReactNode; show?: boolean }[] = [
    { label: "Author", value: author ? <AuthorTag name={author} /> : <span style={{ color: "var(--ink-faint)" }}>Not recorded</span> },
    { label: "About", value: <span className="truncate">{row.entityName}</span> },
    { label: "Kind", value: <span className="capitalize">{row.entityKind}</span> },
    { label: "Type", value: <span className="capitalize">{row.type}</span>, show: Boolean(row.type) },
    { label: "Written", value: mono(stamp(row.created_at)) },
    { label: "Updated", value: mono(stamp(updated)), show: Boolean(updated) },
    {
      label: "Source",
      value: (
        <span className="truncate" title={row.source ?? undefined}>
          {sourceRecordLabel(row.source ?? "")}
        </span>
      ),
      show: Boolean(row.source && row.source !== author),
    },
  ];

  return (
    <aside aria-label="Selected memory" className="panel p-5 flex flex-col gap-4" style={{ borderTop: "2px solid var(--bauhaus-app-blue)" }}>
      <span className="caps" style={{ color: "var(--accent-text)" }}>
        Selected memory
      </span>
      {/* Re-keyed per row so the content settles in with a short fade, the panel itself stays put. */}
      <div key={row.id} className="fade-in flex flex-col gap-4">
        <h2 className="text-[15px] font-medium leading-snug" style={{ color: "var(--ink)" }}>
          {row.text}
        </h2>
        <dl className="grid gap-y-2.5 gap-x-3 text-[13px]" style={{ gridTemplateColumns: "72px minmax(0, 1fr)" }}>
          {fields.filter((f) => f.show !== false).map(({ label, value }) => (
            <div key={label} className="contents">
              <dt className="label self-center">{label}</dt>
              <dd className="min-w-0 flex items-center">{value}</dd>
            </div>
          ))}
        </dl>
      </div>
      <Link href="/entities" className="btn btn-sm self-start">
        Open people &amp; topics <ArrowRight size={14} strokeWidth={1.75} aria-hidden />
      </Link>
    </aside>
  );
}

// ---- stats + sources ----

/** value is a DigitRoll when it is a number, or plain text ("–", "None", "4m ago"). */
function Stat({ label, value, href, children }: { label: string; value: React.ReactNode; href?: string; children?: React.ReactNode }) {
  const numeric = typeof value !== "string" || /\d/.test(value);
  const body = (
    <>
      <span className="label">{label}</span>
      <span
        className="flex items-center gap-2 text-[28px] leading-none tracking-[-0.02em] font-medium tabular"
        style={{ color: numeric ? "var(--ink)" : "var(--ink-dim)" }}
      >
        {children}
        {value}
      </span>
    </>
  );
  const cls = "flex flex-col gap-3 px-4 py-4 min-w-0 bg-[var(--surface)]";
  return href ? (
    <Link href={href} className={`${cls} transition-[background-color,transform] duration-150 ease-[var(--ease-out)] hover:bg-[var(--surface-raised)] active:scale-[0.98]`}>
      {body}
    </Link>
  ) : (
    <div className={cls}>{body}</div>
  );
}

// The full catalogue lives on /connectors; the dashboard shows a short list.
const NOT_CONNECTED_SHOWN = 6;
const SYNC_HOLD_MS = 1200;

// ---- choose your AI app ----

/** App 02: which AI app do you use? Neutral choices, nothing preselected;
 * each opens that app's own steps. */
function AppPicker({ again = false }: { again?: boolean }) {
  return (
    <section className="task-panel p-6 sm:p-8 flex flex-col gap-5" aria-labelledby="pick-title">
      <div className="flex flex-col gap-3">
        <h2 id="pick-title" className="text-[26px] sm:text-[28px] font-semibold tracking-[-0.6px] leading-tight">
          {again ? "Connect another AI app" : "Which AI app do you use?"}
        </h2>
        <p className="text-[16px] leading-relaxed">
          Choose the app you already use. We&apos;ll show only the steps for that app. You can connect more apps later.
        </p>
      </div>
      <ul className="grid gap-3 grid-cols-1 sm:grid-cols-2 xl:grid-cols-4">
        {AI_APPS.map((a) => (
          <li key={a.id}>
            <Link
              href={`/connect/${a.id}`}
              className="flex flex-col gap-1.5 h-full min-h-[76px] px-4 py-3.5 rounded-[3px] transition-colors hover:bg-[var(--bauhaus-app-bg)]"
              style={{ border: "1px solid var(--bauhaus-app-ink)" }}
            >
              <span className="flex items-center gap-2.5 text-[16px] font-semibold">
                <AppMark id={a.id} size={22} />
                {a.name}
              </span>
              <span className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
                {a.hint}
              </span>
            </Link>
          </li>
        ))}
      </ul>
      <div className="flex flex-wrap items-center justify-between gap-3 pt-1">
        <p className="text-[14px]" style={{ color: "var(--ink-dim)" }}>
          Not sure which one? Get help choosing. You can also do this later.
        </p>
        <Link href="/connect/help#choose" className="btn">
          Connection help
        </Link>
      </div>
    </section>
  );
}

/** Or start with your own information: useful before any app is connected. */
function OwnInformation() {
  return (
    <section className="flex flex-col gap-3" aria-labelledby="own-info-title">
      <h2 id="own-info-title" className="section-title">
        Or start with your own information
      </h2>
      <p className="text-[15px]" style={{ color: "var(--ink-dim)" }}>
        You can read and organise your memories and documents before connecting an AI app.
      </p>
      <div className="flex flex-wrap gap-2">
        <Link href="/documents" className="btn">
          <Upload size={18} strokeWidth={1.75} aria-hidden /> Upload a document
        </Link>
        <Link href="/entities" className="btn">
          <Users size={18} strokeWidth={1.75} aria-hidden /> Browse people &amp; topics
        </Link>
      </div>
    </section>
  );
}

const ago = (iso: string | null) => (iso ? since(iso) : "not used yet");

/** App 02.5: the connected Home. Only rendered from server-confirmed connections. */
function ConnectedPanel({ connections, onConnectAnother }: { connections: Connection[]; onConnectAnother: () => void }) {
  const latest = connections[0];
  const app = appForConnection(latest);
  return (
    <section className="p-6 flex flex-col gap-4 rounded-[2px]" style={{ background: "var(--surface)", border: "1px solid var(--bauhaus-app-blue)" }} aria-labelledby="connected-title">
      <h2 id="connected-title" className="text-[20px] font-semibold tracking-[-0.3px]">
        {connections.length === 1 ? `${latest.name} is connected` : `${connections.length} AI apps are connected`}
      </h2>
      <p className="text-[15px]" style={{ color: "var(--ink-dim)" }}>
        {app.id === "chatgpt"
          ? "Your AI app can now use Eunomia. Choose Eunomia in your next ChatGPT conversation."
          : "Your AI apps can now use Eunomia. Ask one to look for memories about you to try it."}
      </p>
      <ul className="flex flex-col gap-1.5">
        {connections.slice(0, 6).map((c, i) => (
          <li key={`${c.name}-${i}`} className="flex flex-wrap items-center gap-x-3 gap-y-1 text-[14px]">
            <span className="font-medium">{c.name}</span>
            <StatusLabel tone="good">{c.via === "sign-in" ? "Signed in" : "Using an API token"}</StatusLabel>
            <span style={{ color: "var(--ink-dim)" }}>Last used {ago(c.at)}</span>
          </li>
        ))}
        {connections.length > 6 && (
          <li className="text-[14px]" style={{ color: "var(--ink-dim)" }}>
            and {connections.length - 6} more
          </li>
        )}
      </ul>
      <div className="flex flex-wrap gap-2">
        {app.openUrl && connections.length === 1 && (
          <a href={app.openUrl} target="_blank" rel="noreferrer" className="btn btn-primary">
            Open {app.name}
          </a>
        )}
        <Link href="/settings?tab=apps" className="btn">
          Manage connected apps
        </Link>
        <button type="button" className="btn btn-ghost" onClick={onConnectAnother}>
          Connect another app
        </button>
      </div>
    </section>
  );
}

export default function HomePage() {
  // Live: rows and counts refresh in place on fixed layouts, nothing jumps.
  const sourcesQuery = useSources({ refetchInterval: 60_000 });
  const rows = sourcesQuery.data ?? (sourcesQuery.isError ? [] : null);
  const liveQuery = useQuery({ queryKey: entityKeys.live(), queryFn: loadLiveMemory, refetchInterval: 60_000 });
  const live = liveQuery.data?.rows ?? (liveQuery.isError ? [] : null);
  const entityCount = liveQuery.data?.total ?? (liveQuery.isError ? 0 : null);
  const syncSource = useSyncSource();
  const [selectedId, setSelectedId] = useState<string | null>(null);
  // One sync mark at a time: running, then done or failed held for SYNC_HOLD_MS.
  const [syncMark, setSyncMark] = useState<{ key: string; status: SyncStatus } | null>(null);
  const syncHold = useRef<ReturnType<typeof setTimeout>>(undefined);
  const [arrived, setArrived] = useState<string[]>([]);
  const knownIds = useRef<Set<string> | null>(null);
  const [dismissedFailures, setDismissedFailures] = useState<Set<string>>(new Set());
  const { connections } = useConnections();
  const [picking, setPicking] = useState(false);

  const arrivalTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const liveRows = liveQuery.data?.rows;
  useEffect(() => {
    if (!liveRows) return;
    // Never on first load: only rows unseen by an earlier poll count as arrivals.
    const known = knownIds.current;
    if (known) {
      const fresh = liveRows.filter((m) => !known.has(m.id)).map((m) => m.id);
      if (fresh.length) {
        setArrived(fresh);
        clearTimeout(arrivalTimer.current);
        arrivalTimer.current = setTimeout(() => setArrived([]), ARRIVAL_MS);
      }
    }
    knownIds.current = new Set([...(known ?? []), ...liveRows.map((m) => m.id)]);
  }, [liveRows]);
  useEffect(
    () => () => {
      clearTimeout(arrivalTimer.current);
      clearTimeout(syncHold.current);
    },
    [],
  );

  async function sync(key: string) {
    clearTimeout(syncHold.current);
    setSyncMark({ key, status: "running" });
    let status: SyncStatus = "done";
    try {
      // offline: nothing was attempted, so it must not read as a successful sync
      if ((await syncSource.mutateAsync(key)).status === "offline") status = "failed";
    } catch {
      status = "failed";
    }
    setSyncMark({ key, status });
    syncHold.current = setTimeout(() => setSyncMark(null), SYNC_HOLD_MS);
  }

  const connected = rows?.filter(isLiveSource) ?? [];
  const disconnected = rows?.filter((r) => !isLiveSource(r)) ?? [];
  const healthy = connected.filter((r) => sourceHealth(r).label === "Healthy").length;
  const totalRecords = rows?.reduce((sum, r) => sum + r.record_count, 0) ?? 0;
  const lastSyncs = connected.map((r) => r.sync_status.last_ok).filter((d): d is string => Boolean(d));
  const lastSync = lastSyncs.length ? lastSyncs.sort().at(-1)! : null;
  const selected = live?.find((m) => m.id === selectedId) ?? live?.[0] ?? null;
  // Connected only on the server's word: an approved sign-in or a token an app has used.
  const connected_apps = connections !== null && connections.length > 0;

  return (
    <div className="flex flex-col gap-8 max-w-[1240px]">
      <PageHeader
        icon={House}
        title="Home"
        intro={connected_apps ? "Your memory, connected across your AI apps." : "Connect an AI app, or start browsing your saved information."}
      />

      {connections === null ? (
        <div className="task-panel p-8 flex flex-col gap-4" aria-busy="true" aria-label="Checking your connected apps">
          <div className="skeleton h-7 w-72" />
          <div className="skeleton h-4 w-full max-w-[520px]" />
        </div>
      ) : !connected_apps ? (
        <>
          <AppPicker />
          <OwnInformation />
        </>
      ) : picking ? (
        <>
          <AppPicker again />
          <button type="button" className="btn self-start" onClick={() => setPicking(false)}>
            Back to Home
          </button>
        </>
      ) : (
        <ConnectedPanel connections={connections} onConnectAnother={() => setPicking(true)} />
      )}

      {rows && (
        <FailureBanner
          rows={rows}
          dismissed={dismissedFailures}
          onDismiss={(key) => setDismissedFailures((prev) => new Set(prev).add(key))}
        />
      )}

      {connected_apps && !picking && (
      <>
      {sourcesQuery.isError && <ErrorLine error={failure(sourcesQuery.error, "Could not load your sources.")} />}
      {liveQuery.isError && <ErrorLine error={failure(liveQuery.error, "Could not load recent memory.")} />}

      <section
        aria-label="Summary"
        className="grid grid-cols-2 lg:grid-cols-4 overflow-hidden rounded-[2px]"
        style={{ gap: "var(--hair)", background: "var(--border)", border: "var(--hair) solid var(--border)" }}
      >
        <Stat label="People & topics" value={entityCount !== null ? <DigitRoll value={entityCount} /> : "–"} href="/entities" />
        <Stat label="Records" value={rows ? <DigitRoll value={totalRecords} /> : "–"} />
        <Stat
          label="Sources healthy"
          value={!rows ? "–" : connected.length ? <DigitRoll value={healthy} format={(v) => `${v}/${connected.length}`} /> : "None"}
        >
          {rows && connected.length > 0 && (
            <span className="dot" aria-hidden style={{ background: healthy === connected.length ? "var(--good)" : "var(--warning)" }} />
          )}
        </Stat>
        <Stat label="Last sync" value={lastSync ? since(lastSync) : "Never"} />
      </section>

      {/* Mobile order: live memory, sources, then inspector and connect. From lg the right column is the sticky inspector. */}
      <div className="grid gap-x-6 gap-y-8 lg:grid-cols-[minmax(0,1fr)_320px] lg:grid-rows-[auto_1fr] items-start">
        <div className="min-w-0 lg:col-start-1 lg:row-start-1">
          <LiveMemory rows={live} arrived={arrived} selectedId={selected?.id ?? null} onSelect={setSelectedId} />
        </div>

        <div className="order-3 lg:order-none flex flex-col gap-8 min-w-0 lg:col-start-2 lg:row-start-1 lg:row-span-2 lg:sticky lg:top-8">
          {selected && <Inspector row={selected} />}
        </div>

        <div className="order-2 lg:order-none flex flex-col gap-8 min-w-0 lg:col-start-1 lg:row-start-2">
          <section className="flex flex-col gap-3" aria-labelledby="connected-sources">
            <h2 id="connected-sources" className="section-title">
              Connected sources
            </h2>
            {rows === null ? (
              <div className="ledger p-3 flex flex-col gap-3" aria-busy="true">
                <div className="skeleton h-4 w-1/2" />
                <div className="skeleton h-4 w-1/3" />
              </div>
            ) : connected.length === 0 ? (
              <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
                No sources connected yet. Pick one under &ldquo;Add a source&rdquo; below.
              </p>
            ) : (
              <div className="ledger overflow-x-auto relative">
                {/* relative: keeps the sr-only labels inside the scroll box, or they widen the page on mobile. */}
                <table className="data-table">
                  <thead>
                    <tr>
                      <th>Source</th>
                      <th className="text-right">Records</th>
                      <th className="hidden sm:table-cell">Last sync</th>
                      <th style={{ width: 1 }}>
                        <span className="sr-only">Actions</span>
                      </th>
                    </tr>
                  </thead>
                  <tbody>
                    {connected.map((s) => {
                      const st = s.sync_status;
                      const mark = syncMark?.key === s.key ? syncMark.status : "idle";
                      const busy = mark === "running";
                      const health = sourceHealth(s);
                      return (
                        <tr key={s.key}>
                          <td>
                            <div className="flex items-center gap-2 min-w-0">
                              <span className="dot" title={health.label} style={{ background: health.tone }} />
                              <span className="sr-only">{health.label}:</span>
                              <span className="font-medium truncate">{sourceLabel(s)}</span>
                              <span className="hidden sm:inline-flex">
                                <AuthorTag name={s.key} />
                              </span>
                            </div>
                          </td>
                          <td className="text-right font-mono text-[12.5px]">
                            <DigitRoll value={s.record_count} />
                          </td>
                          <td className="hidden sm:table-cell text-[12.5px] whitespace-nowrap">
                            <span className="font-mono" style={{ color: "var(--ink-dim)" }}>
                              {since(st.last_ok)}
                            </span>
                            {st.consecutive_failures > 0 && <span style={{ color: "var(--critical)" }}> · {st.consecutive_failures} failed</span>}
                          </td>
                          <td className="text-right">
                            <button onClick={() => sync(s.key)} disabled={busy} className="btn btn-sm">
                              <SyncMark status={mark} size={12} />
                              {busy ? "Syncing…" : "Sync now"}
                            </button>
                          </td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              </div>
            )}
          </section>

          {disconnected.length > 0 && (
            <section className="flex flex-col gap-3" aria-labelledby="add-source">
              <h2 id="add-source" className="section-title">
                Add a source
              </h2>
              <ul className="ledger hairline-rows">
                {disconnected.slice(0, NOT_CONNECTED_SHOWN).map((s) => {
                  const kind = kindForSource(s.key);
                  return (
                    <li key={s.key} className="flex items-center gap-3 min-h-11 px-3 py-2">
                      <span className="dot" aria-hidden style={{ background: "var(--border-strong)" }} />
                      <div className="flex-1 min-w-0 flex flex-col sm:flex-row sm:items-center gap-x-3 gap-y-0.5">
                        <span className="text-[13px] font-medium truncate">{sourceLabel(s)}</span>
                        <span className="text-[12px] font-mono truncate" style={{ color: "var(--ink-faint)" }}>
                          {s.record_types.join(", ")}
                        </span>
                      </div>
                      <Link href={kind ? `/connectors/setup/${kind}` : "/connectors"} className="btn btn-sm shrink-0">
                        Connect <ArrowRight size={12} strokeWidth={1.75} />
                      </Link>
                    </li>
                  );
                })}
                {disconnected.length > NOT_CONNECTED_SHOWN && (
                  <li className="flex items-center min-h-11 px-3">
                    <Link href="/connectors" className="text-[13px] inline-flex items-center gap-1.5 underline-offset-2 hover:underline" style={{ color: "var(--accent-text)" }}>
                      See all {disconnected.length} in Connected sources <ArrowRight size={14} strokeWidth={1.75} aria-hidden />
                    </Link>
                  </li>
                )}
              </ul>
            </section>
          )}
        </div>
      </div>
      </>
      )}
    </div>
  );
}

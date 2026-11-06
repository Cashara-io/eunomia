"use client";

import { kindColor } from "@/lib/kindColors";
import ErrorLine, { failure } from "@/components/ErrorLine";
import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import Link from "next/link";
import { AlertTriangle, ArrowRight, Check, Plus, Upload, Users } from "lucide-react";
import type { EntityDetail, EntityKind, SourceRow } from "@/lib/types";
import { entityKeys, type EntityList } from "@/lib/queries/entities";
import { call } from "@/lib/queries/client";
import { useSources } from "@/lib/queries/sources";
import { useVaults } from "@/lib/queries/vaults";
import { getEntity, listEntities } from "@/lib/gen";
import PageHeader from "@/components/ui/PageHeader";
import InvitationBanner from "@/components/vaults/InvitationBanner";
import AppChoices from "@/components/connect/AppChoices";
import { AppMark, appForConnection, type Connection } from "@/components/connect/apps";
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

function since(iso: string | null | undefined): string {
  if (!iso) return "never";
  const a = age(iso);
  return a === "now" ? "just now" : `${a} ago`;
}

const plural = (n: number, one: string, many: string) => `${n.toLocaleString()} ${n === 1 ? one : many}`;

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
              <div className="text-[12px] font-mono truncate" style={{ color: "var(--ink-dim)" }}>
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

// ---- overview data ----

// ponytail: no "recent activity" endpoint exists, so this fans out one
// entities.get per entity on the first page. Ceiling: memory counts and recency
// only cover the first ENTITY_CAP entities (the count then reads "N+").
// Upgrade path: a backend summary endpoint with totals and recent entities.
const ENTITY_CAP = 24;
const RECENT_SHOWN = 6;
// Code-map entities are not what a team member means by "people and projects".
const CODE_KINDS = new Set<EntityKind>(["repository", "file", "symbol"]);

type RecentEntity = { id: string; name: string; kind: EntityKind; memories: number; lastAt: string | null };
type Overview = { entities: number; memories: number; memoriesCapped: boolean; lastActivity: string | null; recent: RecentEntity[] };

async function loadOverview(): Promise<Overview> {
  const list = (await call(listEntities())) as EntityList;
  const firstPage = list.results.slice(0, ENTITY_CAP);
  const details = await Promise.all(
    firstPage.map((e) => call(getEntity({ path: { entity_id: e.id } })).catch(() => null) as Promise<EntityDetail | null>),
  );
  const loaded = details.filter((d): d is EntityDetail => d !== null);
  const withTimes = loaded.map((d) => ({
    id: d.id,
    name: d.name,
    kind: d.kind,
    memories: d.memory.length,
    lastAt: d.memory.reduce<string | null>((max, m) => (m.created_at && (!max || m.created_at > max) ? m.created_at : max), null),
  }));
  const lastActivity = withTimes.reduce<string | null>((max, e) => (e.lastAt && (!max || e.lastAt > max) ? e.lastAt : max), null);
  const recent = withTimes
    .filter((e) => !CODE_KINDS.has(e.kind))
    .sort((a, b) => (b.lastAt ?? "").localeCompare(a.lastAt ?? ""))
    .slice(0, RECENT_SHOWN);
  return {
    entities: list.total,
    memories: withTimes.reduce((sum, e) => sum + e.memories, 0),
    memoriesCapped: list.total > firstPage.length,
    lastActivity,
    recent,
  };
}

// ---- overview ----

function StatLine({ spaces, overview }: { spaces: number | null; overview: Overview }) {
  const parts = [
    spaces !== null ? plural(spaces, "space", "spaces") : null,
    plural(overview.entities, "person or topic", "people & topics"),
    `${overview.memories.toLocaleString()}${overview.memoriesCapped ? "+" : ""} ${overview.memories === 1 && !overview.memoriesCapped ? "memory" : "memories"}`,
    `last activity ${since(overview.lastActivity)}`,
  ].filter(Boolean);
  return (
    <p className="text-[14px] tabular" style={{ color: "var(--ink-dim)" }} aria-label="Summary">
      {parts.join(" · ")}
    </p>
  );
}

function RecentlyUpdated({ rows }: { rows: RecentEntity[] }) {
  return (
    <section className="flex flex-col gap-2 min-w-0" aria-labelledby="recent-title">
      <div className="flex items-center justify-between gap-3">
        <h2 id="recent-title" className="section-title">
          Recently updated
        </h2>
        <Link href="/entities" className="text-[13px] inline-flex items-center gap-1 underline-offset-2 hover:underline" style={{ color: "var(--accent-text)" }}>
          All people &amp; topics <ArrowRight size={14} strokeWidth={1.75} aria-hidden />
        </Link>
      </div>
      {rows.length === 0 ? (
        <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
          No people or projects yet.
        </p>
      ) : (
        <ul className="hairline-rows border-t">
          {rows.map((e) => (
            <li key={e.id}>
              <Link href={`/entities?id=${encodeURIComponent(e.id)}`} className="flex items-center gap-3 min-h-11 py-2 hover:bg-[var(--surface-raised)]">
                <span className="dot shrink-0" style={{ background: kindColor(e.kind) }} aria-hidden />
                <span className="flex-1 min-w-0 truncate text-[14px] font-medium">{e.name}</span>
                <span className="shrink-0 text-[12px]" style={{ color: "var(--ink-dim)" }}>
                  {plural(e.memories, "memory", "memories")}
                  {e.lastAt && <> · {since(e.lastAt)}</>}
                </span>
              </Link>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

function SourcesHealth({ rows }: { rows: SourceRow[] }) {
  return (
    <section className="flex flex-col gap-2 min-w-0" aria-labelledby="sources-title">
      <div className="flex items-center justify-between gap-3">
        <h2 id="sources-title" className="section-title">
          Connected sources
        </h2>
        <Link href="/connectors" className="text-[13px] inline-flex items-center gap-1 underline-offset-2 hover:underline" style={{ color: "var(--accent-text)" }}>
          Manage <ArrowRight size={14} strokeWidth={1.75} aria-hidden />
        </Link>
      </div>
      <ul className="hairline-rows border-t">
        {rows.map((s) => {
          const health = sourceHealth(s);
          return (
            <li key={s.key}>
              <Link href={`/connectors/${s.key}`} className="flex items-center gap-3 min-h-11 py-2 hover:bg-[var(--surface-raised)]">
                <span className="dot shrink-0" style={{ background: health.tone }} aria-hidden />
                <span className="flex-1 min-w-0 truncate text-[14px] font-medium">{sourceLabel(s)}</span>
                <span className="shrink-0 text-[12px]" style={{ color: health.label === "Healthy" ? "var(--ink-dim)" : health.tone }}>
                  {health.label} · {plural(s.record_count, "record", "records")} · synced {since(s.sync_status.last_ok)}
                </span>
              </Link>
            </li>
          );
        })}
      </ul>
    </section>
  );
}

// ---- choose your AI app ----

/** Which AI app do you use? Neutral choices, nothing preselected; each opens that app's own steps. */
function AppPicker({ again = false }: { again?: boolean }) {
  return (
    <section className="task-panel p-6 sm:p-8 flex flex-col gap-5" aria-labelledby="pick-title">
      <div className="flex flex-col gap-2">
        <h2 id="pick-title" className="text-[20px] font-semibold tracking-[-0.3px] leading-tight">
          {again ? "Connect another AI app" : "Which AI app do you use?"}
        </h2>
        <p className="text-[14px] leading-relaxed" style={{ color: "var(--ink-dim)" }}>
          Choose the app you already use. We&apos;ll show only the steps for that app. You can connect more apps later.
        </p>
      </div>
      <AppChoices />
      <div className="flex flex-wrap items-center justify-between gap-3">
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

/** Or start with your own information: shown while there is nothing saved yet. */
function OwnInformation() {
  return (
    <section className="flex flex-col gap-3" aria-labelledby="own-info-title">
      <h2 id="own-info-title" className="section-title">
        Or start with your own information
      </h2>
      <p className="text-[14px]" style={{ color: "var(--ink-dim)" }}>
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

/** Used in the last day: shown as active. */
const recentlyUsed = (iso: string | null) => Boolean(iso && Date.now() - new Date(iso).getTime() < 86_400_000);

/** The connected-apps card. Only rendered from server-confirmed connections. */
function ConnectedPanel({ connections, onConnectAnother }: { connections: Connection[]; onConnectAnother: () => void }) {
  const latest = connections[0];
  const app = appForConnection(latest);
  const shown = connections.slice(0, 6);
  return (
    <section className="panel flex flex-col overflow-hidden" aria-labelledby="connected-title">
      <div className="p-5 sm:p-6 flex flex-col sm:flex-row sm:items-center gap-4 border-b">
        <span
          className="shrink-0 grid place-items-center w-10 h-10 rounded-full"
          style={{ background: "color-mix(in srgb, var(--good) 14%, transparent)", color: "var(--good)" }}
          aria-hidden
        >
          <Check size={20} strokeWidth={2.25} />
        </span>
        <div className="flex-1 min-w-0 flex flex-col gap-1">
          <h2 id="connected-title" className="text-[20px] font-semibold tracking-[-0.3px]">
            {connections.length === 1 ? `${latest.name} is connected` : `${connections.length} AI apps are connected`}
          </h2>
          <p className="text-[14px]" style={{ color: "var(--ink-dim)" }}>
            {app.id === "chatgpt" && connections.length === 1
              ? "Choose Eunomia in your next ChatGPT conversation."
              : "Ask any of them to look for memories about you to try it."}
          </p>
        </div>
        <div className="flex flex-wrap gap-2 shrink-0">
          {app.openUrl && connections.length === 1 && (
            <a href={app.openUrl} target="_blank" rel="noreferrer" className="btn btn-primary btn-sm">
              Open {app.name}
            </a>
          )}
          <button type="button" className="btn btn-sm" onClick={onConnectAnother}>
            <Plus size={16} strokeWidth={1.75} aria-hidden /> Connect another app
          </button>
        </div>
      </div>
      <ul className="grid sm:grid-cols-2" aria-label="Connected AI apps">
        {shown.map((c, i) => {
          const active = recentlyUsed(c.at);
          return (
            <li key={`${c.name}-${i}`} className="flex items-center gap-3 px-5 sm:px-6 py-3 min-w-0 border-b sm:[&:nth-child(odd)]:border-r">
              <span className="shrink-0 grid place-items-center w-9 h-9 rounded-[6px]" style={{ background: "var(--canvas)", color: "var(--ink)" }} aria-hidden>
                <AppMark id={appForConnection(c).id} size={18} />
              </span>
              <span className="flex-1 min-w-0 flex flex-col">
                <span className="font-medium truncate">{c.name}</span>
                <span className="text-[13px] truncate" style={{ color: "var(--ink-dim)" }}>
                  {c.via === "sign-in" ? "Signed in" : "API token"} · {c.at ? `used ${ago(c.at)}` : "not used yet"}
                </span>
              </span>
              <span
                className="shrink-0 w-2 h-2 rounded-full"
                style={{ background: active ? "var(--good)" : "var(--ink-faint)" }}
                title={active ? "Used in the last day" : "Not used in the last day"}
                aria-label={active ? "Active" : "Idle"}
              />
            </li>
          );
        })}
      </ul>
      <div className="px-5 sm:px-6 py-3 flex items-center justify-between gap-3 text-[14px]">
        <span style={{ color: "var(--ink-dim)" }}>{connections.length > 6 ? `and ${connections.length - 6} more` : ""}</span>
        <Link href="/settings?tab=apps" className="inline-flex items-center gap-1 font-medium underline-offset-2 hover:underline" style={{ color: "var(--accent-text)" }}>
          Manage connected apps <ArrowRight size={14} strokeWidth={1.75} aria-hidden />
        </Link>
      </div>
    </section>
  );
}

export default function HomePage() {
  const sourcesQuery = useSources({ refetchInterval: 60_000 });
  const rows = sourcesQuery.data ?? (sourcesQuery.isError ? [] : null);
  const overviewQuery = useQuery({ queryKey: [...entityKeys.live(), "overview"], queryFn: loadOverview, refetchInterval: 60_000 });
  const overview = overviewQuery.data ?? null;
  const vaults = useVaults().data;
  const [dismissedFailures, setDismissedFailures] = useState<Set<string>>(new Set());
  const { connections } = useConnections();
  const [picking, setPicking] = useState(false);

  const liveSources = rows?.filter(isLiveSource) ?? [];
  // Org spaces are what the team shares; every account also has its personal space.
  const spaceCount = vaults ? vaults.length : null;
  const hasData = Boolean(overview && (overview.entities > 0 || overview.memories > 0)) || liveSources.length > 0 || vaults?.some((v) => v.kind === "org");
  // Connected only on the server's word: an approved sign-in or a token an app has used.
  const connectedApps = connections !== null && connections.length > 0;

  return (
    <div className="flex flex-col gap-8 max-w-[1240px]">
      <PageHeader title="Home" intro="Your team's shared memory. ChatGPT, Claude and other AI apps can look things up here." />

      <InvitationBanner />

      {rows && (
        <FailureBanner rows={rows} dismissed={dismissedFailures} onDismiss={(key) => setDismissedFailures((prev) => new Set(prev).add(key))} />
      )}

      {overviewQuery.isError && <ErrorLine error={failure(overviewQuery.error, "Could not load what your team has saved.")} />}

      {overview === null && !overviewQuery.isError ? (
        <div className="flex flex-col gap-3" aria-busy="true" aria-label="Loading your overview">
          <div className="skeleton h-4 w-full max-w-[440px]" />
          <div className="skeleton h-4 w-2/3 max-w-[320px]" />
        </div>
      ) : (
        hasData &&
        overview && (
          <section className="flex flex-col gap-6" aria-labelledby="overview-title">
            <div className="flex flex-col gap-1">
              <h2 id="overview-title" className="section-title">
                What your team knows
              </h2>
              <StatLine spaces={spaceCount} overview={overview} />
            </div>
            <div className={`grid gap-x-10 gap-y-8 ${liveSources.length > 0 ? "lg:grid-cols-2" : ""}`}>
              <RecentlyUpdated rows={overview.recent} />
              {liveSources.length > 0 && <SourcesHealth rows={liveSources} />}
            </div>
          </section>
        )
      )}

      {connections === null ? (
        <div className="task-panel p-8 flex flex-col gap-4" aria-busy="true" aria-label="Checking your connected apps">
          <div className="skeleton h-7 w-72" />
          <div className="skeleton h-4 w-full max-w-[520px]" />
        </div>
      ) : !connectedApps ? (
        <AppPicker />
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

      {overview && !hasData && !connectedApps && <OwnInformation />}
    </div>
  );
}

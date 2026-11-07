"use client";

import { kindColor } from "@/lib/kindColors";
import ErrorLine, { failure } from "@/components/ErrorLine";
import { useState } from "react";
import { useQueries, useQuery } from "@tanstack/react-query";
import Link from "next/link";
import { AlertTriangle, ArrowRight, Check, Upload, Users } from "lucide-react";
import type { EntityDetail, EntityKind, SourceRow, Vault } from "@/lib/types";
import { entityKeys, type EntityList } from "@/lib/queries/entities";
import { call } from "@/lib/queries/client";
import { useSources } from "@/lib/queries/sources";
import { useDocuments } from "@/lib/queries/documents";
import { vaultKeys, useVaults, type PendingInvite } from "@/lib/queries/vaults";
import { useMe } from "@/lib/queries/auth";
import { adminKeys } from "@/lib/queries/admin";
import { settingsKeys } from "@/lib/queries/settings";
import { getEntity, getHttpsStatus, listEntities, listPeople, listVaultPendingInvitations } from "@/lib/gen";
import type { HttpsStatus } from "@/lib/types";
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
          className="flex flex-wrap items-center gap-3 rounded-[12px] px-4 py-3"
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

// ponytail: no "recent activity" endpoint exists, so this lists each space and fans out
// one entities.get per entity on its first page. Ceiling: memory counts and recency only
// cover about ENTITY_CAP entities across all spaces (past that, the memory count is left out
// rather than shown as an estimate).
// Upgrade path: a backend summary endpoint with totals and recent entities.
const ENTITY_CAP = 24;
const RECENT_SHOWN = 6;
// Code-map entities are not what someone means by "people and projects".
const CODE_KINDS = new Set<EntityKind>(["repository", "file", "symbol"]);

type RecentEntity = { id: string; name: string; kind: EntityKind; memories: number; lastAt: string | null; vault: Vault };
type Overview = { entities: number; memories: number; memoriesCapped: boolean; lastActivity: string | null; recent: RecentEntity[] };

/** Totals and recent entities summed over every space the caller belongs to (one list call per space). */
async function loadOverview(vaults: Vault[]): Promise<Overview> {
  // No space list (it failed to load): fall back to the personal space, which is the default.
  const spaces: Vault[] = vaults.length > 0 ? vaults : [{ id: "", name: "Personal", kind: "personal", created_at: null, role: "owner" }];
  const lists = await Promise.all(
    spaces.map(async (v) => ({
      vault: v,
      list: (await call(listEntities(v.id ? { query: { vault_id: v.id } } : undefined))) as EntityList,
    })),
  );
  const perSpace = Math.max(4, Math.ceil(ENTITY_CAP / Math.max(1, lists.length)));
  const firstPages = lists.map(({ vault, list }) => ({ vault, rows: list.results.slice(0, perSpace) }));
  const details = await Promise.all(
    firstPages.flatMap(({ vault, rows }) =>
      rows.map((e) =>
        (call(getEntity({ path: { entity_id: e.id } })) as Promise<EntityDetail>).then(
          (d) => ({ d, vault }),
          () => null,
        ),
      ),
    ),
  );
  const loaded = details.filter((x): x is { d: EntityDetail; vault: Vault } => x !== null);
  const withTimes = loaded.map(({ d, vault }) => ({
    id: d.id,
    name: d.name,
    kind: d.kind,
    vault,
    memories: d.memory.length,
    lastAt: d.memory.reduce<string | null>((max, m) => (m.created_at && (!max || m.created_at > max) ? m.created_at : max), null),
  }));
  const lastActivity = withTimes.reduce<string | null>((max, e) => (e.lastAt && (!max || e.lastAt > max) ? e.lastAt : max), null);
  const recent = withTimes
    .filter((e) => !CODE_KINDS.has(e.kind))
    .sort((a, b) => (b.lastAt ?? "").localeCompare(a.lastAt ?? ""))
    .slice(0, RECENT_SHOWN);
  const total = lists.reduce((sum, { list }) => sum + list.total, 0);
  return {
    entities: total,
    memories: withTimes.reduce((sum, e) => sum + e.memories, 0),
    memoriesCapped: lists.some(({ list }) => list.total > perSpace),
    lastActivity,
    recent,
  };
}

// ---- overview ----

function StatLine({ spaces, overview }: { spaces: number | null; overview: Overview }) {
  const parts = [
    spaces !== null ? plural(spaces, "space", "spaces") : null,
    plural(overview.entities, "person or topic", "people & topics"),
    // Only an exact count: a partial sum would read as a guess.
    overview.memoriesCapped ? null : plural(overview.memories, "memory", "memories"),
    `last activity ${since(overview.lastActivity)}`,
  ].filter(Boolean);
  return (
    <p className="text-[14px] tabular" style={{ color: "var(--ink-dim)" }} aria-label="Summary">
      {parts.join(" · ")}
    </p>
  );
}

// A list row: dot, name, metadata. Below 640px the name takes the whole first line
// (wrapping, never cut off) and the metadata moves, whole, to a second line.
const ROW = "grid grid-cols-[auto_minmax(0,1fr)] sm:grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-x-3 gap-y-0.5 min-h-11 py-2 hover:bg-[var(--surface-raised)]";
const ROW_NAME = "min-w-0 text-[14px] font-medium [overflow-wrap:anywhere] sm:truncate";
const ROW_META = "col-start-2 sm:col-start-3 text-[12px] whitespace-nowrap";

const entityHref = (e: RecentEntity) =>
  `/entities?id=${encodeURIComponent(e.id)}${e.vault.kind === "personal" ? "" : `&vault=${encodeURIComponent(e.vault.id)}`}`;
const spaceName = (v: Vault) => (v.kind === "personal" ? "Personal" : v.name);

function RecentlyUpdated({ rows, showSpace }: { rows: RecentEntity[]; showSpace: boolean }) {
  return (
    <section className="flex flex-col gap-2 min-w-0" aria-labelledby="recent-title">
      <div className="flex items-center justify-between gap-3">
        <h2 id="recent-title" className="section-title">
          Recently updated
        </h2>
        <Link href="/entities" className="link text-[13px] inline-flex items-center gap-1">
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
              <Link href={entityHref(e)} className={ROW}>
                <span className="dot shrink-0" style={{ background: kindColor(e.kind) }} aria-hidden />
                <span className={ROW_NAME}>
                  {e.name}
                  {/* the space right by the name, so the same name in two spaces reads as two things */}
                  {showSpace && <span className="space-chip ml-2">{spaceName(e.vault)}</span>}
                </span>
                <span className={ROW_META} style={{ color: "var(--ink-dim)" }}>
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
        <Link href="/connectors" className="link text-[13px] inline-flex items-center gap-1">
          Manage <ArrowRight size={14} strokeWidth={1.75} aria-hidden />
        </Link>
      </div>
      <ul className="hairline-rows border-t">
        {rows.map((s) => {
          const health = sourceHealth(s);
          return (
            <li key={s.key}>
              <Link href={`/connectors/${s.key}`} className={ROW}>
                <span className="dot shrink-0" style={{ background: health.tone }} aria-hidden />
                <span className={ROW_NAME}>{sourceLabel(s)}</span>
                <span className={ROW_META} style={{ color: health.label === "Healthy" ? "var(--ink-dim)" : health.tone }}>
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

// ---- admin signals ----

/** Admins only: one quiet line of things worth a look (unanswered space invitations, open sign-up,
 * HTTPS off), each a link to where it is changed. Renders nothing when none apply. */
function AdminSignals({ vaults }: { vaults: Vault[] | undefined }) {
  const owned = (vaults ?? []).filter((v) => v.kind === "org" && v.role === "owner");
  // Same keys and endpoints as the Settings and Memory spaces pages, so the caches are shared.
  const people = useQuery({ queryKey: adminKeys.people(), queryFn: () => call(listPeople()) });
  const https = useQuery({ queryKey: settingsKeys.httpsStatus(), queryFn: () => call(getHttpsStatus()) as Promise<HttpsStatus> });
  const pending = useQueries({
    queries: owned.map((v) => ({
      queryKey: vaultKeys.pending(v.id),
      queryFn: async () => (await call(listVaultPendingInvitations({ path: { vault_id: v.id } }))).results as PendingInvite[],
    })),
  });
  const pendingCount = pending.reduce((n, q) => n + (q.data?.length ?? 0), 0);

  const signals: { href: string; text: string }[] = [];
  if (pendingCount > 0) signals.push({ href: "/vaults", text: plural(pendingCount, "pending invitation", "pending invitations") });
  if (people.data?.signup.mode === "open") signals.push({ href: "/settings?tab=people", text: "Sign-up is open" });
  if (https.data && (!https.data.configured || https.data.state === "off")) signals.push({ href: "/settings?tab=https", text: "HTTPS is off" });
  else if (https.data?.state === "error") signals.push({ href: "/settings?tab=https", text: "HTTPS has a problem" });
  if (signals.length === 0) return null;

  return (
    <p className="text-[13px] -mt-4" aria-label="Workspace notes" style={{ color: "var(--ink-dim)" }}>
      {signals.map((s, i) => (
        <span key={s.href + s.text}>
          {i > 0 && " · "}
          <Link href={s.href} className="link">
            {s.text}
          </Link>
        </span>
      ))}
    </p>
  );
}

// ---- choose your AI app ----

/** Which AI app do you use? Neutral choices, nothing preselected; each opens that app's own steps. */
function AppPicker() {
  return (
    <section className="task-panel p-6 sm:p-8 flex flex-col gap-5" aria-labelledby="pick-title">
      <div className="flex flex-col gap-2">
        <h2 id="pick-title" className="text-[20px] font-semibold tracking-[-0.3px] leading-tight">
          Which AI app do you use?
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

/** Once an app is connected, the picker shrinks to one line that opens the choices in place. */
function ConnectAnother({ none = false, fromSources = false }: { none?: boolean; fromSources?: boolean }) {
  const [open, setOpen] = useState(false);
  return (
    <section className="flex flex-col gap-4" aria-label={none ? "Connect an AI app" : "Connect another AI app"}>
      {none && (
        <p className="text-[14px]" style={{ color: "var(--ink-dim)" }}>
          No AI app has used Eunomia yet. What you see here was added by you or your teammates{fromSources ? " and your connected sources" : ""}.
        </p>
      )}
      <button
        type="button"
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
        className="link self-start inline-flex items-center gap-1 min-h-11 text-[14px] font-medium"
      >
        {none ? "Connect an AI app" : "Connect another app"} <ArrowRight size={14} strokeWidth={1.75} aria-hidden className={open ? "rotate-90" : undefined} />
      </button>
      {open && <AppChoices />}
    </section>
  );
}

/** Or start with your own information: shown while there is nothing saved yet. */
function OwnInformation({ canUpload }: { canUpload: boolean }) {
  return (
    <section className="flex flex-col gap-3" aria-labelledby="own-info-title">
      <h2 id="own-info-title" className="section-title">
        Or start with your own information
      </h2>
      <p className="text-[14px]" style={{ color: "var(--ink-dim)" }}>
        You can read and organise your memories and documents before connecting an AI app.
      </p>
      <div className="flex flex-wrap gap-2">
        {canUpload && (
          <Link href="/documents" className="btn">
            <Upload size={18} strokeWidth={1.75} aria-hidden /> Upload a document
          </Link>
        )}
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
function ConnectedPanel({ connections }: { connections: Connection[] }) {
  const latest = connections[0];
  const app = appForConnection(latest);
  const shown = connections.slice(0, 6);
  return (
    <section className="ledger flex flex-col overflow-hidden" aria-labelledby="connected-title">
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
        {app.openUrl && connections.length === 1 && (
          <a href={app.openUrl} target="_blank" rel="noreferrer" className="btn btn-primary btn-sm shrink-0 self-start sm:self-auto">
            Open {app.name}
          </a>
        )}
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
        <Link href="/settings?tab=apps" className="link inline-flex items-center gap-1 font-medium">
          Manage connected apps <ArrowRight size={14} strokeWidth={1.75} aria-hidden />
        </Link>
      </div>
    </section>
  );
}

export default function HomePage() {
  const sourcesQuery = useSources({ refetchInterval: 60_000 });
  const rows = sourcesQuery.data ?? (sourcesQuery.isError ? [] : null);
  const vaultsQuery = useVaults();
  const vaults = vaultsQuery.data;
  const overviewQuery = useQuery({
    queryKey: [...entityKeys.live(), "overview", (vaults ?? []).map((v) => v.id).join(",")],
    queryFn: () => loadOverview(vaults ?? []),
    enabled: vaults !== undefined || vaultsQuery.isError,
    refetchInterval: 60_000,
  });
  const overview = overviewQuery.data ?? null;
  const documents = useDocuments().data;
  // Hide the upload shortcut while document storage is off (or unknown).
  const canUpload = documents !== undefined && Boolean(documents.storage);
  const [dismissedFailures, setDismissedFailures] = useState<Set<string>>(new Set());
  const { connections } = useConnections();
  const isAdmin = Boolean(useMe().data?.is_admin);

  const liveSources = rows?.filter(isLiveSource) ?? [];
  // Shared spaces plus the personal space every account has.
  const spaceCount = vaults ? vaults.length : null;
  const hasData = Boolean(overview && (overview.entities > 0 || overview.memories > 0)) || liveSources.length > 0 || vaults?.some((v) => v.kind === "org");
  // Connected only on the server's word: an approved sign-in or a token an app has used.
  const connectedApps = connections !== null && connections.length > 0;

  const appBlock =
    connections === null ? (
      <div className="task-panel p-8 flex flex-col gap-4" aria-busy="true" aria-label="Checking your connected apps">
        <div className="skeleton h-7 w-72" />
        <div className="skeleton h-4 w-full max-w-[520px]" />
      </div>
    ) : !connectedApps && hasData ? (
      // Someone already working in Eunomia gets one honest line, not the first-run card.
      <ConnectAnother none fromSources={liveSources.length > 0} />
    ) : !connectedApps ? (
      <AppPicker />
    ) : (
      <>
        <ConnectedPanel connections={connections} />
        <ConnectAnother />
      </>
    );

  return (
    <div className="flex flex-col gap-8 max-w-[1240px]">
      <PageHeader title="Home" intro="Your shared memory. ChatGPT, Claude and other AI apps can look things up here." />

      {isAdmin && <AdminSignals vaults={vaults} />}

      <InvitationBanner />

      {rows && (
        <FailureBanner rows={rows} dismissed={dismissedFailures} onDismiss={(key) => setDismissedFailures((prev) => new Set(prev).add(key))} />
      )}

      {/* Until an app is connected, connecting one comes first: the full picker on a fresh account,
          one line for someone who already has things saved. */}
      {!connectedApps && appBlock}

      {overviewQuery.isError && <ErrorLine error={failure(overviewQuery.error, "Could not load what has been saved.")} />}

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
                What&apos;s saved
              </h2>
              <StatLine spaces={spaceCount} overview={overview} />
            </div>
            <div className={`grid gap-x-10 gap-y-8 ${liveSources.length > 0 ? "lg:grid-cols-2" : ""}`}>
              <RecentlyUpdated rows={overview.recent} showSpace={(vaults?.length ?? 0) > 1} />
              {liveSources.length > 0 && <SourcesHealth rows={liveSources} />}
            </div>
          </section>
        )
      )}

      {connectedApps && appBlock}

      {overview && !hasData && !connectedApps && <OwnInformation canUpload={canUpload} />}
    </div>
  );
}

"use client";

import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { z } from "zod";
import Link from "next/link";
import { API_URL } from "@/lib/api";
import { useLiveStatus } from "@/lib/queries/live";
import ErrorLine, { failure } from "@/components/ErrorLine";
import { words } from "./DefinitionRows";
import { twinHref } from "./model";

const eventSchema = z.object({
  seq: z.number(), at: z.string(), actor: z.string(), actor_name: z.string(), actor_type: z.string(),
  action: z.string(), definition_type: z.string(), kind: z.string(), name: z.string(),
  after: z.unknown().optional(),
  target: z.string(), revision: z.number(), reason: z.string().nullable(),
});
const progressSchema = z.object({ processed: z.number().int().nonnegative(), total: z.number().int().nonnegative() });
export function importProgress(value: unknown): string | null {
  const parsed = progressSchema.safeParse(value);
  return parsed.success ? `${parsed.data.processed.toLocaleString("en-AU")} of ${parsed.data.total.toLocaleString("en-AU")} rows processed` : null;
}
const pageSchema = z.object({ events: z.array(eventSchema), next: z.number(), more: z.boolean() });

export default function Activity({ space, compact = false, definition }: {
  space: string; compact?: boolean; definition?: { type: string; kind: string; name: string };
}) {
  const live = useLiveStatus();
  const [actor, setActor] = useState("");
  const [action, setAction] = useState("");
  const [type, setType] = useState("");
  const [kind, setKind] = useState("");
  const [cursor, setCursor] = useState<number[]>([]);
  const [seen, setSeen] = useState<number | null>(null);
  const [paused, setPaused] = useState(false);
  const before = cursor.at(-1);
  const query = useQuery({
    queryKey: ["ontology-activity", space, { actor, action, type, kind, before, definition }],
    queryFn: async () => {
      const params = new URLSearchParams({ newest: "true", limit: compact ? "5" : "30" });
      if (type) params.set("definition_type", type);
      if (actor) params.set("actor_type", actor);
      if (action) params.set("action", action);
      if (definition?.kind || kind) params.set("kind", definition?.kind || kind);
      if (definition) { params.set("review_only", "true"); params.set("definition_type", definition.type); params.set("name", definition.name); }
      if (before !== undefined) params.set("before", String(before));
      const response = await fetch(`${API_URL}/api/vaults/${encodeURIComponent(space)}/ontology/activity?${params}`, { credentials: "include" });
      if (!response.ok) throw new Error("Could not load activity. Try again.");
      return pageSchema.parse(await response.json());
    },
    refetchInterval: live === "live" || paused ? false : 12000,
  });
  const events = query.data?.events ?? [];
  // Keep an existing reader's rows in place. The notice occupies a reserved line.
  const [anchor, setAnchor] = useState<{ key: string; events: z.infer<typeof eventSchema>[] } | null>(null);
  const key = JSON.stringify({ actor, action, type, kind, before, definition });
  const shown = anchor?.key === key ? anchor.events : events;
  const unseen = seen === null ? 0 : events.filter(e => e.seq > seen).length;
  function browse() { setCursor([]); setAnchor(null); setSeen(null); setPaused(false); }
  function pause() { setAnchor({ key, events: shown }); setSeen(events[0]?.seq ?? 0); setPaused(true); }
  return <section aria-label={definition ? "Review history" : "Ontology activity"} className="ledger p-4 flex flex-col gap-3 min-w-0">
    <div className="flex flex-wrap items-center justify-between gap-2">
      <h2 className="section-title">{definition ? "Review history" : compact ? "See it happen" : "Activity"}</h2>
      <span className="label">{paused ? "View paused" : live === "live" ? "Live" : "Reconnecting"}</span>
      {!definition && <button className="btn btn-ghost btn-sm" onClick={() => { if (paused) { setPaused(false); setAnchor(null); setSeen(null); } else pause(); }}>{paused ? "Follow activity" : "Pause view"}</button>}
    </div>
    {!compact && <div className="flex flex-wrap gap-3 text-[13px]">
      <label>Actor<select aria-label="Activity actor" className="field p-2 ml-2" value={actor} onChange={e => { setActor(e.target.value); browse(); }}>
        <option value="">Everyone</option><option value="user">Person</option><option value="token">Agent token</option><option value="oauth">App</option><option value="system">System</option>
      </select></label>
      <label>Change<select aria-label="Activity change" className="field p-2 ml-2" value={action} onChange={e => { setAction(e.target.value); browse(); }}>
        <option value="">All changes</option>{["proposed", "approved", "rejected", "merged", "described", "imported", "import_progress", "value_written", "strict_changed"].map(a => <option key={a} value={a}>{words(a)}</option>)}
      </select></label>
      <label>Type<select aria-label="Activity definition type" className="field p-2 ml-2" value={type} onChange={e => { setType(e.target.value); browse(); }}>
        <option value="">All types</option>{["kind", "link", "property", "metric", "import", "policy"].map(t => <option key={t} value={t}>{words(t)}</option>)}
      </select></label>
      <label>Kind<input className="field p-2 ml-2" value={kind} placeholder="All kinds" onChange={e => { setKind(e.target.value); browse(); }} /></label>
    </div>}
    {query.isError && <ErrorLine error={failure(query.error, "Could not load activity.")} />}
    <div className="max-h-96 overflow-auto" onScroll={e => { if (!paused && e.currentTarget.scrollTop > 16) pause(); }}>
      <ol className="hairline-rows text-[13px]">
        {shown.map(event => <li key={event.seq} className="py-3 [overflow-wrap:anywhere]">
          <p><span className="font-medium">{event.actor_name}</span> {words(event.action).toLowerCase()} {words(event.name)} <span className="label">{words(event.definition_type)}{event.kind && event.definition_type !== "kind" && event.definition_type !== "link" ? ` · ${words(event.kind)}` : ""}</span></p>
          <time dateTime={event.at} className="text-[12px]" style={{ color: "var(--ink-dim)" }}>{new Date(event.at).toLocaleString("en-AU")}</time>
          {event.action === "import_progress" && <p className="mt-1">{importProgress(event.after)}</p>}
          {event.reason && <p className="mt-1">{event.reason}</p>}
        </li>)}
        {shown.length === 0 && <li className="py-3">{query.isPending ? "Loading activity…" : "No activity recorded for these filters."}</li>}
      </ol>
    </div>
    {unseen > 0 && <button className="btn btn-sm self-start" onClick={() => { setAnchor(null); setSeen(null); setPaused(false); }}>{unseen} new events</button>}
    {!compact && <div className="flex gap-2">
      <button className="btn btn-sm" disabled={cursor.length === 0} onClick={() => setCursor(cursor.slice(0, -1))}>Newer</button>
      <button className="btn btn-sm" disabled={!query.data?.more} onClick={() => { if (query.data) setCursor([...cursor, query.data.next]); }}>Older</button>
    </div>}
    {compact && !definition && <Link className="link self-start text-[13px]" href={twinHref("/ontology/activity", space)}>Open all activity</Link>}
  </section>;
}

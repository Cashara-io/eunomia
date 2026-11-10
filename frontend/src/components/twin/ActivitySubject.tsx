import { useEffect, useRef } from "react";
import Link from "next/link";
import { useOntology } from "@/lib/queries/ontology";
import { definitions, twinHref } from "./model";
import { humanLabel } from "./overview-format";
import ObjectProfile, { objectHref } from "./ObjectProfile";
import ReadState from "./ReadState";
import styles from "./receipts.module.css";
export type ActivityEvent = { definition_type: string; kind: string; name: string; target: string; revision: number; action: string };
export default function ActivitySubject({ event, space, close, restore }: { event: ActivityEvent; space: string; close: () => void; restore: () => void }) {
  const heading = useRef<HTMLHeadingElement>(null);
  const returnFocus = useRef(restore);
  useEffect(() => { returnFocus.current = restore; }, [restore]);
  const object = ["entity", "object"].includes(event.definition_type) && event.target.startsWith("entity:");
  const query = useOntology(space, !object);
  useEffect(() => { heading.current?.focus(); return () => returnFocus.current(); }, []);
  const candidates = query.data ? definitions(query.data).filter(d => d.type === event.definition_type && d.kind === event.kind && (d.name === event.name || (d.type === "link" && d.item.label === event.name))) : [];
  const definition = candidates.length === 1 ? candidates[0] : undefined;
  return <aside className={styles.activitySubject} aria-label="Activity subject" onKeyDown={e => { if (e.key === "Escape") close(); }}>
    <div className={styles.inspectorHead}><p className={styles.plate}>Activity receipt</p><button className="btn btn-sm" onClick={close}>Back to activity</button></div>
    <h3 ref={heading} tabIndex={-1}>{humanLabel(event.name)}</h3>
    {object ? <><p>Historical object version unavailable. This is the current object.</p><ObjectProfile id={event.target} space={space} compact /><Link className="link" href={objectHref(event.target, space)}>Open current object</Link></> : query.isError ? <ReadState error={query.error} retry={() => void query.refetch()} subject="this activity subject" /> : !query.data ? <p role="status">Loading activity subject…</p> : definition ? <>
      <p>{definition.item.revision === event.revision ? "Current definition matches this event's revision." : "Historical version unavailable. The current definition has changed since this event."}</p>
      {event.action === "value_written" && <p>The event names the property definition. The object and historical value were not retained in this activity record.</p>}
      <p className={styles.plate}>Current {humanLabel(definition.type).toLowerCase()} · {humanLabel(definition.kind)}</p>
      <p>{definition.item.description || "No description recorded."}</p>
      <p>Status: {humanLabel(definition.item.status)}</p>
      {"datatype" in definition.item && <p>Value type: {humanLabel(definition.item.datatype)}{definition.item.unit ? ` · ${definition.item.unit}` : ""}</p>}
      {"source_map" in definition.item && definition.item.source_map?.map((source, index) => <p key={index}>{source.source} · {source.path}</p>)}
      <Link className="link" href={twinHref("/ontology/review", space, { selected: definition.id, status: "all" })}>Open current definition</Link>
    </> : <p>Historical version unavailable. This event does not resolve to one accessible current definition in this space.</p>}
  </aside>;
}

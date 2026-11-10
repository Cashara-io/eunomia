"use client";

import { Fragment, useEffect, useLayoutEffect, useRef, useState } from "react";
import { UserIcon, RobotIcon, GearIcon } from "@phosphor-icons/react";
import { activityDay, humanLabel, relativeTime } from "./overview-format";
import styles from "./overview.module.css";
import receipts from "./receipts.module.css";
import { useOverviewHighlight } from "./OverviewHighlight";
import { useQuery } from "@tanstack/react-query";
import { z } from "zod";
import Link from "next/link";
import { API_URL, ApiError } from "@/lib/api";
import { useLiveStatus } from "@/lib/queries/live";
import ReceiptAccess from "./ReceiptAccess";
import ActivitySubject from "./ActivitySubject";
import { useTwinParams } from "./controls";
import ReadState from "./ReadState";
import LoadingSkeleton from "@/components/ui/LoadingSkeleton";
import { words } from "./DefinitionRows";
import { twinHref } from "./model";

const eventSchema = z.object({
  seq: z.number(),
  at: z.string(),
  actor: z.string(),
  actor_name: z.string(),
  actor_type: z.string(),
  action: z.string(),
  definition_type: z.string(),
  kind: z.string(),
  name: z.string(),
  after: z.unknown().optional(),
  target: z.string(),
  revision: z.number(),
  reason: z.string().nullable(),
});
const progressSchema = z.object({
  processed: z.number().int().nonnegative(),
  total: z.number().int().nonnegative(),
});
export function importProgress(value: unknown): string | null {
  const parsed = progressSchema.safeParse(value);
  return parsed.success
    ? `${parsed.data.processed.toLocaleString("en-AU")} of ${parsed.data.total.toLocaleString("en-AU")} rows processed`
    : null;
}
const pageSchema = z.object({
  events: z.array(eventSchema),
  next: z.number(),
  more: z.boolean(),
});

export default function Activity(props: { space: string; compact?: boolean; definition?: { type: string; kind: string; name: string } }) {
  return <ReceiptAccess space={props.space}><ActivityContent {...props} /></ReceiptAccess>;
}
function ActivityContent({
  space,
  compact = false,
  definition,
}: {
  space: string;
  compact?: boolean;
  definition?: { type: string; kind: string; name: string };
}) {
  const { params: routeParams, update } = useTwinParams();
  const selectedSubject = routeParams.get("subject");
  const trigger = useRef<HTMLAnchorElement | null>(null);
  const rowRegion = useRef<HTMLDivElement>(null);
  const scrollPosition = useRef(Number(routeParams.get("activityScroll")) || 0);
  useLayoutEffect(() => {
    if (!selectedSubject && rowRegion.current) rowRegion.current.scrollTop = scrollPosition.current;
  }, [selectedSubject]);
  const live = useLiveStatus();
  const highlightedKinds = useOverviewHighlight();
  const [now, setNow] = useState<number | null>(null);
  useEffect(() => {
    const tick = () => setNow(Date.now());
    tick();
    const timer = setInterval(tick, 30_000);
    return () => clearInterval(timer);
  }, []);
  const [actor, setActor] = useState("");
  const [action, setAction] = useState("");
  const [type, setType] = useState("");
  const [kind, setKind] = useState("");
  const [cursor, setCursor] = useState<number[]>([]);
  const [seen, setSeen] = useState<number | null>(() => routeParams.has("activityPaused") ? Number(routeParams.get("activityPaused")) : null);
  const [paused, setPaused] = useState(() => routeParams.has("activityPaused"));
  const before = cursor.at(-1);
  const query = useQuery({
    retryOnMount: false,
    queryKey: [
      "ontology-activity",
      space,
      { actor, action, type, kind, before, definition },
    ],
    queryFn: async ({ signal }) => {
      const params = new URLSearchParams({
        newest: "true",
        limit: compact ? "5" : "30",
      });
      if (type) params.set("definition_type", type);
      if (actor) params.set("actor_type", actor);
      if (action) params.set("action", action);
      if (definition?.kind || kind)
        params.set("kind", definition?.kind || kind);
      if (definition) {
        params.set("review_only", "true");
        params.set("definition_type", definition.type);
        params.set("name", definition.name);
      }
      if (before !== undefined) params.set("before", String(before));
      const response = await fetch(
        `${API_URL}/api/vaults/${encodeURIComponent(space)}/ontology/activity?${params}`,
        { credentials: "include", signal },
      );
      if (!response.ok) {
        const problem = z.object({ code: z.string().optional() }).safeParse(await response.json());
        throw new ApiError(response.status, problem.success ? problem.data.code ?? "http.error" : "http.error", "Could not load activity.");
      }
      return pageSchema.parse(await response.json());
    },
    refetchInterval: live === "live" || paused ? false : 12000,
  });
  const events = query.data?.events ?? [];
  // Keep an existing reader's rows in place. The notice occupies a reserved line.
  const [anchor, setAnchor] = useState<{
    key: string;
    events: z.infer<typeof eventSchema>[];
  } | null>(null);
  const key = JSON.stringify({ actor, action, type, kind, before, definition });
  const shown = anchor?.key === key ? anchor.events : paused && seen !== null ? events.filter(event => event.seq <= seen) : events;
  const unseen = seen === null ? 0 : events.filter((e) => e.seq > seen).length;
  function browse() {
    update({ activityPaused: "", activityScroll: "", subject: "" }, true);
    setCursor([]);
    setAnchor(null);
    setSeen(null);
    setPaused(false);
  }
  function pause() {
    setAnchor({ key, events: shown });
    setSeen(events[0]?.seq ?? 0);
    setPaused(true);
    update({ activityPaused: String(events[0]?.seq ?? 0), activityScroll: String(scrollPosition.current) }, true);
  }
  return (
    <section
      aria-label={definition ? "Review history" : "Ontology activity"}
      className={styles.activity}
    >
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h2 className="section-title">
          {definition
            ? "Review history"
            : compact
              ? "Latest activity"
              : "Activity"}
        </h2>
        <span className="label">
          {paused ? "View paused" : live === "live" ? "Live" : "Reconnecting"}
        </span>
        {!definition && (
          <button
            className="btn btn-ghost btn-sm"
            onClick={() => {
              if (paused) {
                setPaused(false);
                setAnchor(null);
                setSeen(null);
                update({ activityPaused: "", activityScroll: "" }, true);
              } else pause();
            }}
          >
            {paused ? "Follow activity" : "Pause view"}
          </button>
        )}
      </div>
      {!compact && (
        <div className="flex flex-wrap gap-3 text-[13px]">
          <label>
            Actor
            <select
              aria-label="Activity actor"
              className="field p-2 ml-2"
              value={actor}
              onChange={(e) => {
                setActor(e.target.value);
                browse();
              }}
            >
              <option value="">Everyone</option>
              <option value="user">Person</option>
              <option value="token">Agent token</option>
              <option value="oauth">App</option>
              <option value="system">System</option>
            </select>
          </label>
          <label>
            Change
            <select
              aria-label="Activity change"
              className="field p-2 ml-2"
              value={action}
              onChange={(e) => {
                setAction(e.target.value);
                browse();
              }}
            >
              <option value="">All changes</option>
              {[
                "proposed",
                "approved",
                "rejected",
                "merged",
                "described",
                "imported",
                "import_progress",
                "value_written",
                "strict_changed",
              ].map((a) => (
                <option key={a} value={a}>
                  {words(a)}
                </option>
              ))}
            </select>
          </label>
          <label>
            Type
            <select
              aria-label="Activity definition type"
              className="field p-2 ml-2"
              value={type}
              onChange={(e) => {
                setType(e.target.value);
                browse();
              }}
            >
              <option value="">All types</option>
              {["kind", "link", "property", "metric", "import", "policy"].map(
                (t) => (
                  <option key={t} value={t}>
                    {words(t)}
                  </option>
                ),
              )}
            </select>
          </label>
          <label>
            Kind
            <input
              className="field p-2 ml-2"
              value={kind}
              placeholder="All kinds"
              onChange={(e) => {
                setKind(e.target.value);
                browse();
              }}
            />
          </label>
        </div>
      )}
      {query.isError ? (
        <ReadState
          error={query.error}
          retry={() => void query.refetch()}
          subject="activity"
        />
      ) : query.isPending ? (
        <LoadingSkeleton label="Loading activity" layout="rows" rows={3} />
      ) : (
        <>
          <div
            className={`${styles.activityRows} ${receipts.activityViewport}`}
            ref={rowRegion}
            onScroll={(e) => {
              scrollPosition.current = e.currentTarget.scrollTop;
              if (paused) update({ activityScroll: String(scrollPosition.current) }, true);
              if (!paused && e.currentTarget.scrollTop > 16) pause();
            }}
          >
            <ol className="text-[13px]">
              {shown
                .filter(
                  (event, index, rows) =>
                    rows.findIndex((other) => other.seq === event.seq) ===
                    index,
                )
                .map((event, index, rows) => {
                  const Actor =
                    event.actor_type === "user"
                      ? UserIcon
                      : event.actor_type === "system"
                        ? GearIcon
                        : RobotIcon;
                  const day = now === null ? "" : activityDay(event.at, now);
                  const previous = rows[index - 1];
                  const subject =
                    event.definition_type === "import"
                      ? twinHref("/ontology/objects", space, {
                          kind: event.kind,
                        })
                      : event.definition_type === "kind"
                        ? twinHref(
                            `/ontology/kinds/${encodeURIComponent(event.kind || event.name)}`,
                            space,
                          )
                        : event.definition_type === "property" ||
                            event.definition_type === "metric"
                          ? twinHref("/ontology/review", space, {
                              selected: JSON.stringify([
                                event.definition_type,
                                event.kind,
                                event.name,
                              ]),
                            })
                          : twinHref("/ontology/schema", space);
                  return (
                    <Fragment key={event.seq}>
                      {day &&
                        (!previous ||
                          activityDay(previous.at, now ?? 0) !== day) && (
                          <li className={styles.dayHeading}>{day}</li>
                        )}
                      <li
                        className={styles.activityRow}
                        data-dim={
                          highlightedKinds.length > 0 &&
                          !highlightedKinds.includes(event.kind)
                        }
                      >
                        <span
                          className={styles.actorIcon}
                          title={
                            event.actor_type === "user"
                              ? "Person"
                              : event.actor_type === "system"
                                ? "System"
                                : "AI app"
                          }
                        >
                          <Actor aria-hidden size={18} weight="duotone" />
                          <span className="sr-only">
                            {event.actor_type === "user" ? "Person" : "App"}
                          </span>
                        </span>
                        <div>
                          <p>
                            <span>{event.actor_name}</span>{" "}
                            <strong className="font-medium">
                              {humanLabel(event.action).toLowerCase()}
                            </strong>{" "}
                            <Link href={subject} onClick={e => {
                              if (e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
                              e.preventDefault();
                              trigger.current = e.currentTarget;
                              if (!paused) pause();
                              update({ subject: String(event.seq) });
                            }}>{humanLabel(event.name)}</Link>{" "}
                            <span className="label">
                              {humanLabel(event.definition_type)}
                              {event.kind &&
                              event.definition_type !== "kind" &&
                              event.definition_type !== "link"
                                ? ` · ${humanLabel(event.kind)}`
                                : ""}
                            </span>
                          </p>
                          {event.action === "import_progress" && (
                            <p className="mt-1">
                              {importProgress(event.after)}
                            </p>
                          )}
                          {!compact && event.reason && (
                            <p className="mt-1">{event.reason}</p>
                          )}
                        </div>
                        <time
                          dateTime={event.at}
                          title={new Date(event.at).toLocaleString("en-AU")}
                        >
                          {now === null ? "" : relativeTime(event.at, now)}
                        </time>
                      </li>
                    </Fragment>
                  );
                })}
              {shown.length === 0 && (
                <li className="py-3">
                  {actor || action || type || kind
                    ? "No activity recorded for these filters."
                    : "No activity recorded yet."}
                </li>
              )}
            </ol>
          </div>
          {shown.find(event => String(event.seq) === selectedSubject) && (() => {
            const event = shown.find(event => String(event.seq) === selectedSubject);
            return event ? <ActivitySubject event={event} space={space} close={() => {
              if (trigger.current) window.history.back();
              else update({ subject: "" }, true);
            }} restore={() => trigger.current?.focus({ preventScroll: true })} /> : null;
          })()}
          {unseen > 0 && (
            <button
              className="btn btn-sm self-start"
              onClick={() => {
                setAnchor(null);
                setSeen(null);
                setPaused(false);
                update({ activityPaused: "", activityScroll: "" }, true);
              }}
            >
              {unseen} new events
            </button>
          )}
          {!compact && (
            <div className="flex gap-2">
              <button
                className="btn btn-sm"
                disabled={cursor.length === 0}
                onClick={() => setCursor(cursor.slice(0, -1))}
              >
                Newer
              </button>
              <button
                className="btn btn-sm"
                disabled={!query.data?.more}
                onClick={() => {
                  if (query.data) setCursor([...cursor, query.data.next]);
                }}
              >
                Older
              </button>
            </div>
          )}
        </>
      )}
      {compact && !definition && (
        <Link
          className="link self-start text-[13px]"
          href={twinHref("/ontology/activity", space)}
        >
          Open all activity
        </Link>
      )}
    </section>
  );
}

"use client";
import Link from "next/link";
import { useEffect, useRef, useState } from "react";
import ReceiptAccess from "./ReceiptAccess";
import ValueReceipts from "./ValueReceipts";
import ObjectHero from "./ObjectHero";
import SourceInspector from "./SourceInspector";
import { safeReturn } from "./receipt-model";
import styles from "./receipts.module.css";
import Select from "@/components/Select";
import { ApiError } from "@/lib/api";
import ReadState from "./ReadState";
import LoadingSkeleton from "@/components/ui/LoadingSkeleton";
import {
  useObjectProfile,
  type ObjectColumn,
} from "@/lib/queries/objects";
import { formatPropertyNumber } from "@/lib/numbers";
import { twinHref } from "./model";
import { useTwinParams } from "./controls";
import { words as plainWords } from "./DefinitionRows";
const words = (value: string) => {
  const text = plainWords(value);
  return text.charAt(0).toUpperCase() + text.slice(1);
};

export const objectHref = (id: string, space: string) =>
  twinHref(`/ontology/objects/${encodeURIComponent(id)}`, space);
export function formatted(
  value: string | number | boolean | null | undefined,
  column: Pick<ObjectColumn, "datatype" | "unit">,
): string {
  if (value === null || value === undefined) return "Missing";
  if (typeof value === "boolean") return value ? "Yes" : "No";
  if (typeof value === "number")
    return formatPropertyNumber(value, column.datatype, column.unit);
  if (column.datatype === "date")
    return Number.isNaN(Date.parse(value))
      ? "Unavailable"
      : new Date(value).toLocaleDateString("en-AU", {
          dateStyle: "medium",
          timeZone: "UTC",
        });
  if (column.datatype === "datetime") return date(value);
  return value;
}
function date(value: string | null | undefined) {
  if (!value || value === "NONE" || Number.isNaN(Date.parse(value)))
    return "Date not recorded";
  return new Date(value).toLocaleString("en-AU", {
    dateStyle: "medium",
    timeStyle: "short",
  });
}
const sections = [
  "Summary",
  "Properties",
  "Numbers",
  "Links",
  "Timeline",
  "Sources",
  "Related memories",
];
export default function ObjectProfile(props: { space: string; id: string; compact?: boolean }) {
  return <ReceiptAccess space={props.space}><ProfileContent {...props} /></ReceiptAccess>;
}
function ProfileContent({
  space,
  id,
  compact = false,
}: {
  space: string;
  id: string;
  compact?: boolean;
}) {
  const { params, update } = useTwinParams();
  const section = params.get("section") ?? "Properties";
  const selectedValue = params.get("receipt");
  const valueTrigger = useRef<HTMLButtonElement | null>(null);
  function inspect(name: string, button: HTMLButtonElement) {
    valueTrigger.current = button;
    update({ receipt: name, section });
  }
  useEffect(() => {
    const current = new URLSearchParams(window.location.search);
    if (current.has("section")) return;
    try {
      const saved = sessionStorage.getItem(`receipt-section:${space}:${id}`);
      if (saved && sections.includes(saved)) {
        current.set("section", saved);
        window.history.replaceState(null, "", `${window.location.pathname}?${current}`);
      }
    } catch { /* Sections still work without local presentation storage. */ }
  }, [space, id]);
  const setSection = (section: string) => {
    try { sessionStorage.setItem(`receipt-section:${space}:${id}`, section); } catch { /* Optional presentation state. */ }
    update({ section, receipt: "" });
  };
  const [offset, setOffset] = useState(0);
  const query = useObjectProfile(space, id, offset);
  if (query.error instanceof ApiError && query.error.code === "entity.not_found")
    return <section className="ledger p-6" aria-label="Object unavailable"><h2 className="section-title">Object unavailable</h2><p className="mt-3">This object could not be found in this space. It may have been removed or belong to another space.</p><Link className="link inline-block mt-4" href={safeReturn(params.get("back"), space) ?? twinHref("/ontology/objects", space)}>Back to Objects</Link></section>;
  if (query.isError)
    return (
      <ReadState error={query.error} subject="this object" retry={() => void query.refetch()} />
    );
  if (!query.data || query.isFetching) return <LoadingSkeleton label="Loading object receipt" layout="rows" rows={4} />;
  const data = query.data;
  return (
    <div className={styles.profile}>
      {!compact && (
        <Link
          className="link self-start"
          href={safeReturn(params.get("back"), space) ?? twinHref("/ontology/objects", space, { kind: data.kind })}
        >
          Back to Objects
        </Link>
      )}
      <ObjectHero data={data} compact={compact} />
      <p className={styles.disclaimer}>Approval covers the kind definition, not every value.</p>
      <div className={styles.stack} data-open={!!selectedValue}>
      <div className={styles.body}>
      {compact ? (
        <>
          <Link className="btn btn-sm self-start" href={`${objectHref(id, space)}&back=${encodeURIComponent(twinHref("/ontology/objects", space, Object.fromEntries(params.entries())))}`}>
            Open object page
          </Link>
          <ValueReceipts data={data} inspect={inspect} selected={selectedValue} />
          <Link
            className="link"
            href={`${objectHref(id, space)}&section=Links`}
          >
            Links, numbers and sources
          </Link>
        </>
      ) : (
        <>
          <nav
            aria-label="Object sections"
            className="hidden sm:flex flex-wrap gap-2"
          >
            {sections.map((s) => (
              <button
                key={s}
                className="pill"
                aria-current={s === section ? "page" : undefined}
                onClick={() => {
                  setSection(s);
                  setOffset(0);
                }}
              >
                {s}
              </button>
            ))}
          </nav>
          <div className="sm:hidden">
            <span className="label">Section</span>
            <Select
              aria-label="Object section"
              value={section}
              options={sections.map((s) => ({ value: s, label: s }))}
              onChange={(s) => {
                setSection(s);
                setOffset(0);
              }}
              className="w-full min-h-11"
            />
          </div>
          <section className="ledger p-4 sm:p-5 min-w-0" aria-label={section}>
            <h3 className="section-title mb-4">{section}</h3>
            {section === "Summary" && (
              <>
                <p>{data.summary || "No summary recorded."}</p>
                <dl className="grid grid-cols-2 gap-4 mt-5">
                  <div>
                    <dt className="label">Known properties</dt>
                    <dd className="font-mono text-xl">
                      {
                        data.properties.filter((p) => p.state !== "missing")
                          .length
                      }{" "}
                      / {data.properties.length}
                    </dd>
                  </div>
                  <div>
                    <dt className="label">Recorded links</dt>
                    <dd className="font-mono text-xl">
                      {data.link_counts.reduce((n, g) => n + g.total, 0)}
                    </dd>
                  </div>
                </dl>
                <p className="mt-5 text-[var(--ink-dim)]">
                  {data.warnings.join(" ")}
                </p>
              </>
            )}
            {section === "Properties" && (
              <>
                <p className="text-[var(--ink-dim)] mb-4">
                  Stale means the source changed after the value was recorded.
                  Conflicting means different values share the latest as-of
                  time.
                </p>
                <ValueReceipts data={data} inspect={inspect} selected={selectedValue} />
              </>
            )}
            {section === "Numbers" && (
              <div className="divide-y divide-[var(--border)]">
                {data.numbers.length === 0 && (
                  <p>No approved metrics for this kind.</p>
                )}
                {data.numbers.map((n) => (
                  <div
                    key={n.name}
                    className="py-4 first:pt-0 flex flex-col gap-3"
                  >
                    <h4 className="font-medium">{words(n.name)}</h4>
                    <p>{n.definition}</p>
                    {n.result.run ? (
                      <>
                        <p className="font-mono text-2xl">
                          {formatted(n.result.run.value, {
                            datatype:
                              n.result.run.unit &&
                              /^[A-Z]{3}$/.test(n.result.run.unit)
                                ? "money"
                                : "number",
                            unit: n.result.run.unit,
                          })}
                        </p>
                        <p>
                          Approved metric, for this object only.{" "}
                          <span className="font-mono">
                            {n.result.run.n_entities}
                          </span>{" "}
                          {n.result.run.n_entities === 1
                            ? "contributing object."
                            : "contributing objects."}
                        </p>
                        <p className="font-mono text-xs">
                          As of {date(n.result.run.as_of)}
                        </p>
                        {n.result.run.warnings.map((w) => (
                          <p key={w} className="text-[var(--warning)]">
                            {w}
                          </p>
                        ))}
                        <details>
                          <summary className="link cursor-pointer">
                            Inspect inputs
                          </summary>
                          <p className="my-3">
                            Current values are shown below. Historical input versions
                            are not available, so these are not a receipt for the
                            calculation at its cutoff. Missing inputs are reported in warnings.
                          </p>
                          <ValueReceipts
                            data={{
                              ...data,
                              properties: data.properties.filter((p) =>
                                n.result.run?.input_properties.includes(p.name),
                              ),
                            }}
                            inspect={inspect} selected={selectedValue}
                          />
                        </details>
                      </>
                    ) : (
                      <p className="text-[var(--warning)]">
                        Unavailable: {n.result.unavailable}
                      </p>
                    )}
                  </div>
                ))}
              </div>
            )}
            {section === "Links" && (
              <div id="links" className="flex flex-col gap-5">
                {data.link_counts.length === 0 && <p>No recorded links.</p>}
                {data.link_counts.map((g) => (
                  <div key={`${g.outgoing}-${g.label}`}>
                    <h4 className="font-medium">
                      {g.outgoing ? "Outgoing" : "Incoming"}: {words(g.label)}{" "}
                      <span className="font-mono">({g.total})</span>
                    </h4>
                    <ul className="mt-2 space-y-3">
                      {data.links
                        .filter(
                          (l) =>
                            l.label === g.label &&
                            (l.from_id === id) === g.outgoing,
                        )
                        .map((l) => (
                          <li key={l.id}>
                            <Link
                              className="link"
                              href={objectHref(
                                g.outgoing ? l.to_id : l.from_id,
                                space,
                              )}
                            >
                              {g.outgoing ? l.to_name : l.from_name}
                            </Link>
                            <p className="text-[var(--ink-dim)]">{l.from_name} {plainWords(l.label)} {l.to_name}.</p>
                          </li>
                        ))}
                    </ul>
                  </div>
                ))}
              </div>
            )}
            {section === "Timeline" && (
              <>
                <p className="text-[var(--ink-dim)] mb-4">
                  Recorded property writes and relation creation only. Imports
                  can replace earlier values. Review events are not recorded
                  yet.
                </p>
                <ol className="space-y-4">
                  {[
                    ...data.history.map((h) => ({
                      id: h.id,
                      at: h.recorded_at,
                      text: `Property recorded: ${words(h.property ?? "Unknown property")}`,
                    })),
                    ...data.links.map((l) => ({
                      id: l.id,
                      at: l.recorded_at,
                      text: `Link recorded: ${words(l.label)}`,
                    })),
                  ]
                    .sort(
                      (a, b) =>
                        (Date.parse(b.at) || 0) - (Date.parse(a.at) || 0),
                    )
                    .map((e) => (
                      <li key={e.id}>
                        <p>{e.text}</p>
                        <p className="font-mono text-xs text-[var(--ink-dim)]">
                          {date(e.at)}
                        </p>
                      </li>
                    ))}
                </ol>
                {data.history.length === 0 && data.links.length === 0 && (
                  <p>No recorded events.</p>
                )}
              </>
            )}
            {section === "Sources" && (
              <ValueReceipts data={data} inspect={inspect} selected={selectedValue} />
            )}
            {section === "Related memories" && (
              <div className="space-y-5">
                {data.memories.length === 0 && (
                  <p>No memories about this object.</p>
                )}
                {data.memories.map((m) => (
                  <article key={m.id}>
                    <p className="whitespace-pre-wrap break-words">{m.text}</p>
                    <p className="font-mono text-xs text-[var(--ink-dim)] mt-2">
                      {date(m.recorded_at)}
                    </p>
                    <Link
                      className="link"
                      href={`/entities?id=${encodeURIComponent(id)}&vault=${encodeURIComponent(space)}`}
                    >
                      Open in People &amp; topics
                    </Link>
                  </article>
                ))}
              </div>
            )}
            {["Links", "Timeline", "Related memories"].includes(section) && (
              <div className="flex justify-between mt-5">
                <button
                  className="btn btn-sm"
                  disabled={offset === 0}
                  onClick={() => setOffset(Math.max(0, offset - 100))}
                >
                  Previous records
                </button>
                <button
                  className="btn btn-sm"
                  disabled={
                    !(section === "Links"
                      ? data.more.links
                      : section === "Timeline"
                        ? data.more.history || data.more.links
                        : data.more.memories)
                  }
                  onClick={() => setOffset(offset + 100)}
                >
                  Next records
                </button>
              </div>
            )}
          </section>
        </>
      )}
      </div>
      {selectedValue && <SourceInspector data={data} name={selectedValue} space={space}
        close={() => {
          if (valueTrigger.current) window.history.back();
          else update({ receipt: "" }, true);
        }} restoreFocus={() => valueTrigger.current?.focus()} />}
      </div>
    </div>
  );
}

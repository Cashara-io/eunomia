"use client";
import Link from "next/link";
import { useState } from "react";
import Select from "@/components/Select";
import ErrorLine, { failure } from "@/components/ErrorLine";
import {
  useObjectProfile,
  type ObjectColumn,
  type ObjectProfile as Profile,
} from "@/lib/queries/objects";
import { twinHref } from "./model";
import { useTwinParams } from "./controls";
import { words } from "./DefinitionRows";

export const objectHref = (id: string, space: string) =>
  twinHref(`/ontology/objects/${encodeURIComponent(id)}`, space);
export function formatted(
  value: string | number | boolean | null | undefined,
  column: Pick<ObjectColumn, "datatype" | "unit">,
): string {
  if (value === null || value === undefined) return "Missing";
  if (typeof value === "boolean") return value ? "Yes" : "No";
  if (typeof value === "number") {
    if (column.datatype === "money")
      return `${column.unit || "Currency not recorded"} ${value.toLocaleString("en-AU", { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
    if (column.datatype === "percent")
      return `${(value * 100).toLocaleString("en-AU", { maximumFractionDigits: 2 })}%`;
    return `${value.toLocaleString("en-AU", { maximumFractionDigits: 6 })}${column.unit ? ` ${column.unit}` : ""}`;
  }
  if (["date", "datetime"].includes(column.datatype)) return date(value);
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
export default function ObjectProfile({
  space,
  id,
  compact = false,
}: {
  space: string;
  id: string;
  compact?: boolean;
}) {
  const { params, update } = useTwinParams();
  const section = params.get("section") ?? "Summary";
  const setSection = (section: string) => update({ section });
  const [offset, setOffset] = useState(0);
  const query = useObjectProfile(space, id, offset);
  if (query.isError)
    return (
      <ErrorLine
        error={failure(query.error, "Could not load this object. Try again.")}
      />
    );
  if (!query.data) return <p role="status">Loading object…</p>;
  const data = query.data;
  return (
    <div className="flex flex-col gap-5 min-w-0 text-[13px]">
      {!compact && (
        <Link
          className="link self-start"
          href={twinHref("/ontology/objects", space, { kind: data.kind })}
        >
          Back to Objects
        </Link>
      )}
      <div>
        <h2 className="text-[22px] font-medium break-words">{data.name}</h2>
        <p className="mt-2 text-[var(--ink-dim)]">
          {words(data.kind)} ·{" "}
          {data.kind_approved ? "Approved kind" : "Kind not approved"}
        </p>
      </div>
      <p className="text-[var(--ink-dim)]">
        Approval covers the kind definition, not every value.
      </p>
      {compact ? (
        <>
          <Link className="btn btn-sm self-start" href={objectHref(id, space)}>
            Open object page
          </Link>
          <Properties data={data} space={space} />
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
              className="w-full"
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
              <Properties data={data} space={space} />
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
                          contributing objects.
                        </p>
                        <p>{n.result.run.expr_words}</p>
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
                            The evaluator uses the latest values below, applies
                            the metric filters, then evaluates its definition.
                            Missing inputs are omitted and reported in warnings.
                          </p>
                          <Properties data={data} space={space} />
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
              <div className="space-y-5">
                {data.sources.length === 0 && <p>No source recorded.</p>}
                {data.sources.map((s) => (
                  <div key={s.id} id={`source-${s.id}`}>
                    <h4 className="font-medium">
                      {s.document?.filename ||
                        s.record?.title ||
                        s.record?.external_id ||
                        "Source unavailable"}
                    </h4>
                    {s.document ? (
                      <>
                        <p>{s.document.note}</p>
                        {s.document.row ? (
                          <>
                            <p className="font-mono mt-3">
                              Row {s.document.row.line}
                            </p>
                            <dl className="mt-2 space-y-2">
                              {Object.entries(s.document.row.cells).map(
                                ([key, v]) => (
                                  <div key={key}>
                                    <dt className="label">{key}</dt>
                                    <dd className="font-mono break-words">
                                      {v || "Missing"}
                                    </dd>
                                  </div>
                                ),
                              )}
                            </dl>
                          </>
                        ) : (
                          <p>Exact row unavailable.</p>
                        )}
                        <Link
                          className="link inline-block mt-3"
                          href={`/documents?id=${encodeURIComponent(s.document.id)}&space=${encodeURIComponent(space)}`}
                        >
                          Open document
                        </Link>
                      </>
                    ) : s.record ? (
                      <>
                        <p className="text-[var(--ink-dim)]">
                          {s.record.source} · Exact source record
                        </p>
                        <pre className="overflow-auto max-h-80 whitespace-pre-wrap break-words font-mono text-xs mt-3">
                          {JSON.stringify(s.record.payload, null, 2)}
                        </pre>
                      </>
                    ) : (
                      <p>
                        The source record or CSV row is unavailable. Its
                        location was not retained or the source was removed.
                      </p>
                    )}
                  </div>
                ))}
              </div>
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
  );
}
function Properties({ data, space }: { data: Profile; space: string }) {
  return (
    <dl className="divide-y divide-[var(--border)]">
      {data.properties.length === 0 && <p>No approved properties.</p>}
      {data.properties.map((p) => (
        <div key={p.name} className="py-4 first:pt-0 flex flex-col gap-2">
          <div className="flex flex-wrap justify-between gap-2">
            <dt>{words(p.name)}</dt>
            <dd
              className={
                p.value === null ? "text-[var(--ink-faint)]" : "font-mono"
              }
            >
              {formatted(p.value, p)}
            </dd>
          </div>
          <dd className="text-xs text-[var(--ink-dim)]">
            <span>{words(p.state)}</span> ·{" "}
            <span className="font-mono">{date(p.as_of)}</span>
            {p.unit && <> · Unit: {p.unit}</>}
            {p.source ? (
              <Link
                className="link inline-block mt-1"
                href={`${objectHref(data.id, space)}&section=Sources`}
              >
                Inspect source
              </Link>
            ) : (
              <p className="mt-1">Source not recorded</p>
            )}
          </dd>
        </div>
      ))}
    </dl>
  );
}

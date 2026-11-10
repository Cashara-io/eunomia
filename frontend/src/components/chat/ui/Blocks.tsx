"use client";
import { Children, useId, useState, type ReactNode } from "react";
import Link from "next/link";
import { withUnit } from "@/lib/numbers";
import dynamic from "next/dynamic";
const MiniChart = dynamic(() => import("./Charts"), { ssr: false });
import { z } from "zod";
import { rowsOf, label, formatted, number } from "./data";
import { resolvedDataProps } from "./catalog";
import ImpactView from "../views/ImpactView";
import MetricView from "../views/MetricView";
import ScenarioView from "../views/ScenarioView";
import SubgraphView from "../views/SubgraphView";
import { parseView } from "../views";

export function Notice({ text }: { text?: string }) {
  return (
    <aside role="status" className="ui-notice">
      <p>This display could not be drawn. The source answer is below.</p>
      {text && <pre className="whitespace-pre-wrap">{text}</pre>}
    </aside>
  );
}
export function Layout({
  kind,
  title,
  columns,
  children,
}: {
  kind: string;
  title?: string;
  columns?: string;
  children?: ReactNode;
}) {
  const [active, setActive] = useState(0);
  const id = useId();
  const panels = Children.toArray(children);
  if (kind === "Accordion")
    return (
      <details className="ui-section">
        <summary>{title || "Details"}</summary>
        {children}
      </details>
    );
  if (kind === "Tabs")
    return (
      <section className="ui-section">
        <h3 className="font-medium">{title}</h3>
        <div
          role="tablist"
          aria-label={title || "Analysis views"}
          className="flex flex-wrap gap-2"
        >
          {panels.map((_, i) => (
            <button
              key={i}
              type="button"
              role="tab"
              id={`${id}-tab-${i}`}
              aria-selected={active === i}
              aria-controls={`${id}-panel-${i}`}
              className="pill"
              onClick={() => setActive(i)}
              onKeyDown={(e) => {
                if (e.key === "ArrowRight" || e.key === "ArrowLeft") {
                  e.preventDefault();
                  setActive(
                    (i + (e.key === "ArrowRight" ? 1 : panels.length - 1)) %
                      panels.length,
                  );
                }
              }}
            >
              View {i + 1}
            </button>
          ))}
        </div>
        {panels.map((p, i) => (
          <div
            key={i}
            role="tabpanel"
            id={`${id}-panel-${i}`}
            aria-labelledby={`${id}-tab-${i}`}
            hidden={i !== active}
          >
            {p}
          </div>
        ))}
      </section>
    );
  return (
    <section
      className={`ui-section ${kind === "Grid" ? `ui-grid ui-grid-${columns || "two"}` : "ui-stack"} ${kind === "Card" ? "ui-card" : ""}`}
    >
      {title && <h3 className="font-medium">{title}</h3>}
      {children}
    </section>
  );
}
export function Copy({
  kind,
  text,
  tone,
}: {
  kind: string;
  text: string;
  tone?: string;
}) {
  if (kind === "Heading")
    return <h3 className="text-reading font-semibold">{text}</h3>;
  if (kind === "Badge") return <span className="pill text-xs">{text}</span>;
  if (kind === "Caption")
    return <p className="text-xs text-[var(--ink-dim)]">{text}</p>;
  if (kind === "Callout")
    return (
      <aside className="ui-notice">
        <strong>{tone || "info"}</strong>
        <p>{text}</p>
      </aside>
    );
  return <p className="whitespace-pre-wrap">{text}</p>;
}
const scalarObject = z.record(z.string(), z.unknown());
export default function DataBlock({
  kind,
  raw,
}: {
  kind: string;
  raw: unknown;
}) {
  const parsed = resolvedDataProps.safeParse(raw);
  if (!parsed.success) return <Notice />;
  return <Block kind={kind} props={parsed.data} />;
}
function Block({
  kind,
  props,
}: {
  kind: string;
  props: z.infer<typeof resolvedDataProps>;
}) {
  const [sort, setSort] = useState<{ key: string; desc: boolean } | null>(null);
  const [page, setPage] = useState(0);
  const { title, data, summary, format } = props;
  const rows = rowsOf(data);
  const object = scalarObject.safeParse(data);
  const value = object.success ? object.data : {};
  if (["Impact", "Subgraph", "Metric", "Scenario"].includes(kind)) {
    const view = parseView(JSON.stringify(data));
    if (!view) return <Notice />;
    // Source discriminants and their props are validated by the server before persistence.
    if (view.view === "impact" && "start" in view)
      return <ImpactView view={view} />;
    if (view.view === "subgraph" && "entities" in view)
      return <SubgraphView view={view} />;
    if (view.view === "metric" && "metric" in view)
      return <MetricView view={view} />;
    if (view.view === "scenario" && "result" in view)
      return <ScenarioView view={view} />;
    return <Notice text={view.text} />;
  }
  const sourceColumns = z
    .array(
      z.object({
        key: z.string(),
        datatype: z.string(),
        unit: z.string().nullable().optional(),
      }),
    )
    .safeParse(value.columns);
  const inferred = sourceColumns.success
    ? sourceColumns.data.map((c) => ({
        key: c.key,
        label: c.key.replaceAll("_", " "),
        format:
          c.datatype === "money"
            ? "money"
            : c.datatype === "percent"
              ? "percent"
              : ["date", "datetime"].includes(c.datatype)
                ? "date"
                : c.datatype === "text"
                  ? "text"
                  : "number",
      }))
    : undefined;
  const selectedColumns =
    props.columns ||
    inferred ||
    Object.keys(rows[0] || value)
      .filter((k) => k !== "entity_id")
      .map((key) => ({
        key,
        label: key.replaceAll("_", " "),
        format:
          props.formats?.[key] ||
          (number(value[key]) != null ||
          rows.some((r) => number(r[key]) != null)
            ? format
            : "text"),
      }));
  const columns = selectedColumns.map(c => ({
    ...c,
    total: "total" in c ? c.total : undefined,
    format: props.formats?.[c.key] ?? inferred?.find(source => source.key === c.key)?.format ?? c.format,
  }));
  const totals = scalarObject.safeParse(value.totals);
  const sorted = [...rows].sort((a, b) => {
    if (!sort) return 0;
    const av = a[sort.key],
      bv = b[sort.key];
    const an = number(av),
      bn = number(bv);
    const order =
      an != null && bn != null ? an - bn : label(av).localeCompare(label(bv));
    return sort.desc ? -order : order;
  });
  const entity = (row: Record<string, unknown>, i: number) =>
    typeof row.entity_id === "string" &&
    /^[a-z_]+:[\w-]+$/.test(row.entity_id) ? (
      <Link
        className="pill link"
        href={`/entities?id=${encodeURIComponent(row.entity_id)}`}
        key={i}
      >
        {label(row.name) || row.entity_id}
      </Link>
    ) : (
      <span key={i}>{label(row.name)}</span>
    );
  let content: ReactNode;
  if (kind === "Stat")
    content = (
      <div className="text-3xl ui-numeric tabular-nums">
        {typeof value.unit === "string"
          ? withUnit(number(value.value), value.unit)
          : formatted(number(object.success ? value.value : data), format, props.unit)}
        {number(value.delta) != null && (
          <small className="block text-sm">
            Change {formatted(value.delta, format, props.unit)}
          </small>
        )}
        {rowsOf(value.trend || value.series).length > 0 && (
          <MiniChart
            kind="Sparkline"
            raw={{
              title: "Trend",
              summary,
              data: value.trend || value.series,
              x: props.x || "period",
              y: props.y,
              format,
            }}
          />
        )}
      </div>
    );
  else if (kind === "StatGroup")
    content = <div className="ui-stat-grid">{rows.flatMap((row, i) => columns.filter(c => ["number", "money", "percent"].includes(c.format ?? "") || number(row[c.key]) != null).map(c => <div className="ui-stat-tile" key={`${i}-${c.key}`}><p>{label(row.name) || c.label}</p><div className="text-3xl ui-numeric">{formatted(row[c.key], c.format, props.units?.[c.key])}</div>{row.name != null && <p>{c.label}</p>}</div>))}</div>;
  else if (kind === "Entity" || kind === "EntityList")
    content = (
      <div className="flex flex-wrap gap-2">
        {(rows.length ? rows : [value]).map(entity)}
      </div>
    );
  else if (kind === "Meter") {
    const n = number(value.value),
      max = number(value.max),
      min = number(value.min);
    content =
      n != null && min != null && max != null && max > min ? (
        <>
          <meter
            className="w-full"
            min={min}
            max={max}
            value={n}
            aria-label={title}
          />
          <p>
            {formatted(n, format, props.unit)} of {formatted(max, format, props.unit)}
          </p>
        </>
      ) : (
        <p>No range available.</p>
      );
  } else if (kind === "Comparison")
    content = (
      <dl className="grid grid-cols-2 gap-4">
        <div>
          <dt>Before</dt>
          <dd className="ui-numeric">{formatted(number(value.before), format, props.unit)}</dd>
        </div>
        <div>
          <dt>After</dt>
          <dd className="ui-numeric">{formatted(number(value.after), format, props.unit)}</dd>
        </div>
      </dl>
    );
  else if (kind === "Table")
    content = rows.length ? (
      <>
        <div className="overflow-x-auto">
          <table className="data-table w-full">
            <caption className="sr-only">{title}</caption>
            <thead>
              <tr>
                {columns.map((c) => (
                  <th
                    key={c.key}
                    scope="col"
                    aria-sort={
                      sort?.key === c.key
                        ? sort.desc
                          ? "descending"
                          : "ascending"
                        : "none"
                    }
                  >
                    <button
                      type="button"
                      onClick={() => {
                        setSort({
                          key: c.key,
                          desc: sort?.key === c.key && !sort.desc,
                        });
                        setPage(0);
                      }}
                    >
                      {c.label}
                    </button>
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {sorted.slice(page * 10, page * 10 + 10).map((r, i) => (
                <tr key={i}>
                  {columns.map((c) => (
                    <td
                      key={c.key}
                      className={
                        number(r[c.key]) != null
                          ? "ui-numeric tabular-nums text-right"
                          : ""
                      }
                    >
                      {formatted(r[c.key], c.format, props.units?.[c.key])}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
            {props.columns?.some((c) => c.total) && (
              <tfoot>
                <tr>
                  {columns.map((c, index) => {
                    const total = totals.success
                      ? scalarObject.safeParse(totals.data[c.key])
                      : null;
                    return (
                      <td key={c.key} className="ui-numeric">
                        {c.total && total?.success
                          ? formatted(total.data[c.total], c.total === "count" ? "number" : c.format, props.units?.[c.key])
                          : c.total
                            ? "none"
                            : index === 0
                              ? "Total"
                              : ""}
                      </td>
                    );
                  })}
                </tr>
              </tfoot>
            )}
          </table>
        </div>
        {rows.length > 10 && (
          <nav aria-label="Table pages" className="flex items-center gap-3">
            <button
              className="pill"
              disabled={page === 0}
              onClick={() => setPage(page - 1)}
            >
              Previous
            </button>
            <span>
              {page + 1} / {Math.ceil(rows.length / 10)}
            </span>
            <button
              className="pill"
              disabled={(page + 1) * 10 >= rows.length}
              onClick={() => setPage(page + 1)}
            >
              Next
            </button>
          </nav>
        )}
      </>
    ) : (
      <p>No rows to show.</p>
    );
  else if (kind === "Timeline")
    content = (
      <ol className="ui-timeline">
        {rows.map((r, i) => (
          <li key={i}>
            <time>
              {formatted(
                r[props.x || "date"],
                props.xFormat || props.formats?.[props.x || "date"] || "date",
              )}
            </time>
            <p>
              {formatted(
                r[props.y || "name"],
                props.formats?.[props.y || "name"] || format,
                props.units?.[props.y || "name"],
              )}
            </p>
          </li>
        ))}
      </ol>
    );
  else
    content = (
      <div
        className={
          kind === "StatGroup" ? "ui-grid ui-grid-two" : "ui-definition-groups"
        }
      >
        {(rows.length ? rows : [value]).map((r, i) => (
          <dl
            key={i}
            className="ui-definitions"
            aria-label={
              rows.length > 1 ? label(r.name) || `Row ${i + 1}` : title
            }
          >
            {columns.map((c) => (
              <div key={c.key} className="flex justify-between gap-4">
                <dt>{c.label}</dt>
                <dd className={number(r[c.key]) != null ? "ui-numeric" : ""}>
                  {formatted(r[c.key], c.format, props.units?.[c.key])}
                </dd>
              </div>
            ))}
          </dl>
        ))}
      </div>
    );
  return (
    <section className="ui-section" data-ui-component={kind}>
      <h3 className="font-medium">{title}</h3>
      {content}
      <p className="ui-summary">{summary}</p>
    </section>
  );
}

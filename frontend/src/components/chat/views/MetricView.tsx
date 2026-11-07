import Link from "next/link";
import { ChartBarIcon, WarningIcon } from "@phosphor-icons/react/ssr";
import Duo from "@/components/ui/Duo";
import StatusLabel from "@/components/ui/StatusLabel";
import { entityHref, formatNumber, words } from "./bits";
import type { MetricRun, MetricView as Metric } from "./types";

/** The run's date. A bare-date run ends at 23:59:59 UTC, so it is read in UTC to stay on its day. */
export const asOfDate = (asOf: string) =>
  new Date(asOf).toLocaleDateString("en-AU", { day: "numeric", month: "short", year: "numeric", timeZone: "UTC" });

const plural = (n: number, kind: string) => `${words(kind)}${n === 1 ? "" : "s"}`;

/** The number with its unit, or why there is none. */
export function MetricNumber({ run, size = 40 }: { run: MetricRun; size?: number }) {
  if (run.value == null) {
    return (
      <p className="text-[15px] font-medium" style={{ color: "var(--ink-dim)" }}>
        No value
      </p>
    );
  }
  return (
    <p className="flex items-baseline gap-2 min-w-0">
      <span className="font-mono font-medium leading-none" style={{ fontSize: size, letterSpacing: size >= 40 ? "-0.04em" : "-0.015em" }}>
        {formatNumber(run.value, run.unit)}
      </span>
      {run.unit && (
        <span className="font-mono text-[13px]" style={{ color: "var(--ink-dim)" }}>
          {run.unit}
        </span>
      )}
    </p>
  );
}

export function MetricWarnings({ warnings }: { warnings: string[] }) {
  if (warnings.length === 0) return null;
  return (
    <ul className="flex flex-col gap-1 text-[13px]" aria-label="Notes on this number">
      {warnings.map((w) => (
        <li key={w} className="flex items-start gap-1.5">
          <WarningIcon size={14} weight="bold" className="mt-0.5 shrink-0" aria-hidden />
          <span>{w}</span>
        </li>
      ))}
    </ul>
  );
}

/** One metric's number from the server's own run: the number and its unit, what it computes, as of
 * when, and the entities it came from, each a link. A series is a small table. */
export default function MetricView({ view }: { view: Metric }) {
  const run = view.metric;
  const more = run.n_entities - view.entities.length;
  return (
    <section className="ledger p-4 flex flex-col gap-3" aria-label={`The metric ${words(run.metric)}`}>
      <header className="flex items-start gap-2.5">
        <Duo icon={ChartBarIcon} size={18} tone="none" className="mt-0.5" />
        <div className="min-w-0 flex-1 flex flex-col gap-0.5">
          <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
            <h3 className="section-title">{words(run.metric)}</h3>
            {run.status !== "approved" && <StatusLabel tone="info">Not approved yet</StatusLabel>}
          </div>
          {run.description && <p className="text-[13px]">{run.description}</p>}
        </div>
      </header>
      {!run.series && <MetricNumber run={run} />}
      {run.series && (
        <table className="data-table w-full max-w-md" aria-label={`${words(run.metric)} by period`}>
          <thead>
            <tr>
              <th scope="col">Period</th>
              <th scope="col" className="text-right">
                Value
              </th>
              <th scope="col" className="text-right">
                From
              </th>
            </tr>
          </thead>
          <tbody>
            {run.series.map((p) => (
              <tr key={p.period}>
                <td className="font-mono">{p.period}</td>
                <td className="font-mono text-right">{p.value == null ? "none" : `${formatNumber(p.value, run.unit)}${run.unit ? ` ${run.unit}` : ""}`}</td>
                <td className="font-mono text-right">{p.n}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <p className="text-[13px]" style={{ color: "var(--ink-dim)" }}>
        {run.expr_words}. As of {asOfDate(run.as_of)}, from <span className="font-mono">{run.n_entities}</span> {plural(run.n_entities, run.kind)}.
        {run.metric_mapped && ` Asked for ${words(run.metric_mapped.from)}, which was merged into this one.`}
      </p>
      {view.entities.length > 0 && (
        <ul className="flex flex-wrap gap-1.5" aria-label="Computed from">
          {view.entities.map((e) => (
            <li key={e.id}>
              <Link href={entityHref(e)} className="pill !min-h-7 !px-2.5 text-[13px] hover:underline underline-offset-2">
                {e.name}
              </Link>
            </li>
          ))}
          {more > 0 && (
            <li className="text-[13px] self-center" style={{ color: "var(--ink-dim)" }}>
              and <span className="font-mono">{more}</span> more
            </li>
          )}
        </ul>
      )}
      <MetricWarnings warnings={run.warnings} />
    </section>
  );
}

import Link from "next/link";
import { formatScenario } from "@/lib/numbers";
import { ArrowsLeftRightIcon } from "@phosphor-icons/react/ssr";
import Duo from "@/components/ui/Duo";
import StatusLabel from "@/components/ui/StatusLabel";
import { withUnit, words } from "./bits";
import { asOfDate, MetricWarnings } from "./MetricView";
import type { ScenarioRun, ScenarioView as Scenario } from "./types";

const href = (id: string, vault: string) => `/entities?id=${encodeURIComponent(id)}&vault=${encodeURIComponent(vault)}`;

const shown = (v: unknown, unit: string | null) => (typeof v === "number" ? withUnit(v, unit) : v == null ? "none" : String(v));

/** The heading over the nearby entities: they are linked to what changed, which is not the same as affected. */
function linkedWithin(reached: ScenarioRun["reached"]): string {
  const steps = Math.max(1, ...reached.map((r) => r.depth));
  return `Linked within ${steps} ${steps === 1 ? "step" : "steps"} (not all are affected)`;
}

function MetricRow({ run, i }: { run: ScenarioRun; i: number }) {
  const before = run.baseline[i];
  const after = run.scenario[i];
  const shown = formatScenario(before.value, after.value, after.delta_pct, after.unit);
  const big = { fontVariantNumeric: "tabular-nums", letterSpacing: "-0.015em" } as const;
  return (
    <li className="soft-panel px-3 py-2.5 flex flex-col gap-1 min-w-0">
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
        <h4 className="text-[14px] font-medium">{words(before.metric)}</h4>
        {before.status !== "approved" && <StatusLabel tone="info">Not approved yet</StatusLabel>}
      </div>
      {/* Geist, not Mono, for the numbers: see MetricView */}
      <p className="flex flex-wrap items-baseline gap-x-2 text-[22px] font-medium leading-tight" style={big}>
        <span style={{ color: "var(--ink-dim)" }}>{shown.before}</span>
        <span aria-label="becomes">{"→"}</span>
        <span>{shown.after}</span>
        {after.unit && (
          <span className="font-mono text-[13px] font-normal" style={{ color: "var(--ink-dim)" }}>
            {after.unit}
          </span>
        )}
      </p>
      <p className="text-[13px]">
        <span className="font-medium">{shown.movement}</span>
        <span style={{ color: "var(--ink-dim)" }}>
          {" "}
          · {before.expr_words}, from <span style={big}>{after.n_entities}</span> {words(before.kind)}
          {after.n_entities === 1 ? "" : "s"}
        </span>
      </p>
    </li>
  );
}

/** A what-if the server ran: the headline, the changes in words, each metric before and after with its
 * move, the values it changed, the entities it reaches as chips, and that nothing was saved. */
export default function ScenarioView({ view }: { view: Scenario }) {
  const run = view.result;
  const more = run.n_changed - run.changed.length;
  return (
    <section className="ledger p-4 flex flex-col gap-3" aria-label="What if">
      <header className="flex items-start gap-2.5">
        <Duo icon={ArrowsLeftRightIcon} size={18} tone="none" className="mt-0.5" />
        <div className="min-w-0 flex-1 flex flex-col gap-1">
          <h3 className="section-title">{view.headline}</h3>
          <ul className="flex flex-col text-[13px]" aria-label="The changes">
            {run.changes.map((c, i) => (
              <li key={`${i}-${c}`}>{c}</li>
            ))}
          </ul>
        </div>
      </header>
      {run.baseline.length > 0 && (
        <ul className="grid gap-2 sm:grid-cols-2" aria-label="Metrics before and after">
          {run.baseline.map((m, i) => (
            <MetricRow key={`${m.kind}.${m.metric}`} run={run} i={i} />
          ))}
        </ul>
      )}
      {run.changed.length > 0 && (
        <div className="flex flex-col gap-1">
          <h4 className="label">Changed</h4>
          <ul className="flex flex-col gap-0.5 text-[13px]">
            {run.changed.map((c) => (
              <li key={`${c.entity_id}.${c.property}`} className="flex flex-wrap gap-x-1.5">
                <Link href={href(c.entity_id, run.vault)} className="font-medium hover:underline underline-offset-2">
                  {c.name}
                </Link>
                <span style={{ color: "var(--ink-dim)" }}>{words(c.property)}</span>
                <span style={{ fontVariantNumeric: "tabular-nums" }}>
                  {shown(c.before, c.unit)} {"→"} {shown(c.after, c.unit)}
                </span>
              </li>
            ))}
            {more > 0 && (
              <li style={{ color: "var(--ink-dim)" }}>
                and <span style={{ fontVariantNumeric: "tabular-nums" }}>{more}</span> more
              </li>
            )}
          </ul>
        </div>
      )}
      {run.removed.length > 0 && (
        <p className="text-[13px]">
          <span className="label">Removed</span> {run.removed.map((r) => r.name).join(", ")}
        </p>
      )}
      {run.reached.length > 0 && (
        <div className="flex flex-col gap-1.5">
          <h4 className="label">{linkedWithin(run.reached)}</h4>
          <ul className="flex flex-wrap gap-1.5">
            {run.reached.map((r) => (
              <li key={r.entity_id}>
                <Link href={href(r.entity_id, run.vault)} title={r.via} className="pill !min-h-7 !px-2.5 text-[13px] hover:underline underline-offset-2">
                  {r.name}
                  <span className="ml-1.5" style={{ color: "var(--ink-dim)" }}>
                    {words(r.kind)}
                  </span>
                </Link>
              </li>
            ))}
          </ul>
        </div>
      )}
      <MetricWarnings warnings={run.warnings} />
      <p className="text-[13px] font-medium border-t pt-2.5" style={{ borderColor: "var(--border)" }}>
        Nothing was saved: this is a what-if. As of {asOfDate(run.as_of)}.
      </p>
    </section>
  );
}

"use client";

import ErrorLine, { failure } from "@/components/ErrorLine";
import { useState } from "react";
import { Bar, BarChart, CartesianGrid, ResponsiveContainer, Tooltip as ChartTooltip, XAxis, YAxis } from "recharts";
import { useToolUsage } from "@/lib/queries/audit";
import type { ToolUsageReport, UsageByChannel, UsageByCredential } from "@/lib/gen";
import { PanelHead, TableSkeleton } from "./shared";

const RANGES = [
  { id: "24h", label: "24 hours", hours: 24 },
  { id: "7d", label: "7 days", hours: 168 },
  { id: "30d", label: "30 days", hours: 720 },
] as const;

const CHANNEL: Record<string, string> = { mcp: "MCP", rest: "REST API", chat: "In-app chat" };
const KIND: Record<string, string> = { session: "Browser", token: "API token", oauth: "OAuth app" };

const nf = new Intl.NumberFormat("en");
const fmt = (n: number) => nf.format(n);
const ms = (n: number) => (n >= 1000 ? `${(n / 1000).toFixed(1)} s` : `${Math.round(n)} ms`);
const pct = (part: number, whole: number) => (whole > 0 ? `${((part / whole) * 100).toFixed(part / whole < 0.1 ? 1 : 0)}%` : "0%");
const name = (email: string, id: string) => email || id;

export function UsageSection() {
  const [range, setRange] = useState<(typeof RANGES)[number]["id"]>("7d");
  const hours = RANGES.find((r) => r.id === range)!.hours;
  const usage = useToolUsage(hours);
  const report = usage.data ?? null;
  const org = report?.scope === "org";

  return (
    <div className="flex flex-col gap-7">
      <PanelHead
        title="Tool usage"
        action={
          <div role="tablist" aria-label="Time range" className="flex gap-1.5">
            {RANGES.map((r) => (
              <button
                key={r.id}
                role="tab"
                aria-selected={range === r.id}
                onClick={() => setRange(r.id)}
                className="pill"
              >
                {r.id}
              </button>
            ))}
          </div>
        }
      >
        {report ? (org ? "Every tool call made in your organisation" : "Every tool call made with your account") : "Tool calls"} over the last{" "}
        {RANGES.find((r) => r.id === range)!.label.toLowerCase()}: reads and writes, through MCP, the REST API and the in-app chat. Counts only, never
        arguments or content.
      </PanelHead>

      {usage.isError && <ErrorLine error={failure(usage.error, "Could not load tool usage.", " Reload the page to try again.")} />}

      <Summary report={report} />
      <Chart report={report} />

      {org && (
        <Table title="By person" empty="No tool calls in this range." cols={["Person", "Calls", "Errors", "Avg time"]} loading={!report}>
          {report?.by_person.map((p) => (
            <tr key={p.user_id}>
              <td className="truncate max-w-[260px]" title={p.user_id}>
                {name(p.email, p.user_id)}
              </td>
              <Counts c={p} max={report.by_person[0]?.calls ?? 0} />
            </tr>
          ))}
        </Table>
      )}

      <Table
        title="By credential"
        hint="The token, OAuth app or browser session a call was made with."
        empty="No tool calls in this range."
        cols={[...(org ? ["Person"] : []), "Credential", "Type", "Calls", "Errors", "Avg time"]}
        loading={!report}
      >
        {report?.by_credential.map((c: UsageByCredential) => (
          <tr key={`${c.user_id}|${c.kind}|${c.credential_id}`}>
            {org && (
              <td className="truncate max-w-[200px]" title={c.user_id}>
                {name(c.email, c.user_id)}
              </td>
            )}
            <td className="truncate max-w-[220px]" title={c.credential_id || undefined}>
              {c.label}
            </td>
            <td style={{ color: "var(--ink-dim)" }}>{KIND[c.kind] ?? c.kind}</td>
            <Counts c={c} max={report.by_credential[0]?.calls ?? 0} />
          </tr>
        ))}
      </Table>

      <Table title="By channel" empty="No tool calls in this range." cols={["Channel", "Calls", "Errors", "Avg time"]} loading={!report}>
        {report?.by_channel.map((c: UsageByChannel) => (
          <tr key={c.channel}>
            <td>{CHANNEL[c.channel] ?? c.channel}</td>
            <Counts c={c} max={report.by_channel[0]?.calls ?? 0} />
          </tr>
        ))}
      </Table>

      <Table
        title="By session"
        hint="One MCP session per connection, when the client sends an Mcp-Session-Id. In-app chat counts per thread."
        empty="No session ids seen in this range. Clients that do not send an Mcp-Session-Id are counted under their credential only."
        cols={[...(org ? ["Person"] : []), "Session", "Channel", "Calls", "Errors", "Avg time"]}
        loading={!report}
      >
        {report?.by_session.map((s) => (
          <tr key={`${s.user_id}|${s.channel}|${s.session}`}>
            {org && (
              <td className="truncate max-w-[200px]" title={s.user_id}>
                {name(s.email, s.user_id)}
              </td>
            )}
            <td className="font-mono text-[12px] truncate max-w-[220px]" title={s.session}>
              {s.session}
            </td>
            <td style={{ color: "var(--ink-dim)" }}>{CHANNEL[s.channel] ?? s.channel}</td>
            <Counts c={s} max={report.by_session[0]?.calls ?? 0} />
          </tr>
        ))}
      </Table>

      <Table title="By tool" empty="No tool calls in this range." cols={["Tool", "Calls", "Errors", "Avg time"]} loading={!report}>
        {report?.by_tool.map((t) => (
          <tr key={t.tool}>
            <td>
              <span className="flex items-center gap-2">
                <span className="font-mono text-[12.5px]">{t.tool}</span>
                {!t.read_only && <span className="pill">writes</span>}
              </span>
            </td>
            <Counts c={t} max={report.by_tool[0]?.calls ?? 0} />
          </tr>
        ))}
      </Table>
    </div>
  );
}

function Summary({ report }: { report: ToolUsageReport | null }) {
  const t = report?.totals;
  const cells: { label: string; value: string }[] = [
    { label: "Tool calls", value: t ? fmt(t.calls) : "–" },
    { label: "Errors", value: t ? fmt(t.errors) : "–" },
    { label: "Error rate", value: t ? pct(t.errors, t.calls) : "–" },
    { label: "Avg time", value: t && t.calls > 0 ? ms(t.avg_ms) : "–" },
  ];
  return (
    <section
      aria-label="Totals"
      className="grid grid-cols-2 lg:grid-cols-4 overflow-hidden rounded-[10px]"
      style={{ gap: "var(--hair)", background: "var(--border)", border: "var(--hair) solid var(--border)" }}
    >
      {cells.map((c) => (
        <div key={c.label} className="flex flex-col gap-2.5 px-3.5 py-3 min-w-0 bg-[var(--surface)]">
          <span className="label">{c.label}</span>
          <span className="text-[26px] leading-none tracking-[-0.02em] font-mono" style={{ color: report ? "var(--ink)" : "var(--ink-faint)" }}>
            {c.value}
          </span>
        </div>
      ))}
    </section>
  );
}

function bucketLabel(iso: string, step: string) {
  const d = new Date(iso);
  return step === "hour"
    ? d.toLocaleTimeString("en", { hour: "numeric" })
    : d.toLocaleDateString("en", { month: "short", day: "numeric", timeZone: "UTC" });
}

function Chart({ report }: { report: ToolUsageReport | null }) {
  const data = (report?.series ?? []).map((p) => ({
    bucket: p.bucket,
    ok: p.calls - p.errors,
    errors: p.errors,
  }));
  const empty = report && report.totals.calls === 0;
  return (
    <div className="ledger p-4 flex flex-col gap-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 className="section-title">Calls over time</h3>
        <span className="flex items-center gap-3 text-[12px]" style={{ color: "var(--ink-dim)" }}>
          <Swatch color="var(--series-2)" label="Succeeded" />
          <Swatch color="var(--critical)" label="Errors" />
        </span>
      </div>
      <div
        className="h-[180px] w-full"
        role="img"
        aria-label={report ? `Tool calls per ${report.series_step}, ${fmt(report.totals.calls)} in total` : "Loading chart"}
      >
        {!report ? (
          <span className="skeleton block h-full w-full" />
        ) : empty ? (
          <div className="h-full flex items-center justify-center text-[13px]" style={{ color: "var(--ink-dim)" }}>
            No tool calls in this range yet.
          </div>
        ) : (
          <ResponsiveContainer width="100%" height="100%">
            <BarChart data={data} margin={{ top: 4, right: 0, bottom: 0, left: 0 }} barCategoryGap="18%">
              <CartesianGrid vertical={false} stroke="var(--border)" />
              <XAxis
                dataKey="bucket"
                tickFormatter={(v: string) => bucketLabel(v, report.series_step)}
                tick={{ fill: "var(--ink-faint)", fontSize: 11 }}
                tickLine={false}
                axisLine={{ stroke: "var(--border)" }}
                minTickGap={28}
              />
              <YAxis
                width={36}
                allowDecimals={false}
                tick={{ fill: "var(--ink-faint)", fontSize: 11 }}
                tickLine={false}
                axisLine={false}
              />
              <ChartTooltip
                cursor={{ fill: "var(--surface-hover)" }}
                content={({ active, payload }) => {
                  if (!active || !payload?.length) return null;
                  const p = payload[0].payload as { bucket: string; ok: number; errors: number };
                  return (
                    <div className="panel px-2.5 py-2 text-[12px] flex flex-col gap-0.5" style={{ boxShadow: "var(--shadow-pop)" }}>
                      <span style={{ color: "var(--ink-dim)" }}>
                        {report.series_step === "hour"
                          ? new Date(p.bucket).toLocaleString("en", { month: "short", day: "numeric", hour: "numeric" })
                          : new Date(p.bucket).toLocaleDateString("en", { month: "short", day: "numeric", timeZone: "UTC" })}
                      </span>
                      <span className="font-mono">{fmt(p.ok + p.errors)} calls</span>
                      {p.errors > 0 && (
                        <span className="font-mono" style={{ color: "var(--critical)" }}>
                          {fmt(p.errors)} errors
                        </span>
                      )}
                    </div>
                  );
                }}
              />
              <Bar dataKey="ok" stackId="c" fill="var(--series-2)" radius={[0, 0, 0, 0]} isAnimationActive={false} />
              <Bar dataKey="errors" stackId="c" fill="var(--critical)" radius={[2, 2, 0, 0]} isAnimationActive={false} />
            </BarChart>
          </ResponsiveContainer>
        )}
      </div>
    </div>
  );
}

function Swatch({ color, label }: { color: string; label: string }) {
  return (
    <span className="flex items-center gap-1.5">
      <span aria-hidden className="inline-block h-2 w-2 rounded-[2px]" style={{ background: color }} />
      {label}
    </span>
  );
}

/** The numeric cells of a row; the calls cell carries a thin share bar so rows compare at a glance. */
function Counts({ c, max }: { c: { calls: number; errors: number; avg_ms: number }; max: number }) {
  return (
    <>
      <td className="font-mono text-[12.5px] w-[150px]">
        <span className="flex items-center gap-2">
          <span className="w-[52px] text-right">{fmt(c.calls)}</span>
          <span aria-hidden className="h-1 flex-1 rounded-full overflow-hidden" style={{ background: "var(--surface-raised)" }}>
            <span className="block h-full rounded-full" style={{ width: `${max > 0 ? Math.max(3, (c.calls / max) * 100) : 0}%`, background: "var(--series-2)" }} />
          </span>
        </span>
      </td>
      <td className="font-mono text-[12.5px] w-[90px]" style={{ color: c.errors > 0 ? "var(--critical)" : "var(--ink-faint)" }}>
        {fmt(c.errors)}
      </td>
      <td className="font-mono text-[12.5px] w-[90px]" style={{ color: "var(--ink-dim)" }}>
        {ms(c.avg_ms)}
      </td>
    </>
  );
}

function Table({
  title,
  hint,
  cols,
  empty,
  loading,
  children,
}: {
  title: string;
  hint?: string;
  cols: string[];
  empty: string;
  loading: boolean;
  children: React.ReactNode;
}) {
  const rows = Array.isArray(children) ? children.flat().filter(Boolean).length : children ? 1 : 0;
  return (
    <section className="flex flex-col gap-2.5" aria-label={title}>
      <div>
        <h3 className="section-title">{title}</h3>
        {hint && (
          <p className="text-[12.5px] mt-0.5 max-w-[62ch]" style={{ color: "var(--ink-dim)" }}>
            {hint}
          </p>
        )}
      </div>
      <div className="ledger overflow-x-auto">
        <table className="data-table">
          <thead>
            <tr>
              {cols.map((c) => (
                <th key={c}>{c}</th>
              ))}
            </tr>
          </thead>
          <tbody>
            {loading && <TableSkeleton cols={cols.length} />}
            {!loading && rows === 0 && (
              <tr>
                <td colSpan={cols.length} style={{ color: "var(--ink-dim)", height: 56, whiteSpace: "normal" }}>
                  {empty}
                </td>
              </tr>
            )}
            {children}
          </tbody>
        </table>
      </div>
    </section>
  );
}

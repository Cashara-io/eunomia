"use client";
import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { AxisBottom, AxisLeft, AxisRight } from "@visx/axis";
import { scaleBand, scaleLinear, scaleOrdinal } from "@visx/scale";
import {
  LinePath,
  AreaClosed,
  AreaStack,
  Bar,
  BarStack,
  Pie,
  Arc,
  LineRadial,
} from "@visx/shape";
import { curveLinearClosed } from "@visx/curve";
import { Group } from "@visx/group";
import { Sankey } from "@visx/sankey";
import { hierarchy, Treemap, Partition, Tree } from "@visx/hierarchy";
import { BoxPlot, ViolinPlot } from "@visx/stats";
import { useTooltip, TooltipWithBounds } from "@visx/tooltip";
import { z } from "zod";
import { resolvedDataProps } from "./catalog";
import { drawable, formatted, label, number, requiredNumber, rowsOf, type Row } from "./data";
import { Notice } from "./Blocks";

const palette = [
  "var(--space-1)",
  "var(--space-2)",
  "var(--space-3)",
  "var(--kind-file)",
  "var(--kind-symbol)",
  "var(--kind-person)",
];
function shown(value: unknown, format?: string, unit?: string | null) { return formatted(value, format, unit); }
const colour = (i: number) => palette[i % palette.length];
export default function Chart({
  kind,
  raw,
  domain,
}: {
  kind: string;
  raw: unknown;
  domain?: [number, number];
}) {
  const parsed = resolvedDataProps.safeParse(raw);
  if (!parsed.success) return <Notice />;
  const p = parsed.data;
  const inputRows = rowsOf(p.data);
  const rows = inputRows;
  const numeric =
    Object.keys(rows[0] || {}).find((k) =>
      rows.some((r) => number(r[k]) != null),
    ) || "value";
  const y = p.y || numeric;
  const x = p.x || Object.keys(rows[0] || {}).find((k) => k !== y) || "name";
  const fields = kind === "Histogram" ? ["start", "end", "count"]
    : kind === "BoxPlot" ? ["min", "q1", "median", "q3", "max"]
    : kind === "Violin" ? ["value", "count"]
    : [...new Set([y, ...(p.series || [])]), ...(["Scatter", "Bubble"].includes(kind) ? [x] : []), ...(p.size ? [p.size] : []), ...(["Gauge", "Bullet"].includes(kind) ? ["min", "max"] : [])];
  // Invalid or missing measurements must never enter a zero-default drawing accessor.
  if (rows.some(r => fields.some(key => number(r[key]) == null)))
    return <figure className="ui-section" data-ui-component={kind}><figcaption>{p.title}</figcaption><p>No numeric values to show.</p><p>{p.summary}</p></figure>;
  if (rows.some(r => Object.values(r).some(v => typeof v === "number" && !drawable(v))))
    return <Notice />;
  const sharedValues = kind === "SmallMultiples" ? rows.flatMap((r) =>
    (p.series || [y]).map((key) => requiredNumber(r[key])),
  ) : [];
  if (kind === "SmallMultiples")
    return (
      <section className="ui-section ui-multiples" data-ui-component={kind}>
        <h3>{p.title}</h3>
        <div className="ui-grid ui-grid-two">
          {(p.series?.length ? p.series : [y]).map((key) => (
            <Chart
              key={key}
              kind="Line"
              raw={{ ...p, title: key, y: key, series: [key], summary: "" }}
              domain={[
                Math.min(0, ...sharedValues),
                Math.max(1, ...sharedValues),
              ]}
            />
          ))}
        </div>
        <p>{p.summary}</p>
      </section>
    );
  return (
    <figure
      className={`ui-section ui-chart ${kind === "Sparkline" ? "ui-sparkline" : ""}`}
      aria-label={p.title}
      data-ui-component={kind}
    >
      <figcaption className="font-medium mb-3">{p.title}</figcaption>
      {rows.length &&
      !rows.some((r) => Object.values(r).some((v) => number(v) != null)) &&
      !["Network", "Tree"].includes(kind) ? (
        <p>No numeric values to show.</p>
      ) : rows.length ? (
        <div className="ui-chart-body">
          <ChartWidth>
            {({ width }) => (
              <Plot
                kind={kind}
                rows={rows}
                p={p}
                xKey={x}
                yKey={y}
                width={Math.max(180, width)}
                domain={domain}
              />
            )}
          </ChartWidth>
        </div>
      ) : (
        <p>No data to show.</p>
      )}
      <p className="text-xs text-[var(--ink-dim)]">{p.summary}</p>
    </figure>
  );
}
function ChartWidth({
  children,
}: {
  children: (size: { width: number }) => ReactNode;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  useEffect(() => {
    const element = ref.current;
    if (!element) return;
    const observer = new ResizeObserver(([entry]) =>
      setWidth(entry.contentRect.width),
    );
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  return <div ref={ref}>{children({ width })}</div>;
}
function flowHasCycle(flows: { source: string; target: string }[]): boolean {
  const adjacent = new Map<string, string[]>();
  for (const flow of flows)
    adjacent.set(flow.source, [
      ...(adjacent.get(flow.source) || []),
      flow.target,
    ]);
  const visiting = new Set<string>(),
    complete = new Set<string>();
  function visit(name: string): boolean {
    if (complete.has(name)) return false;
    if (visiting.has(name)) return true;
    visiting.add(name);
    if ((adjacent.get(name) || []).some(visit)) return true;
    visiting.delete(name);
    complete.add(name);
    return false;
  }
  return [...adjacent.keys()].some(visit);
}
type Props = {
  kind: string;
  rows: Row[];
  p: z.infer<typeof resolvedDataProps>;
  xKey: string;
  yKey: string;
  width: number;
  domain?: [number, number];
};
type TreeDatum = {
  name: string;
  value: number;
  children?: TreeDatum[];
  row?: Row;
};
function Plot({ kind, rows, p, xKey, yKey, width, domain }: Props) {
  const formatted = (value: unknown, format?: string) => shown(value, format, p.unit);
  const id = useId();
  const { tooltipData, tooltipLeft, tooltipTop, showTooltip, hideTooltip } =
    useTooltip<string>();
  const sparkline = kind === "Sparkline";
  const cartesian = [
    "Line",
    "Area",
    "StackedArea",
    "Bar",
    "GroupedBar",
    "StackedBar",
    "Histogram",
    "Scatter",
    "Bubble",
    "BoxPlot",
    "Violin",
    "Waterfall",
    "Combo",
  ].includes(kind);
  const height = sparkline
    ? 44
    : kind === "Network"
      ? Math.max(280, Math.ceil(rows.length / 2) * 28 + 40)
      : 280;
  const tickWidth =
    Math.max(
      ...rows.flatMap((r) =>
        [yKey, ...(p.series || []), "max"].map(
          (k) => formatted(r[k], p.formats?.[k] || p.format).length,
        ),
      ),
    ) *
      6.2 +
    16;
  const margin = {
    left: sparkline
      ? 6
      : cartesian
        ? Math.min(112, Math.max(58, tickWidth))
        : 12,
    right: sparkline
      ? 6
      : kind === "Combo"
        ? Math.min(100, tickWidth)
        : cartesian
          ? 18
          : 12,
    top: sparkline ? 6 : 24,
    bottom: sparkline ? 6 : cartesian ? 64 : 24,
  };
  const w = Math.max(40, width - margin.left - margin.right),
    h = height - margin.top - margin.bottom;
  const series = p.series?.length ? p.series : [yKey];
  const numbers = rows.flatMap((r) =>
    (kind === "Combo" ? series.slice(0, 1) : series)
      .map((k) => number(r[k]))
      .filter((v): v is number => v != null),
  );
  const max = Math.max(0, ...numbers),
    min = Math.min(0, ...numbers);
  const stacked = kind === "StackedBar" || kind === "StackedArea";
  const maxStack = stacked ? Math.max(
    0,
    ...rows.map((r) =>
      series.reduce((a, k) => a + Math.max(0, requiredNumber(r[k])), 0),
    ),
  ) : max;
  const scaleY = scaleLinear({
    domain:
      domain ||
      (sparkline
        ? [Math.min(...numbers), max || 1]
        : [min, (stacked ? maxStack : max) || 1]),
    range: [h, 0],
    nice: true,
  });
  const categories = rows.map(
    (r, i) =>
      formatted(r[xKey], p.formats?.[xKey] || p.xFormat || "text") ||
      String(i + 1),
  );
  const band = scaleBand({ domain: categories, range: [0, w], padding: 0.18 });
  const xNumeric = rows.every((r) => number(r[xKey]) != null);
  const continuousX =
    xNumeric &&
    !["Bar", "GroupedBar", "StackedBar", "Waterfall", "StackedArea"].includes(
      kind,
    );
  const valuesX = xNumeric ? rows.map((r) => requiredNumber(r[xKey])) : [];
  const scaleX = scaleLinear({
    domain: [Math.min(...valuesX), Math.max(...valuesX) || 1],
    range: [0, w],
    nice: true,
  });
  const x = (r: Row, i: number) =>
    continuousX
      ? scaleX(requiredNumber(r[xKey]))
      : (requiredNumber(band(categories[i]))) + band.bandwidth() / 2;
  const y = (r: Row, k = yKey) => scaleY(requiredNumber(r[k]));
  const fmt = (v: unknown) => formatted(v, p.formats?.[yKey] || p.format);
  const shares = rows.filter((r) => number(r[yKey]) != null && requiredNumber(r[yKey]) > 0);
  const total = shares.reduce(
    (sum, r) => sum + requiredNumber(r[yKey]),
    0,
  );
  const percent = (v: number, base: number) =>
    base > 0 ? `${((100 * v) / base).toFixed(1).replace(/\.0$/, "")}%` : "none";
  const numericValues = rows
    .map((r) => number(r[yKey]))
    .filter((v): v is number => v !== null);
  const heatMin = Math.min(...numericValues, Infinity),
    heatMax = Math.max(...numericValues, -Infinity);
  const tickCategories = categories.filter(
    (_, i) =>
      i %
        Math.max(
          1,
          Math.ceil(categories.length / Math.max(2, Math.floor(w / 70))),
        ) ===
      0,
  );
  function tip(r: Row) {
    return Object.entries(r)
      .map(
        ([key, v]) =>
          `${key.replaceAll("_", " ")}: ${formatted(v, p.formats?.[key] || (key === xKey ? (xNumeric ? "number" : p.xFormat || "text") : ["Histogram", "Violin"].includes(kind) && key === "count" ? "number" : p.format))}`,
      )
      .join("; ");
  }
  function mark(content: ReactNode, r: Row, key: string | number) {
    return (
      <g
        key={key}
        tabIndex={0}
        aria-label={tip(r)}
        data-chart-mark="true"
        onMouseMove={(e) => {
          e.stopPropagation();
          const rect = e.currentTarget.ownerSVGElement?.getBoundingClientRect();
          showTooltip({
            tooltipData: tip(r),
            tooltipLeft: e.clientX - (requiredNumber(rect?.left)),
            tooltipTop: e.clientY - (requiredNumber(rect?.top)),
          });
        }}
        onMouseLeave={hideTooltip}
        onFocus={(e) => {
          e.stopPropagation();
          showTooltip({
            tooltipData: tip(r),
            tooltipLeft: margin.left,
            tooltipTop: margin.top,
          });
        }}
        onBlur={hideTooltip}
      >
        {content}
        <title>{tip(r)}</title>
      </g>
    );
  }
  const axisProps = {
    stroke: "var(--border-strong)",
    tickStroke: "var(--border-strong)",
    tickLabelProps: {
      fill: "var(--ink-dim)",
      fontSize: 10,
      fontFamily: "inherit",
    },
  };
  const axes = (
    <>
      <AxisLeft
        scale={scaleY}
        numTicks={4}
        tickFormat={(v) => fmt(v)}
        {...axisProps}
      />
      <AxisBottom
        top={h}
        scale={continuousX ? scaleX : band}
        numTicks={Math.max(2, Math.floor(w / 85))}
        tickValues={continuousX ? undefined : tickCategories}
        label={xKey.replaceAll("_", " ")}
        labelOffset={26}
        labelProps={{
          fill: "var(--ink-dim)",
          fontSize: 11,
          fontFamily: "inherit",
        }}
        tickFormat={(v) =>
          continuousX
            ? formatted(v, p.formats?.[xKey] || p.xFormat || "number")
            : label(v).slice(0, width < 420 ? 10 : 18)
        }
        {...axisProps}
      />
    </>
  );
  let drawing: ReactNode;
  let gridY = scaleY;
  let showAxes = !sparkline;
  if (
    kind === "Line" ||
    kind === "Area" ||
    kind === "Sparkline" ||
    kind === "Combo"
  ) {
    const right = scaleLinear({
      domain: [
        Math.min(
          0,
          ...rows.map((r) => requiredNumber(r[p.size || series[1] || yKey])),
        ),
        Math.max(
          1,
          ...rows.map((r) => requiredNumber(r[p.size || series[1] || yKey])),
        ),
      ],
      range: [h, 0],
      nice: true,
    });
    drawing = (
      <>
        {series.map((key, s) => (
          <g key={key}>
            {kind === "Combo" && s === 0 ? (
              rows.map((r, i) =>
                mark(
                  <Bar
                    x={requiredNumber(band(categories[i]))}
                    y={y(r, key)}
                    width={band.bandwidth()}
                    height={Math.max(0, h - y(r, key))}
                    fill={colour(s)}
                  />,
                  r,
                  i,
                ),
              )
            ) : (
              <>
                {kind === "Area" && (
                  <AreaClosed
                    data={rows}
                    defined={(r) => number(r[key]) != null}
                    x={x}
                    y={(r) => y(r, key)}
                    yScale={scaleY}
                    fill={colour(s)}
                    fillOpacity={0.18}
                  />
                )}
                <LinePath
                  data={rows}
                  defined={(r) => number(r[key]) != null}
                  x={x}
                  y={(r) =>
                    kind === "Combo" && s === 1
                      ? right(requiredNumber(r[key]))
                      : y(r, key)
                  }
                  stroke={colour(s)}
                  strokeWidth={2}
                  strokeDasharray={s % 2 ? "5 3" : undefined}
                />
                {rows.map(
                  (r, i) =>
                    number(r[key]) != null &&
                    mark(
                      <g>
                        <circle
                          cx={x(r, i)}
                          cy={
                            kind === "Combo" && s === 1
                              ? right(requiredNumber(r[key]))
                              : y(r, key)
                          }
                          r={10}
                          fill="transparent"
                        />
                        <circle
                          cx={x(r, i)}
                          cy={
                            kind === "Combo" && s === 1
                              ? right(requiredNumber(r[key]))
                              : y(r, key)
                          }
                          r={sparkline ? (i === rows.length - 1 ? 3 : 0) : 3}
                          fill={colour(s)}
                        />
                      </g>,
                      r,
                      i,
                    ),
                )}
              </>
            )}
          </g>
        ))}
        {kind === "Combo" && series.length > 1 && (
          <AxisRight
            left={w}
            scale={right}
            tickFormat={(v) => formatted(v, p.formats?.[series[1]] || p.format)}
            numTicks={4}
            {...axisProps}
          />
        )}
      </>
    );
  } else if (kind === "StackedArea") {
    drawing = (
      <AreaStack
        data={rows}
        keys={series}
        x={(d) => x(d.data, rows.indexOf(d.data))}
        y0={(d) => scaleY(d[0])}
        y1={(d) => scaleY(d[1])}
      >
        {({ stacks, path }) =>
          stacks.map((stack, i) => (
            <g key={stack.key}>
              <path d={path(stack) || ""} fill={colour(i)} fillOpacity={0.7} />
              {stack.map((point, j) =>
                mark(
                  <circle
                    cx={x(point.data, j)}
                    cy={scaleY(point[1])}
                    r={6}
                    fill={colour(i)}
                  />,
                  point.data,
                  j,
                ),
              )}
            </g>
          ))
        }
      </AreaStack>
    );
  } else if (kind === "StackedBar") {
    const numericRows = rows.map((r, i) =>
      Object.fromEntries([
        [xKey, categories[i]],
        ...series.map((k) => [k, requiredNumber(r[k])]),
      ]),
    );
    const colours = scaleOrdinal({
      domain: series,
      range: series.map((_, i) => colour(i)),
    });
    drawing = (
      <BarStack
        data={numericRows}
        keys={series}
        x={(r) => String(r[xKey])}
        xScale={band}
        yScale={scaleY}
        color={colours}
      >
        {(stacks) =>
          stacks.flatMap((stack) =>
            stack.bars.map((bar) =>
              mark(
                <Bar
                  x={bar.x}
                  y={bar.y}
                  width={bar.width}
                  height={Math.max(0, bar.height)}
                  fill={bar.color}
                />,
                rows[bar.index],
                `${stack.index}-${bar.index}`,
              ),
            ),
          )
        }
      </BarStack>
    );
  } else if (["Bar", "GroupedBar", "Waterfall", "Histogram"].includes(kind)) {
    if (kind === "Bar" && p.orientation === "horizontal") {
      showAxes = false;
      const vertical = scaleBand({
        domain: categories,
        range: [0, h],
        padding: 0.2,
      });
      const horizontal = scaleLinear({
        domain: [min, max || 1],
        range: [0, w],
        nice: true,
      });
      drawing = (
        <>
          <AxisLeft scale={vertical} {...axisProps} />
          <AxisBottom
            top={h}
            scale={horizontal}
            numTicks={4}
            tickFormat={(v) => fmt(v)}
            {...axisProps}
          />
          {rows.map((r, i) =>
            mark(
              <Bar
                x={Math.min(horizontal(0), horizontal(requiredNumber(r[yKey])))}
                y={requiredNumber(vertical(categories[i]))}
                width={Math.abs(
                  horizontal(requiredNumber(r[yKey])) - horizontal(0),
                )}
                height={vertical.bandwidth()}
                fill={colour(0)}
              />,
              r,
              i,
            ),
          )}
        </>
      );
    } else if (kind === "Histogram") {
      const bins = rows.map((r) => ({
        start: requiredNumber(r.start),
        end: requiredNumber(r.end),
        count: requiredNumber(r.count),
      }));
      const lo = Math.min(...bins.map((b) => b.start)),
        hi = Math.max(...bins.map((b) => b.end));
      const bx = scaleLinear({
        domain: [lo, hi === lo ? lo + 1 : hi],
        range: [0, w],
      });
      const by = scaleLinear({
        domain: [0, Math.max(1, ...bins.map((b) => b.count))],
        range: [h, 0],
        nice: true,
      });
      gridY = by;
      showAxes = false;
      drawing = (
        <>
          <AxisLeft
            scale={by}
            numTicks={Math.min(4, Math.max(...bins.map((b) => b.count)))}
            tickFormat={(v) => formatted(v, "number")}
            {...axisProps}
          />
          <AxisBottom
            top={h}
            scale={bx}
            numTicks={4}
            tickFormat={(v) => fmt(v)}
            {...axisProps}
          />
          {bins.map((b, i) =>
            mark(
              <Bar
                x={bx(b.start)}
                y={by(b.count)}
                width={Math.max(1, bx(b.end) - bx(b.start) - 1)}
                height={h - by(b.count)}
                fill={colour(0)}
              />,
              b,
              i,
            ),
          )}
        </>
      );
    } else {
      const waterfalls = rows.reduce<{ start: number; end: number }[]>(
        (out, r) => {
          const start = out.length ? requiredNumber(out.at(-1)?.end) : 0;
          return [...out, { start, end: start + (requiredNumber(r[yKey])) }];
        },
        [],
      );
      const wy = scaleLinear({
        domain: [
          Math.min(0, ...waterfalls.map((v) => Math.min(v.start, v.end))),
          Math.max(1, ...waterfalls.map((v) => Math.max(v.start, v.end))),
        ],
        range: [h, 0],
        nice: true,
      });
      if (kind === "Waterfall") gridY = wy;
      drawing = (
        <>
          {rows.flatMap((r, i) =>
            series.map((key, s) => {
              const n = number(r[key]);
              if (n == null) return null;
              const start = kind === "Waterfall" ? waterfalls[i].start : 0,
                end = kind === "Waterfall" ? waterfalls[i].end : n;
              const sy = kind === "Waterfall" ? wy : scaleY;
              return mark(
                <Bar
                  x={
                    (requiredNumber(band(categories[i]))) +
                    (s * band.bandwidth()) / series.length
                  }
                  y={Math.min(sy(start), sy(end))}
                  width={band.bandwidth() / series.length}
                  height={Math.max(1, Math.abs(sy(start) - sy(end)))}
                  fill={colour(s)}
                />,
                r,
                `${i}-${s}`,
              );
            }),
          )}
          {kind === "Waterfall" && (
            <>
              <AxisLeft
                scale={wy}
                numTicks={4}
                tickFormat={(v) => fmt(v)}
                {...axisProps}
              />
              <AxisBottom top={h} scale={band} {...axisProps} />
            </>
          )}
        </>
      );
      if (kind === "Waterfall") showAxes = false;
    }
  } else if (kind === "Scatter" || kind === "Bubble") {
    drawing = rows.map(
      (r, i) =>
        number(r[xKey]) != null &&
        number(r[yKey]) != null &&
        mark(
          <circle
            cx={x(r, i)}
            cy={y(r)}
            r={
              kind === "Bubble"
                ? Math.max(
                    3,
                    24 *
                      Math.sqrt(
                        Math.abs(requiredNumber(r[p.size || series[1] || yKey])) /
                          Math.max(
                            1,
                            ...rows.map((row) =>
                              Math.abs(
                                requiredNumber(row[p.size || series[1] || yKey]),
                              ),
                            ),
                          ),
                      ),
                  )
                : 4
            }
            fill={colour(0)}
            fillOpacity={0.7}
          />,
          r,
          i,
        ),
    );
  } else if (kind === "Pie" || kind === "Donut") {
    showAxes = false;
    const radius = Math.min(w, h) / 2;
    drawing = (
      <Group left={w / 2} top={h / 2}>
        {kind === "Donut" && (
          <text
            className="ui-donut-total"
            textAnchor="middle"
            dy="0.35em"
            style={{
              fontSize: Math.min(
                22,
                ((radius * 0.95) / Math.max(1, fmt(total).length)) * 1.6,
              ),
            }}
          >
            {fmt(total)}
          </text>
        )}
        <Pie
          data={shares}
          pieValue={(r) => requiredNumber(r[yKey])}
          outerRadius={radius}
          innerRadius={kind === "Donut" ? radius * 0.6 : 0}
          padAngle={0.02}
        >
          {(pie) =>
            pie.arcs.map((arc, i) =>
              mark(
                <>
                  <path d={pie.path(arc) || ""} fill={colour(i)} />
                  {arc.endAngle - arc.startAngle > 0.42 && radius > 65 && (
                    <text
                      x={
                        Math.sin((arc.startAngle + arc.endAngle) / 2) *
                        radius *
                        (kind === "Donut" ? 0.8 : 0.65)
                      }
                      y={
                        -Math.cos((arc.startAngle + arc.endAngle) / 2) *
                        radius *
                        (kind === "Donut" ? 0.8 : 0.65)
                      }
                      textAnchor="middle"
                      dy="0.35em"
                      className="ui-on-fill"
                    >
                      {percent(requiredNumber(arc.data[yKey]), total)}
                    </text>
                  )}
                </>,
                arc.data,
                i,
              ),
            )
          }
        </Pie>
      </Group>
    );
  } else if (kind === "BoxPlot" || kind === "Violin") {
    const box = rows[0];
    const stats = scaleLinear({
      domain: [
        number(box.min) ??
          Math.min(0, ...rows.map((r) => requiredNumber(r.value))),
        number(box.max) ??
          Math.max(1, ...rows.map((r) => requiredNumber(r.value))),
      ],
      range: [h, 0],
      nice: true,
    });
    gridY = stats;
    showAxes = false;
    drawing = (
      <>
        <AxisLeft
          scale={stats}
          numTicks={4}
          tickFormat={(v) => fmt(v)}
          {...axisProps}
        />
        {kind === "BoxPlot"
          ? mark(
              <BoxPlot
                min={requiredNumber(box.min)}
                max={requiredNumber(box.max)}
                firstQuartile={requiredNumber(box.q1)}
                thirdQuartile={requiredNumber(box.q3)}
                median={requiredNumber(box.median)}
                valueScale={stats}
                left={w / 2 - 25}
                boxWidth={50}
                fill={colour(0)}
                stroke="var(--ink)"
              />,
              box,
              "box",
            )
          : mark(
              <ViolinPlot
                data={rows}
                value={(r) => requiredNumber(r.value)}
                count={(r) => requiredNumber(r.count)}
                valueScale={stats}
                left={w / 2 - 40}
                width={80}
                fill={colour(0)}
                stroke="var(--ink)"
              />,
              {
                min: Math.min(...numericValues),
                max: Math.max(...numericValues),
                count: rows.reduce((sum, r) => sum + (requiredNumber(r.count)), 0),
              },
              "violin",
            )}
      </>
    );
  } else if (kind === "Heatmap" || kind === "CalendarHeatmap") {
    showAxes = false;
    const dates = rows.map((r) => new Date(label(r[xKey])));
    const calendar = kind === "CalendarHeatmap";
    const valid = dates.every((d) => Number.isFinite(d.valueOf()));
    const earliest = valid
      ? new Date(Math.min(...dates.map((d) => d.valueOf())))
      : new Date(0);
    const origin = earliest.valueOf() - earliest.getUTCDay() * 86400000;
    const weekdays = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const cells = rows.map((r, i) => ({
      row: r,
      x:
        calendar && valid
          ? String(Math.floor((dates[i].valueOf() - origin) / 604800000))
          : categories[i],
      y:
        calendar && valid
          ? weekdays[dates[i].getUTCDay()]
          : label(r[p.group || "group"]),
    }));
    const xs =
      calendar && valid
        ? Array.from(
            { length: Math.max(...cells.map((c) => Number(c.x))) + 1 },
            (_, i) => String(i),
          )
        : [...new Set(cells.map((c) => c.x))];
    const ys =
      calendar && valid ? weekdays : [...new Set(cells.map((c) => c.y))];
    const left = Math.min(70, Math.max(32, ...ys.map((v) => v.length * 6 + 8)));
    const xb = scaleBand({ domain: xs, range: [left, w], padding: 0.06 });
    const yb = scaleBand({ domain: ys, range: [0, h], padding: 0.06 });
    const months = xs.filter(
      (week, i) =>
        i === 0 ||
        new Date(origin + (Number(week) * 7 + 6) * 86400000).getUTCMonth() !==
          new Date(
            origin + ((Number(week) - 1) * 7 + 6) * 86400000,
          ).getUTCMonth(),
    );
    drawing = (
      <>
        {calendar && valid ? (
          months.map((week) => (
            <text key={week} x={xb(week)} y={-8}>
              {new Date(
                origin + (Number(week) * 7 + 6) * 86400000,
              ).toLocaleDateString("en-AU", {
                month: "short",
                timeZone: "UTC",
              })}
            </text>
          ))
        ) : (
          <AxisBottom
            top={h}
            scale={xb}
            tickValues={xs.filter(
              (_, i) =>
                i % Math.max(1, Math.ceil(xs.length / Math.max(2, w / 70))) ===
                0,
            )}
            {...axisProps}
          />
        )}
        <AxisLeft left={left} scale={yb} {...axisProps} />
        {calendar &&
          xs.flatMap((week) =>
            ys.map((day) => (
              <Bar
                key={`${week}-${day}`}
                x={requiredNumber(xb(week))}
                y={requiredNumber(yb(day))}
                width={xb.bandwidth()}
                height={yb.bandwidth()}
                fill="var(--surface-raised)"
              />
            )),
          )}
        {cells.map((cell, i) => {
          const value = number(cell.row[yKey]);
          const intensity =
            value == null
              ? 0
              : 0.2 +
                0.8 *
                  (heatMax === heatMin
                    ? 1
                    : (value - heatMin) / (heatMax - heatMin));
          return mark(
            <>
              <Bar
                x={requiredNumber(xb(cell.x))}
                y={requiredNumber(yb(cell.y))}
                width={xb.bandwidth()}
                height={yb.bandwidth()}
                fill={colour(0)}
                fillOpacity={intensity}
              />
              {xb.bandwidth() > fmt(value).length * 6 + 8 &&
                yb.bandwidth() > 24 && (
                  <text
                    x={(requiredNumber(xb(cell.x))) + xb.bandwidth() / 2}
                    y={(requiredNumber(yb(cell.y))) + yb.bandwidth() / 2}
                    textAnchor="middle"
                    dy="0.35em"
                    className="ui-cell-label ui-label-halo"
                  >
                    {fmt(value)}
                  </text>
                )}
            </>,
            cell.row,
            i,
          );
        })}
      </>
    );
  } else if (kind === "Treemap" || kind === "Partition" || kind === "Tree") {
    showAxes = false;
    const children = (
      parent: string | null,
      depth: number,
      seen: Set<string>,
    ): TreeDatum[] => {
      if (depth > 12) return [];
      return rows.flatMap((r, i) => {
        const key = label(r.entity_id || r.id || r.name || categories[i]);
        const parentKey = label(r[p.parent || "parent"]);
        const hasParent = rows.some(
          (other) =>
            label(other.entity_id || other.id || other.name) === parentKey,
        );
        if (
          seen.has(key) ||
          (parent === null ? hasParent : parentKey !== parent)
        )
          return [];
        const next = new Set(seen);
        next.add(key);
        const nested = children(key, depth + 1, next);
        return [
          {
            name: categories[i],
            row: r,
            value: nested.length ? 0 : Math.max(0, requiredNumber(r[yKey])),
            ...(nested.length ? { children: nested } : {}),
          },
        ];
      });
    };
    const root = hierarchy<TreeDatum>({
      name: p.title,
      value: 0,
      children: children(null, 0, new Set()),
    }).sum((d) => d.value);
    if (kind === "Treemap")
      drawing = (
        <Treemap root={root} size={[w, h]} paddingInner={2}>
          {(tree) =>
            tree.leaves().map((n, i) =>
              mark(
                <>
                  <Bar
                    x={n.x0}
                    y={n.y0}
                    width={Math.max(0, n.x1 - n.x0)}
                    height={Math.max(0, n.y1 - n.y0)}
                    fill={colour(i)}
                  />
                  <text x={n.x0 + 4} y={n.y0 + 15} className="ui-on-fill">
                    {n.x1 - n.x0 > 45 && n.y1 - n.y0 > 20
                      ? n.data.name.slice(0, Math.floor((n.x1 - n.x0 - 8) / 6))
                      : ""}
                  </text>
                </>,
                n.data.row || { name: n.data.name },
                i,
              ),
            )
          }
        </Treemap>
      );
    else if (kind === "Partition")
      drawing = (
        <Partition
          root={root}
          size={[w, (h * (root.height + 1)) / Math.max(1, root.height)]}
        >
          {(tree) =>
            tree
              .descendants()
              .filter((n) => n.depth > 0)
              .map((n, i) =>
                mark(
                  <>
                    <Bar
                      x={n.x0}
                      y={n.y0 - h / Math.max(1, root.height)}
                      width={Math.max(0, n.x1 - n.x0 - 2)}
                      height={Math.max(0, n.y1 - n.y0 - 2)}
                      fill={colour(i)}
                    />
                    {n.x1 - n.x0 > 50 && n.y1 - n.y0 > 22 && (
                      <text
                        x={n.x0 + 6}
                        y={n.y0 - h / Math.max(1, root.height) + 18}
                        className="ui-on-fill"
                      >
                        {n.data.name.slice(
                          0,
                          Math.floor((n.x1 - n.x0 - 12) / 6),
                        )}
                      </text>
                    )}
                  </>,
                  n.data.row || { name: n.data.name },
                  i,
                ),
              )
          }
        </Partition>
      );
    else
      drawing = (
        <Tree root={root} size={[h, Math.max(40, w - 100)]}>
          {(tree) => (
            <>
              {tree.links().map((link, i) => (
                <LinePath
                  key={i}
                  data={[link.source, link.target]}
                  x={(d) => d.y}
                  y={(d) => d.x}
                  stroke="var(--border-strong)"
                />
              ))}
              {tree.descendants().map((n, i) =>
                mark(
                  <>
                    <circle cx={n.y} cy={n.x} r={5} fill={colour(0)} />
                    <text x={n.y + 8} y={n.x + 4}>
                      {n.data.name.slice(0, 12)}
                    </text>
                  </>,
                  n.data.row || { name: n.data.name },
                  i,
                ),
              )}
            </>
          )}
        </Tree>
      );
  } else if (kind === "Sankey") {
    showAxes = false;
    const targetKey = p.group || "target";
    const flows = rows
      .filter((r) => number(r[yKey]) != null && (requiredNumber(r[yKey])) > 0)
      .map((r) => ({
        source: label(r[xKey]),
        target: label(r[targetKey]),
        value: requiredNumber(r[yKey]),
      }));
    const names = [...new Set(flows.flatMap((r) => [r.source, r.target]))];
    const labelRoom = Math.min(
      width < 420 ? 65 : 100,
      Math.max(36, ...names.map((name) => name.length * 6 + 10)),
    );
    const flowWidth = Math.max(40, w - 2 * labelRoom);
    if (flowHasCycle(flows))
      drawing = (
        <text x={0} y={20}>
          This flow has a cycle. Use a Network chart.
        </text>
      );
    else if (!flows.length)
      drawing = (
        <text x={0} y={20}>
          No positive flow values to show.
        </text>
      );
    else
      drawing = (
        <Group left={labelRoom}>
          <Sankey
            root={{ nodes: names.map((name) => ({ name })), links: flows }}
            nodeId={(n) => n.name}
            size={[flowWidth, h]}
            nodeWidth={10}
            nodePadding={24}
          >
            {({ graph, createPath }) => (
              <>
                {graph.links.map((link, i) =>
                  mark(
                    <path
                      d={createPath(link) || ""}
                      fill="none"
                      stroke={colour(names.indexOf(label(link.source)))}
                      strokeWidth={Math.max(1, requiredNumber(link.width))}
                      strokeOpacity={0.4}
                    />,
                    {
                      [xKey]: flows[i].source,
                      [targetKey]: flows[i].target,
                      [yKey]: flows[i].value,
                    },
                    i,
                  ),
                )}
                {graph.nodes.map((node, i) =>
                  mark(
                    <>
                      <Bar
                        x={requiredNumber(node.x0)}
                        y={requiredNumber(node.y0)}
                        width={(requiredNumber(node.x1)) - (requiredNumber(node.x0))}
                        height={(requiredNumber(node.y1)) - (requiredNumber(node.y0))}
                        fill={colour(i)}
                      />
                      <text
                        x={
                          (requiredNumber(node.x0)) > w / 2
                            ? (requiredNumber(node.x0)) - 8
                            : (requiredNumber(node.x1)) + 8
                        }
                        y={((requiredNumber(node.y0)) + (requiredNumber(node.y1))) / 2}
                        dy="0.35em"
                        textAnchor={(requiredNumber(node.x0)) > w / 2 ? "end" : "start"}
                        className="ui-label-halo"
                      >
                        {node.name.slice(0, Math.floor((labelRoom - 8) / 6))}
                      </text>
                    </>,
                    { name: node.name, [yKey]: node.value },
                    node.name,
                  ),
                )}
              </>
            )}
          </Sankey>
        </Group>
      );
  } else if (kind === "Network") {
    showAxes = false;
    const targetKey = p.group || "target";
    const data = [...rows];
    for (const r of rows) {
      const target = r[targetKey];
      if (
        target != null &&
        !data.some((n) => [n.id, n.entity_id, n[xKey]].includes(target))
      )
        data.push({ [xKey]: target });
    }
    const labelRoom = Math.min(88, w * 0.34);
    const perSide = Math.ceil(data.length / 2);
    const nodes = data.map((r, i) => {
      const right = i < perSide;
      const index = right ? i : data.length - 1 - i;
      const sideCount = right ? perSide : data.length - perSide;
      const cy = ((index + 0.5) * h) / Math.max(1, sideCount);
      const normal = (cy - h / 2) / (h / 2);
      return {
        datum: r,
        right,
        x:
          w / 2 +
          (right ? 1 : -1) *
            Math.sqrt(Math.max(0, 1 - normal * normal)) *
            Math.max(12, w / 2 - labelRoom),
        y: cy,
      };
    });
    drawing = (
      <>
        {nodes.map((node, i) => {
          const target = nodes.find(
            (n) =>
              node.datum[targetKey] != null &&
              [n.datum.id, n.datum.entity_id, n.datum[xKey]].includes(
                node.datum[targetKey],
              ),
          );
          return target
            ? mark(
                <line
                  x1={node.x}
                  y1={node.y}
                  x2={target.x}
                  y2={target.y}
                  stroke="var(--border-strong)"
                  strokeWidth={1.5}
                />,
                node.datum,
                `link-${i}`,
              )
            : null;
        })}
        {nodes.map((node, i) =>
          mark(
            <>
              <circle
                cx={node.x}
                cy={node.y}
                r={5}
                fill={colour(0)}
                stroke="var(--surface)"
                strokeWidth={2}
              />
              <text
                x={node.x + (node.right ? 9 : -9)}
                y={node.y}
                dy="0.35em"
                textAnchor={node.right ? "start" : "end"}
              >
                {label(node.datum[xKey]).slice(
                  0,
                  Math.floor((labelRoom - 12) / 6),
                )}
              </text>
            </>,
            node.datum,
            `node-${i}`,
          ),
        )}
      </>
    );
  } else if (kind === "Radar") {
    showAxes = false;
    const radius = Math.max(24, Math.min(w, h) / 2 - 35);
    const radial = scaleLinear({ domain: [0, max || 1], range: [0, radius] });
    drawing = (
      <Group left={w / 2} top={h / 2}>
        {[0.25, 0.5, 0.75, 1].map((f) => (
          <circle key={f} r={radius * f} fill="none" stroke="var(--border)" />
        ))}
        {categories.map((category, i) => {
          const a = (i / categories.length) * Math.PI * 2;
          return (
            <g key={i}>
              <line
                x2={Math.sin(a) * radius}
                y2={-Math.cos(a) * radius}
                stroke="var(--border-strong)"
              />
              <text
                x={Math.sin(a) * (radius + 8)}
                y={-Math.cos(a) * (radius + 8)}
                textAnchor="middle"
              >
                {category.slice(0, 12)}
              </text>
            </g>
          );
        })}
        {series.map((key, s) => (
          <g key={key}>
            <LineRadial
              data={rows}
              angle={(_, i) => (i / rows.length) * Math.PI * 2}
              radius={(r) => radial(requiredNumber(r[key]))}
              curve={curveLinearClosed}
              stroke={colour(s)}
              fill={colour(s)}
              fillOpacity={0.12}
            />
            {rows.map((r, i) =>
              mark(
                <circle
                  cx={
                    Math.sin((i / rows.length) * Math.PI * 2) *
                    radial(requiredNumber(r[key]))
                  }
                  cy={
                    -Math.cos((i / rows.length) * Math.PI * 2) *
                    radial(requiredNumber(r[key]))
                  }
                  r={5}
                  fill={colour(s)}
                />,
                r,
                i,
              ),
            )}
          </g>
        ))}
      </Group>
    );
  } else if (kind === "Funnel") {
    showAxes = false;
    const step = h / rows.length;
    drawing = rows.map((r, i) => {
      const value = Math.max(0, requiredNumber(r[yKey]));
      const barWidth = (value / (max || 1)) * w;
      const previous = i === 0 ? requiredNumber(r[yKey]) : requiredNumber(rows[i - 1][yKey]);
      const first = requiredNumber(rows[0]?.[yKey]);
      return mark(
        <>
          <text x={0} y={i * step + 12}>
            {categories[i]} · {fmt(value)}
          </text>
          <Bar
            x={(w - barWidth) / 2}
            y={i * step + 21}
            width={barWidth}
            height={Math.max(3, step - 48)}
            fill={colour(0)}
          />
          <text x={w / 2} y={(i + 1) * step - 7} textAnchor="middle">
            {i === 0
              ? "100% of first"
              : `${percent(value, previous)} of previous · ${percent(value, first)} of first`}
          </text>
        </>,
        {
          ...r,
          from_previous: i ? percent(value, previous) : "First stage",
          from_first: percent(value, first),
        },
        i,
      );
    });
  } else if (kind === "Gauge" || kind === "Bullet") {
    showAxes = false;
    const r = rows[0],
      n = requiredNumber(r[yKey]),
      lo = requiredNumber(r.min),
      hi = number(r.max);
    if (hi == null || hi <= lo)
      drawing = (
        <text x={w / 2} y={h / 2} textAnchor="middle">
          No range available.
        </text>
      );
    else {
      const fraction = Math.max(0, Math.min(1, (n - lo) / (hi - lo))),
        radius = Math.min(w / 2, h);
      drawing = mark(
        kind === "Gauge" ? (
          <Group left={w / 2} top={h}>
            <Arc
              innerRadius={radius * 0.75}
              outerRadius={radius}
              startAngle={-Math.PI / 2}
              endAngle={Math.PI / 2}
              fill="var(--surface-raised)"
            />
            <Arc
              innerRadius={radius * 0.75}
              outerRadius={radius}
              startAngle={-Math.PI / 2}
              endAngle={-Math.PI / 2 + Math.PI * fraction}
              fill={colour(0)}
            />
            <text y={-30} textAnchor="middle" className="ui-range-value">
              {fmt(n)}
            </text>
            <text x={-radius} y={18}>
              {fmt(lo)}
            </text>
            <text x={radius} y={18} textAnchor="end">
              {fmt(hi)}
            </text>
          </Group>
        ) : (
          <>
            <Bar
              x={0}
              y={h / 2 - 15}
              width={w}
              height={30}
              fill="var(--surface-raised)"
            />
            <Bar
              x={0}
              y={h / 2 - 8}
              width={w * fraction}
              height={16}
              fill={colour(0)}
            />
            <text
              x={w / 2}
              y={h / 2 - 22}
              textAnchor="middle"
              className="ui-range-value"
            >
              {fmt(n)}
            </text>
            <text x={0} y={h / 2 + 36}>
              {fmt(lo)}
            </text>
            <text x={w} y={h / 2 + 36} textAnchor="end">
              {fmt(hi)}
            </text>
            {number(r.target) != null && (
              <line
                x1={
                  w *
                  Math.max(
                    0,
                    Math.min(1, ((requiredNumber(r.target)) - lo) / (hi - lo)),
                  )
                }
                x2={
                  w *
                  Math.max(
                    0,
                    Math.min(1, ((requiredNumber(r.target)) - lo) / (hi - lo)),
                  )
                }
                y1={h / 2 - 20}
                y2={h / 2 + 20}
                stroke="var(--ink)"
                strokeWidth={2}
              />
            )}
          </>
        ),
        r,
        "range",
      );
    }
  } else drawing = null;
  return (
    <div className="ui-plot" style={{ width: "100%", position: "relative" }}>
      <svg
        width={width}
        height={height}
        role="img"
        onMouseLeave={hideTooltip}
        aria-labelledby={`${id}-name ${id}-summary`}
        data-chart={kind}
      >
        <title id={`${id}-name`}>{p.title}</title>
        <desc id={`${id}-summary`}>{p.summary}</desc>
        <Group left={margin.left} top={margin.top}>
          {cartesian &&
            !(kind === "Bar" && p.orientation === "horizontal") &&
            !sparkline &&
            gridY
              .ticks(4)
              .map((v) => (
                <line
                  key={v}
                  x2={w}
                  y1={gridY(v)}
                  y2={gridY(v)}
                  stroke="var(--border)"
                  strokeOpacity={0.65}
                />
              ))}
          {showAxes && axes}
          {drawing}
        </Group>
      </svg>
      {(kind === "Heatmap" || kind === "CalendarHeatmap") &&
        numericValues.length > 0 && (
          <div
            className="ui-colour-scale"
            aria-label={`Colour scale: ${fmt(heatMin)} to ${fmt(heatMax)}`}
          >
            <span>{fmt(heatMin)}</span>
            <span className="ui-scale-steps">
              {[0.2, 0.4, 0.6, 0.8, 1].map((opacity) => (
                <i key={opacity} style={{ background: colour(0), opacity }} />
              ))}
            </span>
            <span>{fmt(heatMax)}</span>
          </div>
        )}
      {((series.length > 1 && !sparkline) ||
        kind === "Pie" ||
        kind === "Donut") && (
        <ul
          className={`ui-legend ${kind === "Pie" || kind === "Donut" ? "ui-share-legend" : ""}`}
          aria-label="Legend"
        >
          {(kind === "Pie" || kind === "Donut"
            ? shares.map((r) =>
                formatted(r[xKey], p.formats?.[xKey] || p.xFormat || "text"),
              )
            : series
          ).map((name, i) => (
            <li key={i}>
              <i style={{ background: colour(i) }} />
              <span>{name.replaceAll("_", " ")}</span>
              {(kind === "Pie" || kind === "Donut") && (
                <span className="ui-legend-value">
                  {fmt(shares[i][yKey])}{" "}
                  <span className="text-[var(--ink-dim)]">
                    ({percent(requiredNumber(shares[i][yKey]), total)})
                  </span>
                </span>
              )}
            </li>
          ))}
        </ul>
      )}
      {tooltipData && (
        <TooltipWithBounds
          className="ui-chart-tooltip"
          role="tooltip"
          left={tooltipLeft}
          top={tooltipTop}
        >
          {tooltipData}
        </TooltipWithBounds>
      )}
    </div>
  );
}

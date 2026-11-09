"use client";
import { useId, type ReactNode } from "react";
import { ParentSize } from "@visx/responsive";
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
import { Graph } from "@visx/network";
import { Sankey } from "@visx/sankey";
import { hierarchy, Treemap, Partition, Tree } from "@visx/hierarchy";
import { BoxPlot, ViolinPlot } from "@visx/stats";
import { useTooltip, TooltipWithBounds } from "@visx/tooltip";
import { z } from "zod";
import { resolvedDataProps } from "./catalog";
import { formatted, label, number, rowsOf, type Row } from "./data";
import { Notice } from "./Blocks";

const palette = [
  "var(--space-1)",
  "var(--space-2)",
  "var(--space-3)",
  "var(--ink)",
  "var(--kind-symbol)",
  "var(--kind-person)",
];
const colour = (i: number) => palette[i % palette.length];
export default function Chart({ kind, raw }: { kind: string; raw: unknown }) {
  const parsed = resolvedDataProps.safeParse(raw);
  if (!parsed.success) return <Notice />;
  const p = parsed.data;
  const rows = rowsOf(p.data);
  const numeric =
    Object.keys(rows[0] || {}).find((k) =>
      rows.some((r) => number(r[k]) != null),
    ) || "value";
  const y = p.y || numeric;
  const x = p.x || Object.keys(rows[0] || {}).find((k) => k !== y) || "name";
  if (kind === "SmallMultiples")
    return (
      <section data-ui-component={kind}>
        <h3>{p.title}</h3>
        <div className="ui-grid ui-grid-two">
          {(p.series?.length ? p.series : [y]).map((key) => (
            <Chart
              key={key}
              kind="Line"
              raw={{ ...p, title: key, y: key, series: [key] }}
            />
          ))}
        </div>
        <p>{p.summary}</p>
      </section>
    );
  return (
    <figure
      className="ui-section ui-chart"
      aria-label={p.title}
      data-ui-component={kind}
    >
      <figcaption className="font-medium mb-3">{p.title}</figcaption>
      {rows.length &&
      !rows.some((r) => Object.values(r).some((v) => number(v) != null)) &&
      !["Network", "Tree"].includes(kind) ? (
        <p>No numeric values to show.</p>
      ) : rows.length ? (
        <div style={{ height: kind === "Sparkline" ? 136 : 350 }}>
          <ParentSize debounceTime={30}>
            {({ width }) => (
              <Plot
                kind={kind}
                rows={rows}
                p={p}
                xKey={x}
                yKey={y}
                width={Math.max(240, width)}
              />
            )}
          </ParentSize>
        </div>
      ) : (
        <p>No data to show.</p>
      )}
      <p className="text-xs text-[var(--ink-dim)]">{p.summary}</p>
    </figure>
  );
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
};
type TreeDatum = {
  name: string;
  value: number;
  children?: TreeDatum[];
  row?: Row;
};
function Plot({ kind, rows, p, xKey, yKey, width }: Props) {
  const id = useId();
  const { tooltipData, tooltipLeft, tooltipTop, showTooltip, hideTooltip } =
    useTooltip<string>();
  const height = kind === "Sparkline" ? 100 : 300;
  const margin = {
    left: 62,
    right: kind === "Combo" ? 55 : 20,
    top: 18,
    bottom: 48,
  };
  const w = Math.max(120, width - margin.left - margin.right),
    h = height - margin.top - margin.bottom;
  const series = p.series?.length ? p.series : [yKey];
  const numbers = rows.flatMap((r) =>
    series.map((k) => number(r[k])).filter((v): v is number => v != null),
  );
  const max = Math.max(0, ...numbers),
    min = Math.min(0, ...numbers);
  const stacked = kind === "StackedBar" || kind === "StackedArea";
  const maxStack = Math.max(
    0,
    ...rows.map((r) =>
      series.reduce((a, k) => a + Math.max(0, number(r[k]) || 0), 0),
    ),
  );
  const scaleY = scaleLinear({
    domain: [min, (stacked ? maxStack : max) || 1],
    range: [h, 0],
    nice: true,
  });
  const categories = rows.map(
    (r, i) => formatted(r[xKey], p.xFormat || "text") || String(i + 1),
  );
  const band = scaleBand({ domain: categories, range: [0, w], padding: 0.18 });
  const xNumeric = rows.every((r) => number(r[xKey]) != null);
  const valuesX = rows.map((r) => number(r[xKey]) || 0);
  const scaleX = scaleLinear({
    domain: [Math.min(...valuesX), Math.max(...valuesX) || 1],
    range: [0, w],
    nice: true,
  });
  const x = (r: Row, i: number) =>
    xNumeric
      ? scaleX(number(r[xKey]) || 0)
      : (band(categories[i]) || 0) + band.bandwidth() / 2;
  const y = (r: Row, k = yKey) => scaleY(number(r[k]) || 0);
  const fmt = (v: unknown) => formatted(v, p.formats?.[yKey] || p.format);
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
        onMouseMove={(e) => {
          e.stopPropagation();
          const rect = e.currentTarget.ownerSVGElement?.getBoundingClientRect();
          showTooltip({
            tooltipData: tip(r),
            tooltipLeft: e.clientX - (rect?.left || 0),
            tooltipTop: e.clientY - (rect?.top || 0),
          });
        }}
        onMouseLeave={hideTooltip}
        onFocus={() =>
          showTooltip({
            tooltipData: tip(r),
            tooltipLeft: margin.left,
            tooltipTop: margin.top,
          })
        }
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
        scale={xNumeric ? scaleX : band}
        numTicks={Math.max(2, Math.floor(w / 85))}
        tickFormat={(v) =>
          xNumeric ? formatted(v, "number") : label(v).slice(0, 18)
        }
        {...axisProps}
      />
    </>
  );
  let drawing: ReactNode;
  let showAxes = true;
  if (
    kind === "Line" ||
    kind === "Area" ||
    kind === "Sparkline" ||
    kind === "Combo"
  ) {
    const right = scaleLinear({
      domain: [
        Math.min(0, ...rows.map((r) => number(r[p.size || series[1]]) || 0)),
        Math.max(1, ...rows.map((r) => number(r[p.size || series[1]]) || 0)),
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
                    x={band(categories[i]) || 0}
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
                      ? right(number(r[key]) || 0)
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
                      <circle
                        cx={x(r, i)}
                        cy={
                          kind === "Combo" && s === 1
                            ? right(number(r[key]) || 0)
                            : y(r, key)
                        }
                        r={3}
                        fill={colour(s)}
                      />,
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
            <path
              key={stack.key}
              d={path(stack) || ""}
              fill={colour(i)}
              fillOpacity={0.6}
            />
          ))
        }
      </AreaStack>
    );
  } else if (kind === "StackedBar") {
    const numericRows = rows.map((r, i) =>
      Object.fromEntries([
        [xKey, categories[i]],
        ...series.map((k) => [k, number(r[k]) || 0]),
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
                x={Math.min(horizontal(0), horizontal(number(r[yKey]) || 0))}
                y={vertical(categories[i]) || 0}
                width={Math.abs(
                  horizontal(number(r[yKey]) || 0) - horizontal(0),
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
        start: number(r.start) ?? 0,
        end: number(r.end) ?? 0,
        count: number(r.count) ?? 0,
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
      showAxes = false;
      drawing = (
        <>
          <AxisLeft
            scale={by}
            numTicks={Math.min(4, Math.max(...bins.map((b) => b.count)))}
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
          const start = out.at(-1)?.end || 0;
          return [...out, { start, end: start + (number(r[yKey]) || 0) }];
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
                    (band(categories[i]) || 0) +
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
                    Math.sqrt(Math.abs(number(r[p.size || series[1]]) || 0)),
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
        <Pie
          data={rows.filter((r) => (number(r[yKey]) || 0) > 0)}
          pieValue={(r) => number(r[yKey]) || 0}
          outerRadius={radius}
          innerRadius={kind === "Donut" ? radius * 0.6 : 0}
          padAngle={0.02}
        >
          {(pie) =>
            pie.arcs.map((arc, i) =>
              mark(
                <path d={pie.path(arc) || ""} fill={colour(i)} />,
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
          Math.min(0, ...rows.map((r) => number(r.value) ?? 0)),
        number(box.max) ??
          Math.max(1, ...rows.map((r) => number(r.value) ?? 0)),
      ],
      range: [h, 0],
      nice: true,
    });
    showAxes = false;
    drawing = (
      <>
        <AxisLeft scale={stats} tickFormat={(v) => fmt(v)} {...axisProps} />
        {kind === "BoxPlot" ? (
          mark(
            <BoxPlot
              min={number(box.min) ?? 0}
              max={number(box.max) ?? 0}
              firstQuartile={number(box.q1) ?? 0}
              thirdQuartile={number(box.q3) ?? 0}
              median={number(box.median) ?? 0}
              valueScale={stats}
              left={w / 2 - 25}
              boxWidth={50}
              fill={colour(0)}
              stroke="var(--ink)"
            />,
            box,
            "box",
          )
        ) : (
          <ViolinPlot
            data={rows}
            value={(r) => number(r.value) ?? 0}
            count={(r) => number(r.count) ?? 0}
            valueScale={stats}
            left={w / 2 - 40}
            width={80}
            fill={colour(0)}
            stroke="var(--ink)"
          />
        )}
      </>
    );
  } else if (kind === "Heatmap" || kind === "CalendarHeatmap") {
    showAxes = false;
    const dates = rows.map((r) => new Date(label(r[xKey])));
    const calendar = kind === "CalendarHeatmap";
    const valid = dates.every((d) => Number.isFinite(d.valueOf()));
    const origin = valid ? Math.min(...dates.map((d) => d.valueOf())) : 0;
    const cells = rows.map((r, i) => ({
      row: r,
      x:
        calendar && valid
          ? String(Math.floor((dates[i].valueOf() - origin) / 604800000))
          : categories[i],
      y:
        calendar && valid
          ? ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"][
              dates[i].getUTCDay()
            ]
          : label(r[p.group || "group"]),
    }));
    const xs = [...new Set(cells.map((c) => c.x))],
      ys =
        calendar && valid
          ? ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"]
          : [...new Set(cells.map((c) => c.y))];
    const xb = scaleBand({ domain: xs, range: [0, w], padding: 0.05 }),
      yb = scaleBand({ domain: ys, range: [0, h], padding: 0.05 });
    drawing = (
      <>
        <AxisBottom
          top={h}
          scale={xb}
          numTicks={Math.floor(w / 60)}
          {...axisProps}
        />
        <AxisLeft scale={yb} {...axisProps} />
        {cells.map((cell, i) =>
          mark(
            <Bar
              x={xb(cell.x) || 0}
              y={yb(cell.y) || 0}
              width={xb.bandwidth()}
              height={yb.bandwidth()}
              fill={colour(0)}
              fillOpacity={
                number(cell.row[yKey]) == null
                  ? 0
                  : 0.15 +
                    (0.85 * Math.abs(number(cell.row[yKey]) || 0)) /
                      (Math.max(Math.abs(min), Math.abs(max)) || 1)
              }
            />,
            cell.row,
            i,
          ),
        )}
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
            value: nested.length ? 0 : Math.max(0, number(r[yKey]) || 0),
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
                  <text
                    x={n.x0 + 4}
                    y={n.y0 + 15}
                    style={{ fill: "var(--surface)" }}
                  >
                    {n.x1 - n.x0 > 55 ? n.data.name.slice(0, 18) : ""}
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
        <Partition root={root} size={[w, h]}>
          {(tree) =>
            tree
              .descendants()
              .map((n, i) =>
                mark(
                  <Bar
                    x={n.x0}
                    y={n.y0}
                    width={Math.max(0, n.x1 - n.x0 - 1)}
                    height={Math.max(0, n.y1 - n.y0 - 1)}
                    fill={colour(i)}
                  />,
                  n.data.row || { name: n.data.name },
                  i,
                ),
              )
          }
        </Partition>
      );
    else
      drawing = (
        <Tree root={root} size={[h, w - 50]}>
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
      .filter((r) => number(r[yKey]) != null && (number(r[yKey]) || 0) > 0)
      .map((r) => ({
        source: label(r[xKey]),
        target: label(r[targetKey]),
        value: number(r[yKey]) || 0,
      }));
    const names = [...new Set(flows.flatMap((r) => [r.source, r.target]))];
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
        <Sankey
          root={{ nodes: names.map((name) => ({ name })), links: flows }}
          nodeId={(n) => n.name}
          size={[w, h]}
          nodeWidth={12}
          nodePadding={14}
        >
          {({ graph, createPath }) => (
            <>
              {graph.links.map((link, i) =>
                mark(
                  <path
                    d={createPath(link) || ""}
                    fill="none"
                    stroke={colour(i)}
                    strokeWidth={Math.max(1, link.width || 0)}
                    strokeOpacity={0.4}
                  />,
                  flows[i],
                  i,
                ),
              )}
              {graph.nodes.map((node, i) =>
                mark(
                  <>
                    <Bar
                      x={node.x0 || 0}
                      y={node.y0 || 0}
                      width={(node.x1 || 0) - (node.x0 || 0)}
                      height={(node.y1 || 0) - (node.y0 || 0)}
                      fill={colour(i)}
                    />
                    <text x={(node.x0 || 0) + 16} y={(node.y0 || 0) + 12}>
                      {node.name}
                    </text>
                  </>,
                  { name: node.name },
                  node.name,
                ),
              )}
            </>
          )}
        </Sankey>
      );
  } else if (kind === "Network") {
    showAxes = false;
    const nodes: (Row & { x: number; y: number; datum: Row })[] = rows.map(
      (r, i) => ({
        ...r,
        datum: r,
        x: w / 2 + Math.cos((i / rows.length) * Math.PI * 2) * w * 0.36,
        y: h / 2 + Math.sin((i / rows.length) * Math.PI * 2) * h * 0.36,
      }),
    );
    const links = rows.flatMap((r, i) => {
      const target = nodes.find(
        (n) => n.id === r.target || n.name === r.target,
      );
      return target ? [{ source: nodes[i], target }] : [];
    });
    drawing = (
      <Graph
        graph={{ nodes, links }}
        nodeComponent={({ node }) =>
          mark(
            <>
              <circle cx={node.x} cy={node.y} r={6} fill={colour(0)} />
              <text x={node.x + 8} y={node.y}>
                {label(node.datum[xKey]).slice(0, 12)}
              </text>
            </>,
            node.datum,
            node.x,
          )
        }
        linkComponent={({ link }) => (
          <line
            x1={link.source.x}
            y1={link.source.y}
            x2={link.target.x}
            y2={link.target.y}
            stroke="var(--border-strong)"
          />
        )}
      />
    );
  } else if (kind === "Radar") {
    showAxes = false;
    const radius = Math.min(w, h) / 2;
    const radial = scaleLinear({ domain: [0, max || 1], range: [0, radius] });
    drawing = (
      <Group left={w / 2} top={h / 2}>
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
          <LineRadial
            key={key}
            data={rows}
            angle={(_, i) => (i / rows.length) * Math.PI * 2}
            radius={(r) => radial(number(r[key]) || 0)}
            curve={curveLinearClosed}
            stroke={colour(s)}
            fill={colour(s)}
            fillOpacity={0.12}
          />
        ))}
      </Group>
    );
  } else if (kind === "Funnel") {
    showAxes = false;
    const step = h / rows.length;
    drawing = rows.map((r, i) => {
      const a = (Math.max(0, number(r[yKey]) || 0) / (max || 1)) * w,
        b = (Math.max(0, number(rows[i + 1]?.[yKey]) || 0) / (max || 1)) * w;
      return mark(
        <>
          <path
            d={`M ${(w - a) / 2} ${i * step} L ${(w + a) / 2} ${i * step} L ${(w + b) / 2} ${(i + 1) * step - 3} L ${(w - b) / 2} ${(i + 1) * step - 3} Z`}
            fill={colour(i)}
          />
          <text
            x={w / 2}
            y={i * step + 15}
            textAnchor="middle"
            style={{ fill: "var(--surface)" }}
          >
            {categories[i]} {fmt(r[yKey])}
          </text>
        </>,
        r,
        i,
      );
    });
  } else if (kind === "Gauge" || kind === "Bullet") {
    showAxes = false;
    const r = rows[0],
      n = number(r[yKey]) || 0,
      lo = number(r.min) ?? 0,
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
      drawing =
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
            <text y={-20} textAnchor="middle">
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
            <text x={w / 2} y={h / 2 - 22}>
              {fmt(n)}
            </text>
          </>
        );
    }
  } else drawing = null;
  return (
    <div style={{ width: "100%", height: height + 36, position: "relative" }}>
      <svg
        width={width}
        height={height}
        role="img"
        tabIndex={0}
        onMouseLeave={hideTooltip}
        onMouseMove={(e) => {
          const rect = e.currentTarget.getBoundingClientRect();
          const index = Math.max(
            0,
            Math.min(
              rows.length - 1,
              Math.round(
                ((e.clientX - rect.left - margin.left) / w) * (rows.length - 1),
              ),
            ),
          );
          showTooltip({
            tooltipData: tip(rows[index]),
            tooltipLeft: e.clientX - rect.left,
            tooltipTop: e.clientY - rect.top,
          });
        }}
        onFocus={() =>
          showTooltip({
            tooltipData: tip(rows[0]),
            tooltipLeft: margin.left,
            tooltipTop: margin.top,
          })
        }
        onBlur={hideTooltip}
        aria-labelledby={`${id}-name ${id}-summary`}
        data-chart={kind}
      >
        <title id={`${id}-name`}>{p.title}</title>
        <desc id={`${id}-summary`}>{p.summary}</desc>
        <Group left={margin.left} top={margin.top}>
          {showAxes && axes}
          {drawing}
        </Group>
      </svg>
      {(series.length > 1 || kind === "Pie" || kind === "Donut") && (
        <ul className="ui-legend" aria-label="Legend">
          {(kind === "Pie" || kind === "Donut" ? categories : series).map(
            (name, i) => (
              <li key={i} style={{ color: colour(i) }}>
                {name}
              </li>
            ),
          )}
        </ul>
      )}
      {tooltipData && (
        <TooltipWithBounds
          className="ui-chart-tooltip"
          left={tooltipLeft}
          top={tooltipTop}
        >
          {tooltipData}
        </TooltipWithBounds>
      )}
    </div>
  );
}

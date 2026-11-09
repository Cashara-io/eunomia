"use client";
import dynamic from "next/dynamic";
import { GeneratingPlaceholder } from "../GenerationBox";
import { Component, type ReactNode } from "react";
import {
  JSONUIProvider,
  Renderer,
  type ComponentRenderProps,
} from "@json-render/react";
import { z } from "zod";
import { catalog, chartNames, components } from "./catalog";
import { Copy, Layout, Notice } from "./Blocks";
import DataBlock from "./Blocks";
import "./ui.css";

const Chart = dynamic(() => import("./Charts"), {
  ssr: false,
  loading: () => <GeneratingPlaceholder />,
});
const copyProps = z.object({ text: z.string(), tone: z.string().optional() });
const layoutProps = z.object({
  title: z.string().optional(),
  columns: z.string().optional(),
});
const specSchema = z.strictObject({
  root: z.string(),
  elements: z.record(
    z.string(),
    z.strictObject({
      type: z.string(),
      props: z.record(z.string(), z.unknown()),
      children: z.array(z.string()).optional(),
    }),
  ),
});
const inputSchema = z.object({
  view: z.literal("ui"),
  text: z.string(),
  spec: specSchema,
  data: z.object({ sources: z.record(z.string(), z.unknown()) }),
});
function pointer(
  data: unknown,
  path: string,
): { found: boolean; value: unknown } {
  let value = data;
  for (const token of path
    .slice(1)
    .split("/")
    .map((s) => s.replaceAll("~1", "/").replaceAll("~0", "~"))) {
    if (!value || typeof value !== "object" || !Object.hasOwn(value, token))
      return { found: false, value: undefined };
    value = Reflect.get(value, token);
  }
  return { found: true, value };
}
function checked(view: z.infer<typeof inputSchema>): boolean {
  if (
    JSON.stringify(view.spec).length > 65536 ||
    Object.keys(view.spec.elements).length > 100
  )
    return false;
  const seen = new Set<string>();
  function walk(id: string, depth: number): boolean {
    const e = view.spec.elements[id];
    if (!e || seen.has(id) || depth > 12) return false;
    seen.add(id);
    const def = Object.entries(components).find(
      ([name]) => name === e.type,
    )?.[1];
    if (!def || !def.props.safeParse(e.props).success) return false;
    const binding = z.object({ $state: z.string() }).safeParse(e.props.data);
    if (binding.success && !pointer(view.data, binding.data.$state).found)
      return false;
    return (e.children || []).every((c) => walk(c, depth + 1));
  }
  return (
    walk(view.spec.root, 1) &&
    seen.size === Object.keys(view.spec.elements).length
  );
}
const layouts = new Set([
  "Stack",
  "Grid",
  "Card",
  "Section",
  "Tabs",
  "Accordion",
]);
const copies = new Set(["Heading", "Text", "Caption", "Badge", "Callout"]);
const charts = new Set<string>(chartNames);
const registry = Object.fromEntries(
  Object.keys(catalog.data.components).map((kind) => [
    kind,
    function CatalogComponent({ element, children }: ComponentRenderProps) {
      if (kind === "Divider") return <hr className="border-[var(--border)]" />;
      if (layouts.has(kind)) {
        const props = layoutProps.safeParse(element.props);
        return props.success ? (
          <Layout kind={kind} {...props.data}>
            {children}
          </Layout>
        ) : (
          <Notice />
        );
      }
      if (copies.has(kind)) {
        const props = copyProps.safeParse(element.props);
        return props.success ? (
          <Copy kind={kind} {...props.data} />
        ) : (
          <Notice />
        );
      }
      return charts.has(kind) ? (
        <Chart kind={kind} raw={element.props} />
      ) : (
        <DataBlock kind={kind} raw={element.props} />
      );
    },
  ]),
);
class Boundary extends Component<
  { text: string; children: ReactNode },
  { failed: boolean }
> {
  state = { failed: false };
  static getDerivedStateFromError() {
    return { failed: true };
  }
  render() {
    return this.state.failed ? (
      <Notice text={this.props.text} />
    ) : (
      this.props.children
    );
  }
}
export default function UiView({ view: raw }: { view: unknown }) {
  const parsed = inputSchema.safeParse(raw);
  const fallback = z.object({ text: z.string() }).safeParse(raw);
  if (!parsed.success || !checked(parsed.data))
    return <Notice text={fallback.success ? fallback.data.text : undefined} />;
  const view = parsed.data;
  const spec = structuredClone(view.spec);
  for (const e of Object.values(spec.elements)) {
    const binding = z.object({ $state: z.string() }).safeParse(e.props.data);
    if (!binding.success) continue;
    const segments = binding.data.$state.split("/");
    const source = z
      .object({
        columns: z
          .array(z.object({ key: z.string(), datatype: z.string(), unit: z.string().nullable().optional() }))
          .optional(),
        metric: z.object({ unit: z.string().nullable() }).optional(),
      })
      .safeParse(view.data.sources[segments[2]]);
    if (!source.success) continue;
    const formats: Record<string, string> = Object.fromEntries(
      (source.data.columns || []).map((c) => [
        c.key,
        c.datatype === "money"
          ? "money"
          : c.datatype === "percent"
            ? "percent"
            : ["date", "datetime"].includes(c.datatype)
              ? "date"
              : !["number", "integer"].includes(c.datatype)
                ? "text"
                : "number",
      ]),
    );
    e.props.formats = formats;
    e.props.units = Object.fromEntries((source.data.columns || []).map(c => [c.key, c.unit ?? null]));
    if (typeof e.props.x === "string") e.props.xFormat = formats[e.props.x] ?? "text";
    const key = typeof e.props.y === "string"
      ? e.props.y
      : segments[3] === "rows" && segments[5]
        ? segments[5]
        : segments[3] === "totals"
          ? segments[4]
          : undefined;
    const column = source.data.columns?.find(
      (c) => c.key === (segments[3] === "distributions" ? segments[4] : key),
    );
    const count = segments[3] === "total_rows" || (segments[3] === "totals" && ["count", "distinct"].includes(segments[5])) || (segments[3] === "metric" && (segments[4] === "n_entities" || (segments[4] === "series" && segments[6] === "n")));
    const datatype = count ? "number" : column?.datatype;
    e.props.unit = count ? null : column?.unit ?? source.data.metric?.unit ?? null;
    e.props.format = count ? "number" : column ? formats[column.key] : "number";
    if (!count && (datatype === "money" || /^[A-Z]{3}$/.test(source.data.metric?.unit || "")))
      e.props.format = "money";
    else if (!count && (datatype === "percent" || source.data.metric?.unit === "%"))
      e.props.format = "percent";
  }
  return (
    <Boundary text={view.text}>
      <div className="generative-ui">
        <JSONUIProvider registry={registry} initialState={view.data}>
          <Renderer
            spec={spec}
            registry={registry}
            fallback={() => <Notice text={view.text} />}
          />
        </JSONUIProvider>
        {Object.entries(view.data.sources).map(([name, raw]) => {
          const source = z
            .object({
              status: z.string().optional(),
              truncated: z.boolean().optional(),
              warnings: z.array(z.string()).optional(),
            })
            .safeParse(raw);
          return source.success ? (
            <aside key={name} className="text-xs text-[var(--ink-dim)]">
              {source.data.status && source.data.status !== "approved" && (
                <p>Not approved</p>
              )}
              {source.data.truncated && <p>Data was truncated.</p>}
              {source.data.warnings?.map((w, i) => (
                <p key={i}>{w}</p>
              ))}
            </aside>
          ) : null;
        })}
        <details>
          <summary className="text-xs cursor-pointer">Source answer</summary>
          <pre className="whitespace-pre-wrap text-xs">{view.text}</pre>
        </details>
      </div>
    </Boundary>
  );
}

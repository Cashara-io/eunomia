import { defineCatalog, defineSchema } from "@json-render/core";
import { schema } from "@json-render/react/schema";
import { z } from "zod";

const words = z.string().max(1000);
const binding = z.strictObject({ $state: z.string().regex(/^\/sources\//) });
const format = z.enum(["number", "money", "percent", "date", "text"]);
const column = z.strictObject({
  key: words,
  label: words,
  format: format.optional(),
  total: z.enum(["sum", "avg", "count"]).optional(),
});
const container = {
  props: z.strictObject({ title: words.optional() }),
  slots: ["default"],
  description: "Groups its children. Put numbers in titles through bound placeholders, for example {{/sources/sales/total_rows}}.",
};
const copy = {
  props: z.strictObject({
    text: words,
    tone: z.enum(["info", "warning", "success"]).optional(),
  }),
  description: "Model prose. Use {{/sources/sales/total_rows}} for numbers. The server substitutes one exact typed numeric or date binding. Raw digits need verbatim server text tokens or matching applied ISO dates or bucket labels. Number words are unchecked.",
};
const dataProps = z.strictObject({
  title: words,
  data: binding,
  x: words.optional(),
  y: words.optional(),
  series: z.array(words).max(12).optional(),
  format: format.optional(),
  formats: z.record(z.string(), format).optional(),
  xFormat: format.optional(),
  columns: z.array(column).max(24).optional(),
  summary: words,
  group: words.optional(),
  size: words.optional(),
  parent: words.optional(),
  orientation: z.enum(["vertical", "horizontal"]).optional(),
});
export const chartNames = [
  "Line",
  "Area",
  "StackedArea",
  "Bar",
  "GroupedBar",
  "StackedBar",
  "Histogram",
  "Scatter",
  "Bubble",
  "Pie",
  "Donut",
  "BoxPlot",
  "Violin",
  "CalendarHeatmap",
  "Heatmap",
  "Treemap",
  "Partition",
  "Radar",
  "Funnel",
  "Waterfall",
  "Sparkline",
  "Gauge",
  "Bullet",
  "Network",
  "Tree",
  "Combo",
  "SmallMultiples",
  "Sankey",
] as const;
const chartDescription =
  "Sankey binds rows with a source column (x), target column (group, default target), and numeric value column (y). Cyclic flows should use Network. Histogram binds /sources/name/distributions/property/bins (start,end,count); BoxPlot binds /sources/name/distributions/property/box (min,q1,median,q3,max); Violin binds /sources/name/distributions/property/density (value,count). Table with totals binds the whole dataset. Other charts bind rows. Server-bound rows only. x is a column key, y is a numeric column key, series lists numeric columns. Use line for trends, donut or treemap for shares, histogram or box plot for distributions, scatter for relationships, Sankey or Funnel for flow, waterfall for change. Summary describes the question, not invented numbers.";
const charts = Object.fromEntries(
  chartNames.map((name) => [
    name,
    { props: dataProps, description: chartDescription },
  ]),
);
export const components = {
  Stack: container,
  Grid: {
    ...container,
    props: z.strictObject({
      title: words.optional(),
      columns: z.enum(["one", "two", "three"]).optional(),
    }),
  },
  Card: container,
  Section: container,
  Tabs: container,
  Accordion: container,
  Divider: { props: z.strictObject({}), description: "Separates sections" },
  Heading: copy,
  Text: copy,
  Callout: copy,
  Badge: copy,
  Caption: copy,
  ...Object.fromEntries(
    [
      "Stat",
      "StatGroup",
      "Table",
      "KeyValue",
      "Entity",
      "EntityList",
      "Meter",
      "Timeline",
      "Comparison",
      "Impact",
      "Subgraph",
      "Metric",
      "Scenario",
    ].map((name) => [
      name,
      {
        props: dataProps,
        description:
          "Displays bound server data. Legacy blocks bind a resolved source of the same type. All numeric values must come from data, including deltas, totals and bounds.",
      },
    ]),
  ),
  ...charts,
};
const safeSchema = defineSchema(() => schema.definition, {
  promptTemplate: ({ formatZodType }) =>
    [
      'Use show_view with view "ui", spec {root,elements} and named sources. root is a string key. elements MUST be an OBJECT keyed by element IDs, never an array or nested tree. Example: {"root":"chart","elements":{"chart":{"type":"Line","props":{"title":"Trend","summary":"Trend","data":{"$state":"/sources/sales/rows"},"x":"date","y":"amount"},"children":[]}}}. Each element has type, props and children. Use an empty children array for leaves. All elements must form one tree.',
      'Never provide initial state, actions, repeats, conditions, sample data or literal numeric values. Data props must be {$state:"/sources/name/path"}. The server resolves these paths. The model writes words and arranges components.',
      ...Object.entries(components).map(
        ([name, def]) =>
          `${name}: ${def.description} Props: ${formatZodType(def.props)}`,
      ),
    ].join("\n"),
});
export const catalog = defineCatalog(safeSchema, { components, actions: {} });
export type DataProps = z.infer<typeof dataProps>;
export const resolvedDataProps = dataProps.extend({ data: z.unknown(), unit: z.string().nullable().optional(), units: z.record(z.string(), z.string().nullable()).optional() });

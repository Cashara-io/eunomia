import { expect, test } from "bun:test";
import { parseFragment } from "parse5";
import { renderToStaticMarkup } from "react-dom/server";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import ChatText from "../src/components/chat/ChatText";
import MetricView from "../src/components/chat/views/MetricView";
import UiView from "../src/components/chat/ui/UiView";
import Chart from "../src/components/chat/ui/Charts";
import DataBlock from "../src/components/chat/ui/Blocks";
import { chartNames } from "../src/components/chat/ui/catalog";
import { drawable, proportionalTotal, formatted, number, requiredNumber } from "../src/components/chat/ui/data";

const ui = new URL("../src/components/chat/ui", import.meta.url).pathname;
test("UI numeric access never defaults a missing measurement to zero", () => {
  for (const name of readdirSync(ui).filter(n => /\.tsx?$/.test(n))) {
    expect(readFileSync(join(ui, name), "utf8")).not.toMatch(/(?:\?\?|\|\|)\s*0\b/);
  }
  for (const value of [null, undefined, NaN, Infinity, -Infinity, "12", "", false, {}]) {
    expect(number(value)).toBeNull();
    expect(() => requiredNumber(value)).toThrow("Missing numeric measurement");
  }
  expect(number(0)).toBe(0);
  expect(requiredNumber(0)).toBe(0);
  expect(formatted(NaN, "number")).toBe("none");
});
test("every chart checks y even when series is supplied", () => {
  for (const kind of chartNames) {
    const row = { name: "Supplier", value: null, other: 12, min: null, max: 10, start: null, end: 10, count: null, q1: null, median: null, q3: null };
    const html = renderToStaticMarkup(<Chart kind={kind} raw={{title: "Values", summary: "Values", x: "name", y: "value", series: ["other"], data: [row]}} />);
    expect(html).toContain("No numeric values to show.");
    expect(html).not.toContain("<svg");
  }
});
test("meter requires its minimum instead of inventing zero", () => {
  const html = renderToStaticMarkup(<DataBlock kind="Meter" raw={{title: "Range", summary: "Range", data: {value: 5, min: null, max: 10}}} />);
  expect(html).toContain("No range available.");
  expect(html).not.toContain("<meter");
});
test("unsafe finite chart values fall back before scale arithmetic", () => {
  const html = renderToStaticMarkup(<Chart kind="Line" raw={{title: "Values", summary: "Values", x: "name", y: "value", series: ["value"], data: [{name: "Low", value: -1e308}, {name: "High", value: 1e308}]}} />);
  expect(html).toContain("This display could not be drawn.");
  expect(html).not.toContain("<svg");
  expect(html).not.toMatch(/NaN|Infinity/);
});
test("explicit table columns inherit server formats for cells and totals", () => {
  const html = renderToStaticMarkup(<DataBlock kind="Table" raw={{title: "Rows", summary: "Rows", columns: [{key: "amount", label: "Amount", total: "sum"}], formats: {amount: "money"}, units: {amount: "USD"}, data: {rows: [{amount: 120.25}], columns: [{key: "amount", datatype: "money", unit: "USD"}], totals: {amount: {sum: 120.25}}}}} />);
  expect(html.match(/USD 120.25/g)).toHaveLength(2);
});
test("numeric formats treat strings as missing", () => {
  for (const format of ["number", "money", "percent"]) {
    expect(formatted("12", format)).toBe("none");
    expect(formatted("missing", format)).toBe("none");
  }
});

test("server types override model formats and counts stay unitless", () => {
  const source = {rows: [{quantity: 12, amount: 120.25}], columns: [{key: "amount", datatype: "money", unit: "USD"}, {key: "quantity", datatype: "number"}], total_rows: 1, totals: {amount: {count: 1}}};
  const stat = (path, format) => renderToStaticMarkup(<UiView view={{view: "ui", text: "Values", spec: {root: "a", elements: {a: {type: "Stat", props: {title: "Value", summary: "Value", data: {$state: path}, format}}}}, data: {sources: {sample: source}}}} />);
  expect(stat("/sources/sample/rows/0/quantity", "money")).not.toContain("A$");
  expect(stat("/sources/sample/total_rows", "money")).not.toContain("A$");
  expect(stat("/sources/sample/total_rows", "money")).not.toContain("USD");
  expect(stat("/sources/sample/totals/amount/count", "money")).not.toContain("USD");
  expect(stat("/sources/sample/rows/0/amount", "number")).toContain("USD 120.25");
});

test("stat groups retain a missing typed measurement", () => {
  for (const value of [null, NaN, Infinity, "12"]) {
    const html = renderToStaticMarkup(<DataBlock kind="StatGroup" raw={{title: "Values", summary: "Values", data: {rows: [{name: "Supplier", amount: value}], columns: [{key: "amount", datatype: "money", unit: "USD"}]}}} />);
    expect(html).toContain("ui-stat-tile");
    expect(html).toContain("none");
    expect(html).not.toContain("A$0");
  }
});

test("omitted y never substitutes an available measurement", () => {
  const raw = {title: "Amount", summary: "Amount", x: "name", data: [{name: "Supplier", amount: null, quantity: 12}]};
  const unresolved = renderToStaticMarkup(<Chart kind="Line" raw={raw} />);
  expect(unresolved).toContain("This display could not be drawn.");
  expect(unresolved).not.toContain("<svg");
  const resolved = renderToStaticMarkup(<Chart kind="Line" raw={{...raw, y: "amount", series: ["amount"]}} />);
  expect(resolved).toContain("No numeric values to show.");
  expect(resolved).not.toContain("<svg");
});
test("proportional charts reject subnormal and zero totals", () => {
  for (const kind of ["Pie", "Donut", "Treemap", "Partition", "Funnel", "StackedBar", "StackedArea"]) {
    for (const value of [1e-320, 5e-324, 0]) {
      const html = renderToStaticMarkup(<Chart kind={kind} raw={{title: "Values", summary: "Values", x: "name", y: "value", series: ["value"], parent: "parent", data: [{name: "Supplier", value, parent: null}]}} />);
      expect(html).toContain("This display could not be drawn.");
      expect(html).not.toContain("<svg");
      expect(html).not.toMatch(/NaN|Infinity/);
    }
  }
});

test("shared proportional accessor accepts safe totals only", () => {
  for (const values of [[0], [1e-320], [5e-324], [null], [Infinity]]) expect(proportionalTotal(values)).toBeNull();
  expect(drawable(1e-320)).toBe(false);
  expect(proportionalTotal([10, 20])).toBe(30);
  for (const kind of ["Pie", "Donut", "Treemap", "Partition", "Funnel", "StackedBar", "StackedArea"]) {
    const html = renderToStaticMarkup(<Chart kind={kind} raw={{title: "Values", summary: "Values", x: "name", y: "value", series: ["value"], parent: "parent", data: [{name: "Supplier", value: 12, parent: null}]}} />);
    expect(html).toContain("<svg");
    expect(html).not.toMatch(/NaN|Infinity/);
  }
});

 test("server substituted prose displays literally without interpreting placeholders", () => {
  const view = {view: "ui", text: "Total 1588.00 AUD", data: {sources: {}}, spec: {root: "copy", elements: {copy: {type: "Text", props: {text: "Total 1588.00 AUD <script> {{sources.sales.amount}}"}}}}};
  const html = renderToStaticMarkup(<UiView view={view} />);
  expect(html).toContain("Total 1588.00 AUD &lt;script&gt; {{sources.sales.amount}}");
  expect(html).not.toContain("<script>");
});

test("UI prose never reparses Markdown entities or escapes", () => {
  for (const type of ["Text", "Heading", "Caption", "Badge", "Callout"]) {
    const text = 'Revenue rose &half;% Count &#49; **bold** \\u0031 \\*one*';
    const view = {view: "ui", text, data: {sources: {}}, spec: {root: "a", elements: {a: {type, props: {text}}}}};
    const html = renderToStaticMarkup(<UiView view={view} />);
    expect(html).toContain('Revenue rose &amp;half;% Count &amp;#49; **bold** \\u0031 \\*one*');
    expect(html).not.toContain("½");
    expect(html).not.toContain("<strong>bold</strong>");
  }
});


test("grouped metric view displays server groups and keeps a missing value missing", () => {
  const html = renderToStaticMarkup(<MetricView view={{view: "metric", text: "Sales by group", entities: [], metric: {
    metric: "sales", kind: "sale", vault: "vault:test", description: null, expr_words: "Sum of sales", value: null,
    unit: "AUD", as_of: "2026-11-10T00:00:00Z", n_entities: 3, entity_ids: [], truncated: false, status: "approved", warnings: [],
    groups: [
      {group: {entity_id: "entity:store", name: "Store"}, value: 40.25, n_entities: 2, entity_ids: [], truncated: false},
      {group: null, value: null, n_entities: 1, entity_ids: [], truncated: false},
    ],
  }}} />);
  expect(html).toContain("Store");
  expect(html).toContain("40.25");
  expect(html).toContain("Unassigned");
  expect(html).toContain("none");
  expect(html).not.toContain("No value");
  expect(html).not.toContain("0.00");
});

function visibleUnits(html) {
  const root = parseFragment(html);
  const tags = new Set(["p", "h1", "h2", "h3", "h4", "h5", "h6", "li", "td", "th", "pre"]);
  const text = node => node.nodeName === "#text" ? node.value : node.tagName === "br" ? " " : (node.childNodes || []).filter(child => !["ul", "ol", "p", "pre", "h1", "h2", "h3", "h4", "h5", "h6"].includes(child.tagName)).map(text).join("");
  const units = [];
  function walk(node) {
    if (tags.has(node.tagName)) {
      const value = text(node).replace(/\s+/g, " ").trim();
      if (value) units.push(value);
    }
    for (const child of node.childNodes || []) walk(child);
  }
  walk(root);
  return units;
}
test("chat and the server render identical visible Markdown units", () => {
  const fixture = JSON.parse(readFileSync(new URL("../../backend/tests/fixtures/chat_markdown.json", import.meta.url), "utf8"));
  const original = renderToStaticMarkup(<ChatText text={fixture.reply} />);
  expect(visibleUnits(original)).toEqual(visibleUnits(fixture.raw_server_html));
  const html = renderToStaticMarkup(<ChatText text={fixture.checked} />);
  expect(visibleUnits(html)).toEqual(visibleUnits(fixture.server_html));
  expect(visibleUnits(html)).toEqual(fixture.units);
  expect(html).toContain("<strong>");
  expect(html).toContain("<table>");
  expect(html).toContain("<li>");
  expect(html).toContain('href="https://example.com/report"');
  expect(html.match(/unverified number/g)).toHaveLength(1);
  expect(html).not.toContain("999");
});
test("chat skips raw HTML and joins inline children without joining cells", () => {
  const html = renderToStaticMarkup(<ChatText text={"Count 1**2** &amp; &#49;. <b>Visible</b>\n\n<div>hidden 999</div>\n\n| Left | Right |\n| --- | --- |\n| 12 | 34 |"} />);
  expect(visibleUnits(html)).toEqual(["Count 12 & 1. Visible", "Left", "Right", "12", "34"]);
  expect(html).not.toContain("<b>");
  expect(html).not.toContain("999");
});
test("overlap notes are visible in metric and generated views", () => {
  const note = "1 entity contributes to more than one group; groups overlap and must not be summed";
  const metric = {as_of: "2026-01-01T00:00:00Z", expr_words: "Sum of sales", metric: "sales", kind: "sample", value: null, unit: "AUD", groups: [{group: "Supplier", value: 100, n_entities: 1, entity_ids: [], truncated: false}], warnings: [note], status: "approved", entity_ids: [], n_entities: 1, truncated: false};
  expect(renderToStaticMarkup(<MetricView view={{type: "metric", headline: "Sales", metric, entities: []}} />)).toContain(note);
  const view = {type: "ui", spec: {root: "h", elements: {h: {type: "Heading", props: {text: "Sales"}, children: []}}}, data: {sources: {sales: {warnings: [note]}}}, text: note};
  expect(renderToStaticMarkup(<UiView view={view} />)).toContain(note);
});

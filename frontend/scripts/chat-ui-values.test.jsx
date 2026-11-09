import { expect, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import UiView from "../src/components/chat/ui/UiView";
import Chart from "../src/components/chat/ui/Charts";
import DataBlock from "../src/components/chat/ui/Blocks";
import { chartNames } from "../src/components/chat/ui/catalog";
import { formatted, number, requiredNumber } from "../src/components/chat/ui/data";

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
  const html = renderToStaticMarkup(<Chart kind="Line" raw={{title: "Values", summary: "Values", x: "name", y: "value", data: [{name: "Low", value: -1e308}, {name: "High", value: 1e308}]}} />);
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

import { test, expect } from "bun:test";
import { receiptState, sourceFor, sourceAvailable, safeReturn } from "../src/components/twin/receipt-model";
import { receiptFixture } from "../tests/fixtures/receipts";
test("each value state has its own words, including recorded zero", () => {
  const data = receiptFixture();
  expect(new Set(data.properties.map(receiptState)).size).toBe(6);
});
test("duplicate names join by identity and unavailable content fails closed", () => {
  const data = receiptFixture();
  expect(sourceFor(data, data.properties.find(p => p.name === "unit_cost"))?.document?.row.line).toBe(42);
  expect(sourceFor(data, data.properties.find(p => p.name === "discount"))?.document?.row.line).toBe(9);
  expect(sourceAvailable(data.sources.find(s => s.id === "denied"))).toBe(false);
  expect(sourceAvailable({ state: "unknown" })).toBe(false);
});
test("return context cannot escape the active space or ontology", () => {
  expect(safeReturn("//elsewhere.test/ontology/objects?space=business", "business")).toBeNull();
  expect(safeReturn("/ontology/objects?space=foreign", "business")).toBeNull();
  expect(safeReturn("/ontology/objects?space=business&search=House", "business")).toContain("search=House");
});

// Row labels and comparison messages remain independent of display rounding.
test("current row links disclose that the row was matched by name", async () => {
  const { sourceLabel } = await import("../src/components/twin/receipt-model");
  expect(sourceLabel(receiptFixture().sources[0])).toBe("products.csv, row 42 (matched by name)");
});
test("current cell differences use shared typed formatting and stale wording", async () => {
  const { sourceMismatch } = await import("../src/components/twin/receipt-model");
  const data = receiptFixture(), property = data.properties.find(p => p.name === "unit_cost"), source = data.sources[0];
  property.state = "recorded";
  source.document.row.cells.unit_cost = "99.00";
  expect(sourceMismatch(property, source)).toBe("The file now says AUD 99.00. The recorded value is AUD 12.00. The file may have changed since this value was recorded.");
  property.state = "stale";
  expect(sourceMismatch(property, source)).toContain("The source changed after this value was recorded.");
  source.document.row.cells.unit_cost = "12.000";
  expect(sourceMismatch(property, source)).toBeNull();
  property.datatype = "percent"; property.unit = "%"; property.value = 0.125;
  source.document.row.cells.unit_cost = "0.1250";
  expect(sourceMismatch(property, source)).toBeNull();
  source.document.row.cells.unit_cost = "0.99";
  expect(sourceMismatch(property, source)).toContain("99.00%");
  property.datatype = "bool"; property.value = false;
  source.document.row.cells.unit_cost = "false";
  expect(sourceMismatch(property, source)).toBeNull();
});

const comparisons = [
  ["grouped number", "number", null, 1200, "1,200", null],
  ["below display precision", "money", "AUD", 12.001, "12.002", "The file now says"],
  ["exact decimal precision", "number", null, "9007199254740993.001", "9007199254740993.002", "The file now says"],
  ["currency code", "money", "AUD", 1200, "AUD 1,200.00", null],
  ["recorded currency code", "money", "AUD", "AUD 1,200", "1200.00", null],
  ["currency symbol", "money", "AUD", 1200, "$1,200.00", null],
  ["explicit Australian symbol", "money", "AUD", 1200, "A$1,200.00", null],
  ["negative currency", "money", "AUD", -1200, "-$1,200.00", null],
  ["currency suffix", "money", "AUD", 1200, "1,200.00 aud", null],
  ["different currency", "money", "AUD", 1200, "USD 1,200", "Can't compare"],
  ["empty current cell", "money", "AUD", 12, "  ", "Can't compare"],
  ["case and spaces", "text", null, " Hello ", "hello", null],
  ["Unicode text", "text", null, "CAFÉ", " cafe\u0301 ", null],
  ["different text", "text", null, "Hello", "Goodbye", "The file now says"],
  ["boolean yes", "boolean", null, true, " YES ", null],
  ["boolean one", "bool", null, "true", "1", null],
  ["boolean no", "boolean", null, false, "0", null],
  ["boolean changed", "boolean", null, true, "no", "The file now says"],
  ["boolean unparseable", "boolean", null, true, "maybe", "Can't compare"],
  ["ISO date", "date", null, "2026-11-13", "2026-11-13", null],
  ["unambiguous day first", "date", null, "2026-11-13", "13/11/2026", null],
  ["different date", "date", null, "2026-11-13", "14/11/2026", "The file now says"],
  ["ambiguous day first", "date", null, "2026-04-03", "03/04/2026", "Can't compare"],
  ["impossible date", "date", null, "2026-02-28", "2026-02-30", "Can't compare"],
  ["timestamp offset", "datetime", null, "2026-11-13T00:00:00Z", "2026-11-13T11:00:00+11:00", null],
  ["timestamp fractional precision", "datetime", null, "2026-11-13T00:00:00.0001Z", "2026-11-13T00:00:00.0002Z", "The file now says"],
  ["timestamp missing zone", "datetime", null, "2026-11-13T00:00:00Z", "2026-11-13T00:00:00", "Can't compare"],
  ["leap day", "date", null, "2024-02-29", "29/02/2024", null],
  ["unparseable", "number", null, 12, "twelve", "Can't compare"],
  ["invalid grouping", "number", null, 12, "1,2", "Can't compare"],
  ["unreconciled recorded type", "number", null, true, "1", "Can't compare"],
  ["percent notation", "percent", "%", 0.125, "12.50%", null],
  ["scientific notation", "number", null, "0.000000001200", "1.2e-9", null],
];
test.each(comparisons)("typed source comparison: %s", async (_name, datatype, unit, value, current, expected) => {
  const { sourceMismatch } = await import("../src/components/twin/receipt-model");
  const data = receiptFixture();
  const property = {...data.properties[0], name:"unit_cost", state:"recorded", datatype, unit, value};
  data.sources[0].document.row.cells.unit_cost = current;
  const message = sourceMismatch(property, data.sources[0]);
  if (expected === null) expect(message).toBeNull();
  else expect(message).toContain(expected);
  if (_name === "below display precision") {
    expect(message).toContain("12.002");
    expect(message).toContain("12.001");
  }
});

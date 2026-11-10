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

// Compare what readers see, including the property's unit and display precision.
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

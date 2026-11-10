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

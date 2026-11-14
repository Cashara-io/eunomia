import { test, expect } from "bun:test";
import fixture from "../tests/fixtures/ontology-ui.json";

test("fixture retains the paper's actual totals without creating DOM rows", () => {
  expect(fixture.ontology.kinds.reduce((n, k) => n + k.entities, 0)).toBe(
    18000,
  );
});

import {
  humanLabel,
  plural,
  relativeTime,
  activityDay,
} from "../src/components/twin/overview-format";
test("display formatting keeps identifiers separate, uses human plurals and relative ages", () => {
  expect(humanLabel("unit_cost")).toBe("Unit cost");
  expect(humanLabel("stockItem")).toBe("Stock Item");
  expect(humanLabel("identifier", "API display name")).toBe("API display name");
  expect(plural("Stock item")).toBe("Stock items");
  expect(plural("source", 1)).toBe("source");
  expect(plural("person", 2)).toBe("people");
  const now = new Date("2026-11-10T03:34:00Z").getTime();
  expect(relativeTime("2026-11-10T03:32:00Z", now)).toBe("2 min ago");
  expect(activityDay("2026-11-09T03:32:00Z", now)).toBe("Yesterday");
});

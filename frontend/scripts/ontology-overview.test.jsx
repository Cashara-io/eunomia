import { test, expect } from "bun:test";
import { businessSummary } from "../src/components/twin/overview-model";
import fixture from "../tests/fixtures/ontology-ui.json";

// The generated ViewStatus union is narrower than JSON inference.
const approved = (kind, entities) => ({
  kind,
  entities,
  built_in: false,
  status: "approved",
  revision: 1,
  samples: [],
  synonyms: [],
  properties: [],
  metrics: [],
});
test("summary chooses the largest kinds, strongest actual links and deterministic ties", () => {
  const data = {
    vault: "v",
    strict: true,
    kinds: [
      approved("d", 4),
      approved("b", 10),
      approved("a", 10),
      approved("c", 5),
      approved("hidden", 1),
    ],
    links: [
      {
        from_kind: "a",
        label: "supplies",
        to_kind: "b",
        count: 12,
        revision: 1,
        samples: [],
        status: "approved",
      },
      {
        from_kind: "hidden",
        label: "supplies",
        to_kind: "b",
        count: 99,
        revision: 1,
        samples: [],
        status: "approved",
      },
      {
        from_kind: "c",
        label: "contains",
        to_kind: "d",
        count: 24,
        revision: 1,
        samples: [],
        status: "approved",
      },
      {
        from_kind: "a",
        label: "removed",
        to_kind: "b",
        count: 90,
        revision: 1,
        samples: [],
        status: "rejected",
      },
    ],
  };
  const summary = businessSummary(data);
  expect(summary.top.map((k) => k.kind)).toEqual(["a", "b", "c", "d"]);
  expect(summary.links.map((l) => l.label)).toEqual(["contains", "supplies"]);
  expect(summary.total).toBe(30);
  expect(
    businessSummary({
      ...data,
      kinds: [...data.kinds].reverse(),
      links: [...data.links].reverse(),
    }).links,
  ).toEqual(summary.links);
});
test("fixture retains the paper's actual totals without creating DOM rows", () => {
  expect(fixture.ontology.kinds.reduce((n, k) => n + k.entities, 0)).toBe(
    18000,
  );
});

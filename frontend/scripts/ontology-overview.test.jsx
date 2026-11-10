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

import {
  overviewLayout,
  routeLabels,
} from "../src/components/twin/overview-layout";
import {
  humanLabel,
  plural,
  relativeTime,
  activityDay,
} from "../src/components/twin/overview-format";
test("landscape has one deterministic form per kind, proportionate heights, and every actual route", () => {
  const kinds = fixture.ontology.kinds,
    links = fixture.ontology.links;
  const layout = overviewLayout(kinds, links);
  expect(new Set(layout.nodes.map((n) => n.kind.kind)).size).toBe(16);
  expect(layout.edges).toHaveLength(48);
  expect(overviewLayout([...kinds].reverse(), links).nodes).toEqual(
    layout.nodes,
  );
  expect(
    layout.nodes.find((n) => n.kind.kind === "stock_item").height,
  ).toBeGreaterThan(
    layout.nodes.find((n) => n.kind.kind === "location").height,
  );
  expect(layout.unlinked.map((k) => k.kind)).toEqual([
    "delivery_route",
    "warehouse",
  ]);
  const labels = routeLabels(layout, layout.edges.slice(0, 3));
  expect(
    labels.every(
      (label) => Number.isFinite(label.x) && Number.isFinite(label.y),
    ),
  ).toBe(true);
});
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

test("height has a generous range, a fixed footprint and a visibly different rank", () => {
  const { nodes } = overviewLayout(
    fixture.ontology.kinds,
    fixture.ontology.links,
  );
  const stock = nodes.find((n) => n.kind.kind === "stock_item");
  const contract = nodes.find((n) => n.kind.kind === "contract");
  expect(stock.height / contract.height).toBeGreaterThan(4);
  expect(stock.height - contract.height).toBeGreaterThan(90);
  expect(nodes.find((n) => n.kind.kind === "warehouse").height).toBe(4);
});

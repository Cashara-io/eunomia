import { test, expect } from "bun:test";
import fixture from "../tests/fixtures/ontology-ui.json";
import { richNeighbourhood } from "../tests/fixtures/connections";
import { filterScene } from "../src/components/twin/connections-model";
import {
  LANE_OPEN_LIMIT,
  MAX_STAGE_KINDS,
  buildLanes,
  kindPairs,
  kindRanks,
  kindRows,
  laneOfNode,
  placeLanes,
  stageKinds,
} from "../src/components/twin/strata/strata-model";

const { kinds, links } = fixture.ontology;
const names = kinds.map((k) => k.kind);
const link = (from_kind, label, to_kind, count = 1, status = "approved") => ({ from_kind, label, to_kind, count, status });

test("pairs carry every recorded link type once and orient by the heavier direction", () => {
  const pairs = kindPairs(links, new Set(names));
  expect(pairs.flatMap((p) => p.links)).toHaveLength(links.filter((l) => l.status !== "rejected").length);
  const reversed = kindPairs([...links].reverse(), new Set(names));
  expect(reversed).toEqual(pairs);
  const supplier = pairs.find((p) => [p.from, p.to].sort().join() === "product,supplier");
  expect(supplier.count).toBe(120 + 137);
  expect(supplier.from).toBe("product");
  expect(supplier.reverse).toBe(120);
});

test("columns follow link direction, not kind names, and survive cycles", () => {
  const chain = [link("a", "feeds", "b", 5), link("b", "feeds", "c", 5), link("c", "feeds", "a", 1)];
  const pairs = kindPairs(chain, new Set(["a", "b", "c"]));
  const ranks = kindRanks(["a", "b", "c"], pairs);
  expect(ranks.get("a")).toBeLessThan(ranks.get("b"));
  expect(ranks.get("b")).toBeLessThan(ranks.get("c"));
  const renamed = chain.map((l) => ({ ...l, from_kind: l.from_kind.toUpperCase(), to_kind: l.to_kind.toUpperCase() }));
  const again = kindRanks(["A", "B", "C"], kindPairs(renamed, new Set(["A", "B", "C"])));
  expect([again.get("A"), again.get("B"), again.get("C")]).toEqual([ranks.get("a"), ranks.get("b"), ranks.get("c")]);
  const all = kindRanks(names, kindPairs(links, new Set(names)));
  expect(Math.max(...all.values())).toBeLessThan(3);
});

test("rows keep proposed kinds visible and count what waits for review per kind", () => {
  const rows = kindRows(fixture.ontology);
  expect(rows).toHaveLength(16);
  expect(rows[0].kind).toBe("stock_item");
  expect(rows.reduce((n, r) => n + r.waiting, 0)).toBe(7);
  expect(rows.find((r) => r.kind === "warehouse").waiting).toBe(1);
});

test("beyond the stage limit the selection, its links and kinds in review come first", () => {
  const rows = kindRows(fixture.ontology);
  const pairs = kindPairs(links, new Set(names));
  expect(rows.length).toBeGreaterThan(MAX_STAGE_KINDS);
  const none = stageKinds(rows, pairs, "");
  expect(none.shown).toHaveLength(MAX_STAGE_KINDS);
  expect(none.hidden).toBe(rows.length - MAX_STAGE_KINDS);
  expect(none.shown).toContain("warehouse");
  const picked = stageKinds(rows, pairs, "location");
  expect(picked.shown[0]).toBe("location");
  for (const p of pairs.filter((p) => p.from === "location" || p.to === "location"))
    expect(picked.shown).toContain(p.from === "location" ? p.to : p.from);
  expect(stageKinds(rows.slice(0, 5), pairs, "").hidden).toBe(0);
});

test("lanes group neighbours by relation, direction and kind, so each verb appears once", () => {
  const data = richNeighbourhood();
  const scene = filterScene(data, "house", "", "both", 1);
  const lanes = buildLanes(scene, "house");
  expect(lanes.map((l) => l.verb).sort()).toEqual(["alternative to", "billed on", "on order", "shipped in", "stocked at", "supplies"]);
  expect(lanes.find((l) => l.verb === "supplies").dir).toBe("in");
  const orders = lanes.find((l) => l.verb === "on order");
  expect(orders.nodes).toHaveLength(24);
  expect(orders.nodes.length).toBeGreaterThan(LANE_OPEN_LIMIT);
  expect(laneOfNode(lanes, "order-3").id).toBe(orders.id);
  const reversed = buildLanes({ ...scene, nodes: [...scene.nodes].reverse(), edges: [...scene.edges].reverse() }, "house");
  expect(reversed.map((l) => [l.id, l.nodes.map((n) => n.id)])).toEqual(lanes.map((l) => [l.id, l.nodes.map((n) => n.id)]));
});

test("deeper hops sit in lanes tied to the lane that reached them", () => {
  const scene = filterScene(richNeighbourhood(2), "house", "", "both", 2);
  const lanes = buildLanes(scene, "house");
  const far = lanes.filter((l) => l.hop === 2);
  expect(far.length).toBeGreaterThan(0);
  for (const lane of far) expect(lanes.some((l) => l.id === lane.via)).toBe(true);
  const placed = placeLanes(lanes);
  expect(placed.far).toEqual(far);
  expect(placed.left.length).toBeLessThanOrEqual(3);
  expect(placed.right.length).toBeLessThanOrEqual(3);
  expect([...placed.left, ...placed.right, ...placed.below]).toHaveLength(lanes.filter((l) => l.hop === 1).length);
});

test("an object linked to the focus by two relations appears in both lanes", () => {
  const data = richNeighbourhood();
  data.edges.push({ id: "again", from: "house", to: "n0", label: "ordered_from" });
  const lanes = buildLanes(filterScene(data, "house", "", "both", 1), "house");
  expect(lanes.filter((l) => l.nodes.some((n) => n.id === "n0")).map((l) => l.verb).sort()).toEqual(["ordered from", "supplies"]);
});

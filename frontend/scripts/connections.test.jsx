import { test, expect } from "bun:test";
import {
  filterScene,
  edgeSentence,
} from "../src/components/twin/connections-model";
import { neighbourhood } from "../tests/fixtures/connections";
test("directed neighbourhoods and table share one filtered scene", () => {
  const data = neighbourhood();
  const incoming = filterScene(data, "house", "", "in", 1);
  expect(incoming.nodes.map((n) => n.id).sort()).toEqual(["house", "n0"]);
  expect(
    incoming.edges.map((e) =>
      edgeSentence(e, new Map(incoming.nodes.map((n) => [n.id, n]))),
    ),
  ).toEqual(["North Coast Beans supplies House blend 1kg"]);
  expect(filterScene(data, "house", "stocked_at", "out", 1).edges).toHaveLength(
    1,
  );
  expect(filterScene(data, "house", "supplies", "out", 1).nodes).toHaveLength(
    1,
  );
});
test("sentences retain the API subject, verb and object for opposing and self links", () => {
  const nodes = new Map([
    ["a", { id: "a", name: "House blend", kind: "product" }],
    ["b", { id: "b", name: "North Coast", kind: "supplier" }],
  ]);
  const edges = [
    { id: "ab", from: "a", label: "ordered_from", to: "b" },
    { id: "ba", from: "b", label: "Supplies", to: "a" },
    { id: "aa", from: "a", label: "replaces", to: "a" },
  ];
  expect(edges.map((e) => edgeSentence(e, nodes))).toEqual([
    "House blend ordered from North Coast",
    "North Coast Supplies House blend",
    "House blend replaces House blend",
  ]);
});

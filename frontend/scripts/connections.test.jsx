import { test, expect } from "bun:test";
import {
  filterScene,
  connectionsLayout,
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
test("stable layout groups unique nodes and deduplicates returned edges", () => {
  const data = neighbourhood(24);
  data.nodes.push(data.nodes[1]);
  data.edges.push(data.edges[0]);
  const scene = filterScene(data, "house", "", "both", 1),
    a = connectionsLayout(scene, "house"),
    b = connectionsLayout(
      { ...scene, nodes: [...scene.nodes].reverse() },
      "house",
    );
  expect(a.groups).toEqual(b.groups);
  expect(a.groups[0].count).toBe(24);
  expect(new Set(a.nodes.map((n) => n.id)).size).toBe(25);
  expect(scene.edges.length).toBe(24);
  expect(a.nodes.find((n) => n.id === "house")).toMatchObject({
    x: 600,
    y: 470,
    hop: 0,
  });
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

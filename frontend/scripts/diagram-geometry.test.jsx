import { test, expect } from "bun:test";
import { diagramGeometry } from "../src/components/chat/views/diagram-geometry";
import { boxesIntersect } from "../src/components/twin/canvas-geometry";
import { diagramFixture } from "../tests/fixtures/chat-diagrams";

for (const type of ["path", "neighbourhood", "impact", "kinds"]) {
  for (const width of [1440, 390]) {
    test(`${type} at ${width}: every verb has a clear plate on its route`, () => {
      const d = diagramFixture(type).diagram;
      // Include opposing, parallel and self edges, and a long verb.
      d.edges.push(
        {
          id: "parallel",
          from: d.nodes[1].id,
          to: d.nodes[0].id,
          label: "delivered_to",
        },
        {
          id: "opposing",
          from: d.nodes[0].id,
          to: d.nodes[1].id,
          label: "orders_from",
        },
        {
          id: "self",
          from: d.nodes[1].id,
          to: d.nodes[1].id,
          label: "is_part_of_the_delivery_plan",
        },
      );
      const g = diagramGeometry(d, width === 390);
      expect(g.edges).toHaveLength(d.edges.length);
      for (const [i, edge] of g.edges.entries()) {
        for (const n of g.nodes)
          expect(boxesIntersect(edge.box, n.box)).toBe(false);
        for (const other of g.edges.slice(i + 1))
          expect(boxesIntersect(edge.box, other.box)).toBe(false);
        expect(edge.path).toContain(
          `${edge.box.x + edge.box.w / 2},${edge.box.y + edge.box.h / 2}`,
        );
      }
      expect(diagramGeometry(d, width === 390)).toEqual(g);
    });
  }
}
test("phone projection preserves a chain and its stored edge direction", () => {
  const d = diagramFixture("path").diagram;
  const desktop = diagramGeometry(d, false),
    phone = diagramGeometry(d, true);
  expect(new Set(desktop.nodes.map((n) => n.y)).size).toBe(1);
  expect(new Set(phone.nodes.map((n) => n.x)).size).toBe(1);
  expect(phone.nodes.map((n) => n.y)).toEqual(
    [...phone.nodes.map((n) => n.y)].sort((a, b) => a - b),
  );
  expect(phone.edges.map((e) => [e.from, e.to])).toEqual(
    desktop.edges.map((e) => [e.from, e.to]),
  );
});

test("an empty selection has finite drawing bounds", () => {
  const d = diagramFixture("kinds").diagram;
  d.nodes = [];
  d.edges = [];
  expect(diagramGeometry(d, true)).toMatchObject({ width: 320, height: 100 });
});

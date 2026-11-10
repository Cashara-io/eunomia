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
          for (const box of n.collisionBoxes) expect(boxesIntersect(edge.box, box)).toBe(false);
        for (const other of g.edges.slice(i + 1))
          expect(boxesIntersect(edge.box, other.box)).toBe(false);
        const [sx, sy, cx, cy, ex, ey] = edge.path.match(/-?[\d.]+/g).map(Number);
        expect(edge.box.x + edge.box.w / 2).toBeCloseTo((sx + 2 * cx + ex) / 4);
        expect(edge.box.y + edge.box.h / 2).toBeCloseTo((sy + 2 * cy + ey) / 4);
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

test("capped impact keeps true distances and an entirely omitted band", () => {
  const d = diagramFixture("impact").diagram;
  d.nodes = d.nodes.filter((_, i) => i === 0 || i === 2);
  d.bands = [
    { distance: 0, shown: 1, omitted: 0, x: 150 },
    { distance: 1, shown: 0, omitted: 14, x: 410 },
    { distance: 2, shown: 1, omitted: 0, x: 670 },
  ];
  for (const phone of [false, true]) {
    expect(diagramGeometry(d, phone).bands.map((b) => b.text)).toEqual([
      "Source", "1 hop: 14 not shown", "2 hops: 1 shown",
    ]);
  }
});

test("ordinary path edges take one gentle curve between the forms", () => {
  const g = diagramGeometry(diagramFixture("path").diagram, false);
  for (const e of g.edges) {
    expect(e.path.match(/Q/g)).toHaveLength(1);
    const numbers = e.path.match(/-?[\d.]+/g).map(Number);
    const [sx, sy, cx, cy, ex, ey] = numbers;
    expect(Math.abs(cy - (sy + ey) / 2)).toBeLessThanOrEqual(160);
    expect(e.box.x + e.box.w / 2).toBeCloseTo((sx + 2 * cx + ex) / 4);
    expect(e.box.y + e.box.h / 2).toBeCloseTo((sy + 2 * cy + ey) / 4);
  }
});

test("kind links fit their labels between the forms without a deep detour", () => {
  const g = diagramGeometry(diagramFixture("kinds").diagram, false);
  for (const e of g.edges) {
    const [sx, sy, cx, cy, ex, ey] = e.path.match(/-?[\d.]+/g).map(Number);
    expect(Math.hypot(cx - (sx + ex) / 2, cy - (sy + ey) / 2)).toBeLessThanOrEqual(128);
  }
});

test("the full edge cap keeps every parallel label clear", () => {
  const d = diagramFixture("path").diagram;
  d.edges = Array.from({ length: 24 }, (_, i) => ({
    id: `parallel-${i}`, from: d.nodes[0].id, to: d.nodes[1].id, label: "supplied_by",
  }));
  const g = diagramGeometry(d, false);
  for (const [i, e] of g.edges.entries())
    for (const other of g.edges.slice(i + 1))
      expect(boxesIntersect(e.box, other.box)).toBe(false);
});


test("phone impact labels fit within the node column", () => {
  const g = diagramGeometry(diagramFixture("impact").diagram, true);
  const left = Math.min(...g.nodes.map((n) => n.box.x));
  const right = Math.max(...g.nodes.map((n) => n.box.x + n.box.w));
  for (const e of g.edges) {
    expect(e.box.x).toBeGreaterThanOrEqual(left);
    expect(e.box.x + e.box.w).toBeLessThanOrEqual(right);
  }
  expect(g.width * 0.86).toBeLessThanOrEqual(320);
});

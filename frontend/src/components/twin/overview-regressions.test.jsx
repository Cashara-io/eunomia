import { test, expect } from "bun:test";
import fixture from "../../../tests/fixtures/ontology-ui.json";
import { overviewLayout, routeLabels } from "./overview-layout";
import { linkSentence } from "./overview-format";

test("route endpoints and every label arrow follow the subject to its destination", () => {
  const layout = overviewLayout(fixture.ontology.kinds, fixture.ontology.links);
  const labels = routeLabels(layout, layout.edges);
  expect(labels.length).toBeGreaterThan(0);
  for (const edge of layout.edges) {
    const from = layout.nodes.find((n) => n.kind.kind === edge.link.from_kind);
    const to = layout.nodes.find((n) => n.kind.kind === edge.link.to_kind);
    expect(edge.path).toStartWith(`M${from.x},${from.y + 18} `);
    expect(edge.path).toEndWith(`${to.x},${to.y + 18}`);
  }
  for (const label of labels) {
    const from = layout.nodes.find((n) => n.kind.kind === label.link.from_kind);
    const to = layout.nodes.find((n) => n.kind.kind === label.link.to_kind);
    const t = label.t,
      u = 1 - t,
      bend = label.bend;
    const dx =
      3 * u * u * bend +
      6 * u * t * (to.x - from.x - 2 * bend) +
      3 * t * t * bend;
    const dy = 3 * u * u * 47 + 6 * u * t * (to.y - from.y) - 3 * t * t * 47;
    expect(label.angle).toBeCloseTo((Math.atan2(dy, dx) * 180) / Math.PI);
    expect(label.sentence).toBe(linkSentence(label.link));
  }
  const delivered = fixture.ontology.links.find(
    (l) => l.label === "delivered_to",
  );
  expect(linkSentence(delivered)).toBe("Order delivered to Location");
  expect(labels.some((l) => Math.abs(l.angle) > 90)).toBe(true);
});

for (const count of [1, 16, 60, 120])
  test(`${count} kinds and 300 links have stable bounds and collision-free label boxes`, () => {
    const kinds = Array.from({ length: count }, (_, i) => ({
      ...fixture.ontology.kinds[0],
      kind: `長い名前${i}ABCDEFGHIJKLMNOPQRSTUVWXYZ`,
      entities: count - i,
    }));
    const links = Array.from({ length: 300 }, (_, i) => ({
      ...fixture.ontology.links[0],
      label: `route_${i}`,
      count: 300 - i,
      from_kind: kinds[i % count].kind,
      to_kind: kinds[(i + 1) % count].kind,
    }));
    const layout = overviewLayout(kinds, links);
    expect(layout.nodes).toHaveLength(Math.min(count, 16));
    expect(layout.overflow).toHaveLength(Math.max(0, count - 16));
    expect(layout.height).toBeLessThanOrEqual(1200);
    expect(overviewLayout([...kinds].reverse(), [...links].reverse())).toEqual(
      layout,
    );
    expect(layout.edges.filter((e) => e.idle)).toHaveLength(
      Math.min(5, layout.edges.length),
    );
    const labels = routeLabels(layout, layout.edges);
    const boxes = [
      ...layout.nodes.map((n) => ({
        x: n.x - 116,
        y: n.y - n.height - 90,
        w: 232,
        h: 82,
      })),
      ...labels.map((l) => ({
        x: l.x - l.width / 2,
        y: l.y - l.height / 2,
        w: l.width,
        h: l.height,
      })),
    ];
    for (let i = 0; i < boxes.length; i++)
      for (let j = i + 1; j < boxes.length; j++) {
        const a = boxes[i],
          b = boxes[j];
        expect(
          a.x < b.x + b.w &&
            a.x + a.w > b.x &&
            a.y < b.y + b.h &&
            a.y + a.h > b.y,
        ).toBe(false);
      }
  });

for (const count of [16, 60, 120])
  test(`low-count kinds with owner issues join the top kinds at ${count}`, () => {
    const kinds = Array.from({ length: count }, (_, i) => ({
      ...fixture.ontology.kinds[0],
      kind: `kind_${i}`,
      entities: count - i,
    }));
    const attention = new Set(kinds.slice(-2).map((k) => k.kind));
    const layout = overviewLayout(kinds, [], attention);
    for (const kind of attention)
      expect(layout.nodes.some((n) => n.kind.kind === kind)).toBe(true);
    expect(layout.nodes.length).toBeLessThanOrEqual(20);
    for (const kind of kinds.slice(0, 16))
      expect(layout.nodes.some((n) => n.kind.kind === kind.kind)).toBe(true);
    expect(layout.nodes.length + layout.overflow.length).toBe(count);
  });

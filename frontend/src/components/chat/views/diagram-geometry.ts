import { boxesIntersect, type Box } from "../../twin/canvas-geometry";
import { formCentre, formPort } from "../../twin/connections-model";
import type { DiagramView } from "./diagram-schema";
type Diagram = DiagramView["diagram"];

// Only a fixed phone projection and collision-free routes are computed here.
// Node order, distance bands and coordinates come from the saved server result.
export function diagramGeometry(d: Diagram, phone: boolean) {
  if (!d.nodes.length)
    return {
      nodes: [],
      edges: [],
      bands: [],
      left: 0,
      top: 0,
      width: 320,
      height: 100,
    };
  const vertical =
    phone && (d.diagram_type === "path" || d.diagram_type === "impact");
  const nodes = d.nodes.map((n) => {
    const x = vertical ? n.y : n.x,
      y = vertical ? n.x : n.y;
    const height =
      d.diagram_type === "kinds"
        ? 12 +
          Math.sqrt(
            (n.count ?? 0) / Math.max(1, ...d.nodes.map((n) => n.count ?? 0)),
          ) *
            70
        : n.focus
          ? 35
          : 20;
    const plateHeight =
      (d.diagram_type === "kinds" ? 46 : 76) + n.metrics.length * 20;
    return {
      ...n,
      x,
      y,
      height,
      scale: 0.8,
      plateHeight,
      box: {
        x: x - 86,
        y: y - height - plateHeight - 38,
        w: 172,
        h: height + plateHeight + 72,
      },
    };
  });
  const bands =
    d.diagram_type === "impact"
      ? [...new Set(d.nodes.map((n) => n.x))]
          .sort((a, b) => a - b)
          .map((x, i) => {
            const bandNodes = nodes.filter((n) =>
              vertical ? n.y === x : n.x === x,
            );
            const count = bandNodes.length;
            return {
              text:
                i === 0
                  ? "Source"
                  : `${i} ${i === 1 ? "hop" : "hops"} · ${count} shown`,
              x: vertical ? Math.min(...bandNodes.map((n) => n.box.x)) : x - 86,
              y: Math.min(...bandNodes.map((n) => n.box.y)) - 34,
              w: 172,
              h: 26,
            };
          })
      : [];
  const occupied: Box[] = [...nodes.map((n) => n.box), ...bands];
  const byId = new Map(nodes.map((n) => [n.id, n]));
  const edges = [...d.edges]
    .sort((a, b) => a.id.localeCompare(b.id))
    .flatMap((e, i) => {
      const a = byId.get(e.from),
        b = byId.get(e.to);
      if (!a || !b) return [];
      const from = formCentre(a),
        to = formCentre(b);
      const label =
        e.label.replaceAll("_", " ") +
        (e.count === undefined ? "" : ` · ${e.count}`);
      const units = [...label].reduce(
        (sum, c) => sum + (/[MW@\u0080-\uffff]/.test(c) ? 2 : 1),
        0,
      );
      const w = Math.min(176, Math.max(80, label.length * 9 + 24));
      const h = units > 16 ? Math.ceil(units / 14) * 24 + 12 : 38;
      let box: Box | undefined;
      const dx = to.x - from.x,
        dy = to.y - from.y,
        len = Math.hypot(dx, dy) || 1;
      for (const offset of [
        0, 48, -48, 96, -96, 160, -160, 240, -240, 340, -340,
      ]) {
        for (const t of [0.5, 0.35, 0.65, 0.2, 0.8]) {
          const candidate = {
            x: from.x + dx * t - (dy / len) * offset - w / 2,
            y: from.y + dy * t + (dx / len) * offset - h / 2,
            w,
            h,
          };
          if (!occupied.some((r) => boxesIntersect(candidate, r))) {
            box = candidate;
            break;
          }
        }
        if (box) break;
      }
      // Reserve a separate outside lane if the middle is full. Every verb stays visible.
      box ??= {
        x: Math.min(...nodes.map((n) => n.box.x)),
        y: Math.max(...occupied.map((r) => r.y + r.h)) + 20,
        w,
        h,
      };
      occupied.push({ x: box.x - 5, y: box.y - 5, w: w + 10, h: h + 10 });
      const centre = { x: box.x + w / 2, y: box.y + h / 2 };
      const self = a.id === b.id;
      const start = formPort(
        a,
        self ? { x: from.x + 100, y: from.y - 30 } : centre,
      );
      const end = formPort(b, self ? { x: to.x + 100, y: to.y + 30 } : centre);
      // Two curves meet at the label centre, so the plate is always on its own edge.
      const lane = ((i % 5) - 2) * 5;
      const path = `M${start.x},${start.y} Q${(start.x + centre.x) / 2 + lane},${(start.y + centre.y) / 2 - lane} ${centre.x},${centre.y} Q${(end.x + centre.x) / 2 - lane},${(end.y + centre.y) / 2 + lane} ${end.x},${end.y}`;
      return [{ ...e, label, box, path }];
    });
  const left = Math.min(...occupied.map((r) => r.x)) - 20;
  const top = Math.min(...occupied.map((r) => r.y)) - 24;
  const width = Math.max(...occupied.map((r) => r.x + r.w)) - left + 20;
  const height = Math.max(...occupied.map((r) => r.y + r.h)) - top + 24;
  return { nodes, edges, bands, left, top, width, height };
}

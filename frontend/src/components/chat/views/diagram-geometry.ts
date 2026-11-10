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
      collisionBoxes: [
        { x: x - 80, y: y - height - plateHeight - 32, w: 160, h: plateHeight },
        { x: x - 52, y: y - height - 20, w: 104, h: height + 34 },
      ],
      box: {
        x: x - 86,
        y: y - height - plateHeight - 38,
        w: 172,
        h: height + plateHeight + 50,
      },
    };
  });
  const bands = d.diagram_type === "impact"
    ? d.bands.map((band) => {
        const bandNodes = nodes.filter((n) => n.distance === band.distance);
        return {
          text: bandLabel(band),
          x: vertical ? Math.min(...nodes.map((n) => n.box.x)) : band.x - 86,
          y: vertical
            ? band.x - 190
            : Math.min(...(bandNodes.length ? bandNodes : nodes).map((n) => n.box.y)) - 34,
          w: 230,
          h: 26,
        };
      })
    : [];
  const occupied: Box[] = [...nodes.flatMap((n) => n.collisionBoxes), ...bands];
  const byId = new Map(nodes.map((n) => [n.id, n]));
  const edges = [...d.edges]
    .sort((a, b) => a.id.localeCompare(b.id))
    .flatMap((e) => {
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
      const w = Math.min(e.count === undefined ? 176 : 120, Math.max(80, label.length * 8 + 16));
      const h = units > 16 ? Math.ceil(units / 14) * 24 + 12 : 38;
      const self = a.id === b.id;
      const start = formPort(a, self ? { x: from.x + 100, y: from.y - 30 } : to);
      const end = formPort(b, self ? { x: to.x + 100, y: to.y + 30 } : from);
      const dx = end.x - start.x, dy = end.y - start.y;
      const len = Math.hypot(dx, dy) || 1;
      let box: Box | undefined;
      let midpoint = { x: 0, y: 0 };
      // A single quadratic has a continuous tangent through its midpoint.
      // Try small perpendicular offsets first, keeping ordinary links close to the forms.
      const columnLeft = Math.min(...nodes.map((n) => n.box.x));
      const columnRight = Math.max(...nodes.map((n) => n.box.x + n.box.w));
      // On phones, first keep plates in the node column so labels remain visible
      // without sideways scrolling. Dense graphs can still use the wider canvas.
      const alongOffsets = vertical
        ? [0, 16, -16, 24, -24, 48, -48, 64, -64, 80, -80, 96, -96]
        : [0, 16, -16, 24, -24, 48, -48];
      placement: for (const inColumn of vertical ? [true, false] : [false]) {
        for (const offset of [0, 32, -32, 48, -48, 64, -64, 72, -72, 96, -96, 160, -160, 240, -240, 340, -340, 480, -480, 680, -680]) {
          for (const along of alongOffsets) {
            midpoint = self
              ? { x: from.x + 140 + Math.abs(offset), y: from.y }
              : { x: (start.x + end.x) / 2 - dy / len * offset + dx / len * along,
                  y: (start.y + end.y) / 2 + dx / len * offset + dy / len * along };
            const candidate = { x: midpoint.x - w / 2, y: midpoint.y - h / 2, w, h };
            if (inColumn && (candidate.x < columnLeft || candidate.x + w > columnRight)) continue;
            box = candidate;
            if (!occupied.some((r) => boxesIntersect(candidate, r))) break placement;
          }
        }
      }
      // Dense parallel links may fill every nearby slot. Reserve a clear outer midpoint
      // for those labels, while keeping the same continuous curve and stored direction.
      if (!box || occupied.some((r) => box && boxesIntersect(box, r))) {
        midpoint = {
          x: Math.max(...occupied.map((r) => r.x + r.w)) + w / 2 + 20,
          y: (start.y + end.y) / 2,
        };
        box = { x: midpoint.x - w / 2, y: midpoint.y - h / 2, w, h };
      }
      occupied.push({ x: box.x - 5, y: box.y - 5, w: w + 10, h: h + 10 });
      const control = { x: 2 * midpoint.x - (start.x + end.x) / 2,
                  y: 2 * midpoint.y - (start.y + end.y) / 2 };
      const path = `M${start.x},${start.y} Q${control.x},${control.y} ${end.x},${end.y}`;
      return [{ ...e, label, box, path }];
    });
  const left = Math.min(...occupied.map((r) => r.x)) - 20;
  const top = Math.min(...occupied.map((r) => r.y)) - 24;
  const width = Math.max(...occupied.map((r) => r.x + r.w)) - left + 20;
  const height = Math.max(...occupied.map((r) => r.y + r.h)) - top + 24;
  return { nodes, edges, bands, left, top, width, height };
}

export function bandLabel(b: Diagram["bands"][number]) {
  if (b.distance === 0) return "Source";
  const counts = [
    b.shown ? `${b.shown} shown` : "",
    b.omitted ? `${b.omitted} not shown` : "",
  ].filter(Boolean).join(", ");
  return `${b.distance} ${b.distance === 1 ? "hop" : "hops"}: ${counts}`;
}

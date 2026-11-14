import type {
  ConnectionNode,
  ConnectionEdge,
  Neighbourhood,
} from "@/lib/queries/connections";
export type Direction = "both" | "in" | "out";
export type Scene = {
  nodes: ConnectionNode[];
  edges: ConnectionEdge[];
  hops: Record<string, number>;
};
export function edgeSentence(
  edge: ConnectionEdge,
  nodes: ReadonlyMap<string, ConnectionNode>,
) {
  return `${nodes.get(edge.from)?.name ?? "Unavailable object"} ${edge.label.replaceAll("_", " ")} ${nodes.get(edge.to)?.name ?? "Unavailable object"}`;
}
export function filterScene(
  data: Neighbourhood,
  focus: string,
  relation: string,
  direction: Direction,
  depth: number,
): Scene {
  const nodes = new Map(data.nodes.map((n) => [n.id, n]));
  const edges = [
    ...new Map(
      data.edges
        .filter(
          (e) =>
            nodes.has(e.from) &&
            nodes.has(e.to) &&
            (!relation || e.label === relation),
        )
        .map((e) => [e.id, e]),
    ).values(),
  ];
  const hops: Record<string, number> = {};
  if (nodes.has(focus)) hops[focus] = 0;
  let frontier = [focus];
  for (let h = 1; h <= depth; h++) {
    const next = new Set<string>();
    for (const id of frontier)
      for (const e of edges) {
        const target =
          e.from === id && direction !== "in"
            ? e.to
            : e.to === id && direction !== "out"
              ? e.from
              : null;
        if (target && hops[target] === undefined) {
          hops[target] = h;
          next.add(target);
        }
      }
    frontier = [...next];
  }
  // Keep only edges traversable in the selected direction from the focus.
  return {
    nodes: [...nodes.values()].filter((n) => hops[n.id] !== undefined),
    hops,
    edges: edges.filter(
      (e) =>
        hops[e.from] !== undefined &&
        hops[e.to] !== undefined &&
        (direction === "both" ||
          (direction === "out" ? hops[e.from] < depth : hops[e.to] < depth)),
    ),
  };
}
export type FormAnchor = {
  id: string;
  x: number;
  y: number;
  height: number;
  scale: number;
};
export function formCentre(a: FormAnchor) {
  return { x: a.x + a.scale, y: a.y - a.height - 4 * a.scale };
}
// Intersect the line with the actual top-face polygon, including its isometric skew.
export function formPort(a: FormAnchor, toward: { x: number; y: number }) {
  const centre = formCentre(a);
  const dx = toward.x - centre.x,
    dy = toward.y - centre.y;
  const points = [
    [-62, -9],
    [26, -23],
    [64, 1],
    [-24, 15],
  ].map(([x, y]) => ({
    x: a.x + x * a.scale,
    y: a.y + y * a.scale - a.height,
  }));
  for (let i = 0; i < points.length; i++) {
    const p = points[i],
      q = points[(i + 1) % points.length];
    const sx = q.x - p.x,
      sy = q.y - p.y;
    const det = dx * sy - dy * sx;
    if (Math.abs(det) < 0.00001) continue;
    const px = p.x - centre.x,
      py = p.y - centre.y;
    const t = (px * sy - py * sx) / det;
    const u = (px * dy - py * dx) / det;
    if (t >= 0 && u >= 0 && u <= 1)
      return { x: centre.x + dx * t, y: centre.y + dy * t };
  }
  return points[0];
}

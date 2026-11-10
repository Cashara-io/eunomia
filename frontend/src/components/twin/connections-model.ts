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
export type PlacedNode = ConnectionNode & {
  x: number;
  y: number;
  hop: number;
  group: string;
};
export type Layout = {
  nodes: PlacedNode[];
  groups: {
    id: string;
    kind: string;
    hop: number;
    count: number;
    x: number;
    y: number;
  }[];
  ms: number;
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
export function connectionsLayout(scene: Scene, focus: string): Layout {
  const start = performance.now();
  const buckets = new Map<string, ConnectionNode[]>();
  for (const node of scene.nodes)
    if (node.id !== focus) {
      const key = JSON.stringify([scene.hops[node.id], node.kind]);
      const group = buckets.get(key) ?? [];
      group.push(node);
      buckets.set(key, group);
    }
  const nodes: PlacedNode[] = scene.nodes
    .filter((n) => n.id === focus)
    .map((n) => ({ ...n, x: 600, y: 470, hop: 0, group: "" }));
  const groups: Layout["groups"] = [];
  const byHop = new Map<number, string[]>();
  for (const [id, items] of buckets) {
    const hop = scene.hops[items[0].id];
    byHop.set(hop, [...(byHop.get(hop) ?? []), id]);
  }
  for (const [hop, keys] of byHop) {
    keys.sort();
    const slots = keys.flatMap((id) => {
      const members = (buckets.get(id) ?? []).sort(
        (a, b) => a.name.localeCompare(b.name) || a.id.localeCompare(b.id),
      );
      return members.length >= 8
        ? [{ id, members }]
        : members.map((n) => ({ id, members: [n] }));
    });
    const rx = (slots.length <= 5 ? 300 : 450) + (hop - 1) * 350;
    const ry = (slots.length <= 5 ? 220 : 350) + (hop - 1) * 290;
    slots.forEach(({ id, members }, i) => {
      const angle = -Math.PI / 2 + (i * Math.PI * 2) / slots.length;
      const x = 600 + Math.cos(angle) * rx;
      const y = 470 + Math.sin(angle) * ry;
      if (members.length >= 8)
        groups.push({
          id,
          kind: members[0].kind,
          hop,
          count: members.length,
          x,
          y,
        });
      members.forEach((n, j) =>
        nodes.push({
          ...n,
          hop,
          group: members.length >= 8 ? id : "",
          x: members.length === 1 ? x : x + ((j % 5) - 2) * 225,
          y: members.length === 1 ? y : y + 180 + Math.floor(j / 5) * 155,
        }),
      );
    });
  }
  return { nodes, groups, ms: performance.now() - start };
}

export function loadedDegrees(scene: Scene) {
  const degrees = new Map(scene.nodes.map((n) => [n.id, 0]));
  for (const edge of scene.edges) {
    degrees.set(edge.from, (degrees.get(edge.from) ?? 0) + 1);
    if (edge.to !== edge.from)
      degrees.set(edge.to, (degrees.get(edge.to) ?? 0) + 1);
  }
  return degrees;
}
export const formHeight = (degree: number) => 12 + Math.sqrt(degree) * 9;
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

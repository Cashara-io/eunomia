import type {
  ConnectionNode,
  ConnectionEdge,
  Neighbourhood,
} from "@/lib/queries/connections";
import { humanLabel } from "./overview-format";
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
  return `${nodes.get(edge.from)?.name ?? "Unavailable object"} ${humanLabel(edge.label).toLowerCase()} ${nodes.get(edge.to)?.name ?? "Unavailable object"}`;
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
    .map((n) => ({ ...n, x: 600, y: 370, hop: 0, group: "" }));
  const groups: Layout["groups"] = [];
  const byHop = new Map<number, string[]>();
  for (const [id, items] of buckets) {
    const hop = scene.hops[items[0].id];
    const keys = byHop.get(hop) ?? [];
    keys.push(id);
    byHop.set(hop, keys);
  }
  for (const [hop, keys] of byHop) {
    keys.sort();
    keys.forEach((id, i) => {
      const members = (buckets.get(id) ?? []).sort(
        (a, b) => a.name.localeCompare(b.name) || a.id.localeCompare(b.id),
      );
      const side = i % 2 === 0 ? -1 : 1;
      const rows = Math.ceil(keys.length / 2);
      const x = 600 + side * (260 + (hop - 1) * 500);
      const y = 370 + (Math.floor(i / 2) - (rows - 1) / 2) * 250;
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
          group: id,
          x:
            members.length === 1
              ? x
              : keys.length === 1
                ? 600 + ((j % 3) - 1) * 235
                : x + side * (280 + (j % 3) * 235),
          y:
            members.length === 1
              ? y
              : y + (keys.length === 1 ? 170 : 0) + Math.floor(j / 3) * 185,
        }),
      );
    });
  }
  return { nodes, groups, ms: performance.now() - start };
}

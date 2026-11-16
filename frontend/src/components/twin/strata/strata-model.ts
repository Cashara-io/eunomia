import type { LinkItem, OntologyOut } from "@/lib/queries/ontology";
import type { ConnectionNode } from "@/lib/queries/connections";
import type { Scene } from "../connections-model";
import { definitions, visibleKind } from "../model";
import { humanLabel } from "../overview-format";

/** The Kinds level shows at most this many kinds. The rail lists every kind. */
export const MAX_STAGE_KINDS = 14;
/** A lane with more members than this is a stack that opens. */
export const LANE_OPEN_LIMIT = 5;
const MAX_RANKS = 3;

export type KindPair = {
  from: string;
  to: string;
  count: number;
  reverse: number;
  links: LinkItem[];
};

/** Kinds that exist as definitions, with their object counts and how many of their definitions await review. */
export function kindRows(data: OntologyOut) {
  const waiting = new Map<string, number>();
  for (const d of definitions(data))
    if (d.item.status === "proposed")
      waiting.set(d.kind, (waiting.get(d.kind) ?? 0) + 1);
  return data.kinds
    .filter(visibleKind)
    .map((k) => ({ kind: k.kind, entities: k.entities, status: k.status, waiting: waiting.get(k.kind) ?? 0 }))
    .sort((a, b) => b.entities - a.entities || a.kind.localeCompare(b.kind));
}

/** One entry per pair of kinds, oriented by the heavier direction of the recorded links. */
export function kindPairs(links: LinkItem[], kinds: ReadonlySet<string>): KindPair[] {
  const pairs = new Map<string, { lo: string; hi: string; up: number; down: number; links: LinkItem[] }>();
  for (const link of links) {
    if (link.status === "rejected" || link.from_kind === link.to_kind) continue;
    if (!kinds.has(link.from_kind) || !kinds.has(link.to_kind)) continue;
    const [lo, hi] = [link.from_kind, link.to_kind].sort();
    const key = JSON.stringify([lo, hi]);
    const pair = pairs.get(key) ?? { lo, hi, up: 0, down: 0, links: [] };
    if (link.from_kind === lo) pair.up += link.count;
    else pair.down += link.count;
    pair.links.push(link);
    pairs.set(key, pair);
  }
  return [...pairs.values()]
    .map((p) => {
      const forward = p.up >= p.down;
      return {
        from: forward ? p.lo : p.hi,
        to: forward ? p.hi : p.lo,
        count: p.up + p.down,
        reverse: forward ? p.down : p.up,
        links: [...p.links].sort((a, b) => a.label.localeCompare(b.label) || a.from_kind.localeCompare(b.from_kind)),
      };
    })
    .sort((a, b) => b.count - a.count || a.from.localeCompare(b.from) || a.to.localeCompare(b.to));
}

/** Column per kind, computed from link direction: sources first, what they feed after. Cycles are broken at the lightest link. */
export function kindRanks(names: string[], pairs: KindPair[]) {
  const next = new Map<string, string[]>(names.map((n) => [n, []]));
  const reaches = (start: string, goal: string) => {
    const seen = new Set<string>();
    const stack = [start];
    while (stack.length) {
      const id = stack.pop() as string;
      if (id === goal) return true;
      if (seen.has(id)) continue;
      seen.add(id);
      stack.push(...(next.get(id) ?? []));
    }
    return false;
  };
  const dag: KindPair[] = [];
  for (const p of pairs) {
    if (reaches(p.to, p.from)) continue;
    next.get(p.from)?.push(p.to);
    dag.push(p);
  }
  const rank = new Map<string, number>(names.map((n) => [n, 0]));
  for (let pass = 0; pass < names.length; pass++) {
    let changed = false;
    for (const p of dag) {
      const wanted = (rank.get(p.from) ?? 0) + 1;
      if ((rank.get(p.to) ?? 0) < wanted) {
        rank.set(p.to, wanted);
        changed = true;
      }
    }
    if (!changed) break;
  }
  const deepest = Math.max(0, ...rank.values());
  if (deepest >= MAX_RANKS)
    for (const [name, value] of rank)
      rank.set(name, Math.floor((value * (MAX_RANKS - 1)) / deepest));
  return rank;
}

/** Which kinds the stage draws. Beyond the limit: the selection, what it links to, kinds in review, then the largest. */
export function stageKinds(
  rows: ReturnType<typeof kindRows>,
  pairs: KindPair[],
  selected: string,
) {
  if (rows.length <= MAX_STAGE_KINDS) return { shown: rows.map((r) => r.kind), hidden: 0 };
  const weight = new Map<string, number>();
  for (const p of pairs)
    if (p.from === selected) weight.set(p.to, p.count);
    else if (p.to === selected) weight.set(p.from, p.count);
  const order = [...rows].sort((a, b) => {
    const score = (r: typeof a) =>
      r.kind === selected ? 3 : weight.has(r.kind) ? 2 : r.waiting > 0 ? 1 : 0;
    return (
      score(b) - score(a) ||
      (weight.get(b.kind) ?? 0) - (weight.get(a.kind) ?? 0) ||
      b.entities - a.entities ||
      a.kind.localeCompare(b.kind)
    );
  });
  const shown = order.slice(0, MAX_STAGE_KINDS).map((r) => r.kind);
  return { shown, hidden: rows.length - shown.length };
}

export type Lane = {
  id: string;
  hop: number;
  label: string;
  dir: "in" | "out";
  kind: string;
  verb: string;
  via: string;
  nodes: ConnectionNode[];
};

/** Neighbours grouped by relation, direction and kind, so each verb is written once. A node sits in the lane it was first reached by. */
export function buildLanes(scene: Scene, focus: string): Lane[] {
  const lanes = new Map<string, Lane>();
  const laneOf = new Map<string, string>();
  const edges = [...scene.edges].sort((a, b) => a.id.localeCompare(b.id));
  const nodes = scene.nodes
    .filter((n) => n.id !== focus && scene.hops[n.id] !== undefined)
    .sort((a, b) => scene.hops[a.id] - scene.hops[b.id] || a.id.localeCompare(b.id));
  const near = new Map(scene.nodes.map((n) => [n.id, n]));
  for (const edge of edges) {
    if (edge.from === edge.to) continue;
    const dir = edge.from === focus ? "out" : edge.to === focus ? "in" : null;
    const node = near.get(dir === "out" ? edge.to : edge.from);
    if (!dir || !node || scene.hops[node.id] !== 1) continue;
    const id = JSON.stringify([1, edge.label, dir === "out" ? "out" : "in", node.kind, ""]);
    const lane = lanes.get(id) ?? { id, hop: 1, label: edge.label, dir: dir === "out" ? "out" : "in", kind: node.kind, verb: humanLabel(edge.label).toLowerCase(), via: "", nodes: [] } satisfies Lane;
    if (!lane.nodes.some((n) => n.id === node.id)) lane.nodes.push(node);
    lanes.set(id, lane);
    if (!laneOf.has(node.id)) laneOf.set(node.id, id);
  }
  for (const node of nodes.filter((n) => scene.hops[n.id] > 1)) {
    const hop = scene.hops[node.id];
    const edge = edges.find(
      (e) =>
        (e.from === node.id && scene.hops[e.to] === hop - 1) ||
        (e.to === node.id && scene.hops[e.from] === hop - 1),
    );
    if (!edge) continue;
    const dir = edge.to === node.id ? "out" : "in";
    const parent = dir === "out" ? edge.from : edge.to;
    const via = hop > 1 ? (laneOf.get(parent) ?? "") : "";
    const id = JSON.stringify([hop, edge.label, dir, node.kind, via]);
    const lane = lanes.get(id) ?? {
      id,
      hop,
      label: edge.label,
      dir,
      kind: node.kind,
      verb: humanLabel(edge.label).toLowerCase(),
      via,
      nodes: [],
    };
    lane.nodes.push(node);
    lanes.set(id, lane);
    laneOf.set(node.id, id);
  }
  for (const lane of lanes.values())
    lane.nodes.sort((a, b) => a.name.localeCompare(b.name) || a.id.localeCompare(b.id));
  return [...lanes.values()].sort(
    (a, b) =>
      a.hop - b.hop ||
      (a.dir === b.dir ? 0 : a.dir === "in" ? -1 : 1) ||
      a.verb.localeCompare(b.verb) ||
      a.kind.localeCompare(b.kind),
  );
}

/** Lanes around the focus: incoming on the left, outgoing on the right, and what overflows three a side below. */
export function placeLanes(lanes: Lane[]) {
  const near = lanes.filter((l) => l.hop === 1);
  const left = near.filter((l) => l.dir === "in");
  const right = near.filter((l) => l.dir === "out");
  const SIDE = 3;
  return {
    left: left.slice(0, SIDE),
    right: right.slice(0, SIDE),
    below: [...left.slice(SIDE), ...right.slice(SIDE)],
    far: lanes.filter((l) => l.hop > 1),
  };
}

export const laneOfNode = (lanes: Lane[], id: string) =>
  lanes.find((l) => l.nodes.some((n) => n.id === id));

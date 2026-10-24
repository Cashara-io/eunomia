// Analysis of the points the vector cloud API returns. Everything here is
// computed in the shared 3D PCA space (the only coordinates the browser has),
// so distances describe the picture, not the full embedding space.

export type InsightPoint = { id: string; vault: string; kind: string; label: string; x: number; y: number; z: number };

export type VaultStat = {
  vault: string;
  count: number;
  kinds: { kind: string; count: number }[];
  /** mean distance of the vault's points to its own centroid */
  spread: number;
  centroid: [number, number, number];
};

export type Pair = { a: InsightPoint; b: InsightPoint; distance: number };
/** `ratio`: multiples of the vault's mean spread */
export type Outlier = { point: InsightPoint; distance: number; ratio: number };
/** `separation`: centroid distance relative to the two vaults' mean spread; `mixed`: share of the two vaults' points whose nearest neighbour sits in the other vault */
export type VaultGap = { a: string; b: string; distance: number; separation: number; mixed: number };
export type Cluster = { size: number; representative: InsightPoint; vaults: number };

export type Insights = {
  total: number;
  vaults: VaultStat[];
  kinds: { kind: string; count: number }[];
  closestPairs: Pair[];
  outliers: Outlier[];
  gaps: VaultGap[];
  clusters: Cluster[];
  isolated: number;
  /** mean distance from a point to its nearest neighbour */
  meanNearest: number;
};

const dist = (a: InsightPoint, b: InsightPoint) => Math.hypot(a.x - b.x, a.y - b.y, a.z - b.z);
const centroidOf = (ps: InsightPoint[]): [number, number, number] => {
  const n = Math.max(1, ps.length);
  return [ps.reduce((s, p) => s + p.x, 0) / n, ps.reduce((s, p) => s + p.y, 0) / n, ps.reduce((s, p) => s + p.z, 0) / n];
};
const toCentroid = (p: InsightPoint, c: [number, number, number]) => Math.hypot(p.x - c[0], p.y - c[1], p.z - c[2]);

function count(items: string[]) {
  const m = new Map<string, number>();
  for (const i of items) m.set(i, (m.get(i) ?? 0) + 1);
  return [...m].map(([kind, n]) => ({ kind, count: n })).sort((a, b) => b.count - a.count || a.kind.localeCompare(b.kind));
}

export function analyse(points: InsightPoint[], vaultOrder: string[]): Insights {
  const n = points.length;
  const byVault = new Map<string, InsightPoint[]>();
  for (const p of points) byVault.set(p.vault, [...(byVault.get(p.vault) ?? []), p]);
  const order = [...byVault.keys()].sort((a, b) => (vaultOrder.indexOf(a) + 1 || 99) - (vaultOrder.indexOf(b) + 1 || 99));

  const vaults: VaultStat[] = order.map((vault) => {
    const ps = byVault.get(vault)!;
    const centroid = centroidOf(ps);
    return {
      vault,
      count: ps.length,
      kinds: count(ps.map((p) => p.kind)),
      spread: ps.reduce((s, p) => s + toCentroid(p, centroid), 0) / ps.length,
      centroid,
    };
  });
  const statOf = new Map(vaults.map((v) => [v.vault, v]));

  // Nearest neighbour of every point; the smallest pair distances are kept for the closest list.
  const nearest: { j: number; d: number }[] = points.map(() => ({ j: -1, d: Infinity }));
  let pairs: { i: number; j: number; d: number }[] = [];
  const PAIR_CAP = 4000;
  for (let i = 0; i < n; i++) {
    for (let j = i + 1; j < n; j++) {
      const d = dist(points[i], points[j]);
      if (d < nearest[i].d) nearest[i] = { j, d };
      if (d < nearest[j].d) nearest[j] = { j: i, d };
      pairs.push({ i, j, d });
      if (pairs.length > PAIR_CAP * 2) pairs = pairs.sort((a, b) => a.d - b.d).slice(0, PAIR_CAP);
    }
  }
  pairs.sort((a, b) => a.d - b.d);
  const closestPairs: Pair[] = pairs.slice(0, 6).map(({ i, j, d }) => ({ a: points[i], b: points[j], distance: d }));
  const linked = nearest.filter((x) => x.j >= 0);
  const meanNearest = linked.length ? linked.reduce((s, x) => s + x.d, 0) / linked.length : 0;

  // Outliers: farthest from their own vault's centroid, relative to that vault's spread.
  const outliers: Outlier[] = points
    .map((point) => {
      const v = statOf.get(point.vault)!;
      const distance = toCentroid(point, v.centroid);
      return { point, distance, ratio: v.spread > 1e-9 ? distance / v.spread : 0 };
    })
    .filter((o) => n >= 4 && o.ratio > 1.5)
    .sort((a, b) => b.ratio - a.ratio)
    .slice(0, 5);

  // Vault pairs: centroid gap and how much their neighbourhoods interleave.
  const gaps: VaultGap[] = [];
  for (let a = 0; a < vaults.length; a++) {
    for (let b = a + 1; b < vaults.length; b++) {
      const A = vaults[a];
      const B = vaults[b];
      const distance = Math.hypot(A.centroid[0] - B.centroid[0], A.centroid[1] - B.centroid[1], A.centroid[2] - B.centroid[2]);
      const mean = (A.spread + B.spread) / 2;
      const members = points.map((p, i) => ({ p, i })).filter(({ p }) => p.vault === A.vault || p.vault === B.vault);
      const crossing = members.filter(({ p, i }) => nearest[i].j >= 0 && points[nearest[i].j].vault !== p.vault).length;
      gaps.push({ a: A.vault, b: B.vault, distance, separation: mean > 1e-9 ? distance / mean : 0, mixed: members.length ? crossing / members.length : 0 });
    }
  }

  // Clusters: single linkage, an edge when two points sit within 1.4x the mean nearest distance.
  const eps = meanNearest * 1.4;
  const parent = points.map((_, i) => i);
  const find = (i: number): number => {
    while (parent[i] !== i) {
      parent[i] = parent[parent[i]];
      i = parent[i];
    }
    return i;
  };
  const union = (i: number, j: number) => {
    parent[find(i)] = find(j);
  };
  if (eps > 0) {
    for (const { i, j, d } of pairs) if (d <= eps) union(i, j);
    // the list above is capped; every nearest-neighbour link counts regardless
    nearest.forEach((x, i) => x.j >= 0 && x.d <= eps && union(i, x.j));
  }
  const groups = new Map<number, number[]>();
  points.forEach((_, i) => {
    const r = find(i);
    groups.set(r, [...(groups.get(r) ?? []), i]);
  });
  const real = [...groups.values()].filter((g) => g.length >= 2).sort((a, b) => b.length - a.length);
  const clusters: Cluster[] = real.slice(0, 5).map((g) => {
    const ps = g.map((i) => points[i]);
    const c = centroidOf(ps);
    const representative = ps.reduce((best, p) => (toCentroid(p, c) < toCentroid(best, c) ? p : best), ps[0]);
    return { size: g.length, representative, vaults: new Set(ps.map((p) => p.vault)).size };
  });
  const isolated = [...groups.values()].filter((g) => g.length === 1).length;

  return { total: n, vaults, kinds: count(points.map((p) => p.kind)), closestPairs, outliers, gaps, clusters, isolated, meanNearest };
}

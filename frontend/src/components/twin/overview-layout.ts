import type { KindItem, LinkItem } from "@/lib/queries/ontology";

export const linkKey = (link: LinkItem) =>
  JSON.stringify([link.from_kind, link.label, link.to_kind]);

// Stable rows on a gently receding plane. Height uses a square-root scale so a
// large kind has presence without hiding smaller kinds. Zero stays flat.
export function overviewLayout(kinds: KindItem[], links: LinkItem[]) {
  const linked = new Set(links.flatMap((l) => [l.from_kind, l.to_kind]));
  const sorted = [...kinds].sort(
    (a, b) =>
      Number(!linked.has(a.kind)) - Number(!linked.has(b.kind)) ||
      a.kind.localeCompare(b.kind),
  );
  const max = Math.max(1, ...kinds.map((k) => k.entities));
  const rows = Math.max(1, Math.ceil(kinds.length / 4));
  const height = 180 + rows * 150;
  const nodes = sorted.map((kind, i) => ({
    kind,
    x: 125 + (i % 4) * 275 + Math.floor(i / 4) * 12,
    y: 190 + Math.floor(i / 4) * 150 - (i % 4) * 14,
    height: kind.entities === 0 ? 4 : 10 + Math.sqrt(kind.entities / max) * 34,
    linked: linked.has(kind.kind),
  }));
  const edges = links.flatMap((link, i) => {
    const from = nodes.find((n) => n.kind.kind === link.from_kind);
    const to = nodes.find((n) => n.kind.kind === link.to_kind);
    if (!from || !to) return [];
    const bend = 28 + (i % 3) * 8;
    return [
      {
        link,
        key: linkKey(link),
        path: `M${from.x},${from.y + 18} C${from.x + bend},${from.y + 65} ${to.x - bend},${to.y + 65} ${to.x},${to.y + 18}`,
        x: (from.x + to.x) / 2,
        y: (from.y + to.y) / 2 + 45,
      },
    ];
  });
  return {
    nodes,
    edges,
    height,
    unlinked: sorted.filter((k) => !linked.has(k.kind)),
  };
}

export function routeLabels(
  layout: ReturnType<typeof overviewLayout>,
  visible: ReturnType<typeof overviewLayout>["edges"],
) {
  const occupied = layout.nodes.flatMap((n) => [
    { x: n.x - 110, y: n.y - n.height - 96, w: 220, h: 43 },
    { x: n.x - 24, y: n.y - n.height - 53, w: 48, h: 40 },
    { x: n.x - 68, y: n.y - n.height - 24, w: 136, h: n.height + 52 },
  ]);
  return visible.map((edge) => {
    const from = layout.nodes.find((n) => n.kind.kind === edge.link.from_kind);
    const to = layout.nodes.find((n) => n.kind.kind === edge.link.to_kind);
    const w = Math.max(100, edge.link.label.length * 7 + 32),
      h = 26;
    let position = { x: edge.x, y: edge.y };
    if (from && to) {
      const bend = 28 + (layout.edges.indexOf(edge) % 3) * 8;
      for (const t of [0.5, 0.35, 0.65, 0.2, 0.8, 0.1, 0.9]) {
        const u = 1 - t;
        const x =
          u * u * u * from.x +
          3 * u * u * t * (from.x + bend) +
          3 * u * t * t * (to.x - bend) +
          t * t * t * to.x;
        const y =
          u * u * u * (from.y + 18) +
          3 * u * u * t * (from.y + 65) +
          3 * u * t * t * (to.y + 65) +
          t * t * t * (to.y + 18);
        const box = { x: x - w / 2, y: y - h / 2, w, h };
        if (
          !occupied.some(
            (r) =>
              box.x < r.x + r.w &&
              box.x + w > r.x &&
              box.y < r.y + r.h &&
              box.y + h > r.y,
          )
        ) {
          position = { x, y };
          break;
        }
      }
    }
    const collides = (x: number, y: number) =>
      occupied.some(
        (r) =>
          x - w / 2 < r.x + r.w &&
          x + w / 2 > r.x &&
          y - h / 2 < r.y + r.h &&
          y + h / 2 > r.y,
      );
    if (collides(position.x, position.y)) {
      const candidates = [];
      for (let y = 36; y < layout.height - 24; y += 36)
        for (let x = 105; x < 1110; x += 85)
          if (!collides(x, y)) candidates.push({ x, y });
      candidates.sort(
        (a, b) =>
          Math.hypot(a.x - edge.x, a.y - edge.y) -
          Math.hypot(b.x - edge.x, b.y - edge.y),
      );
      position = candidates[0] ?? position;
    }
    occupied.push({ x: position.x - w / 2, y: position.y - h / 2, w, h });
    return { ...edge, ...position };
  });
}

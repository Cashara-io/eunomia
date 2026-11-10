import type { DiagramView } from "../../src/components/chat/views/diagram-schema";
export function diagramFixture(
  type: DiagramView["diagram"]["diagram_type"],
): DiagramView {
  const kinds = type === "kinds",
    names = kinds
      ? ["Supplier", "Product", "Order", "Location"]
      : [
          "Banksia Supply",
          "House blend",
          "Order arriving today",
          "Carlton store",
        ];
  const nodes = names.map((name, i) => ({
    id: kinds
      ? ["supplier", "product", "order", "location"][i]
      : `entity:item${i}`,
    name,
    kind: ["supplier", "product", "order", "location"][i],
    focus:
      type === "neighbourhood"
        ? i === 1
        : i === 0 || (type === "path" && i === 3),
    emphasis: i === 3,
    distance: i,
    x: type === "neighbourhood" ? [460, 460, 730, 190][i] : 150 + i * 260,
    y:
      type === "neighbourhood"
        ? [150, 460, 615, 615][i]
        : kinds
          ? 210 - i * 14
          : 160,
    ...(kinds ? { count: [3, 24, 18, 4][i] } : {}),
    metrics: [],
  }));
  const edges = [
    {
      id: "relates_to:a",
      from: nodes[1].id,
      to: nodes[0].id,
      label: "supplied_by",
    },
    {
      id: "relates_to:b",
      from: nodes[1].id,
      to: nodes[2].id,
      label: "included_in",
    },
    {
      id: "relates_to:c",
      from: nodes[2].id,
      to: nodes[3].id,
      label: "delivered_to",
    },
  ].map((e) => ({ ...e, ...(kinds ? { count: 12 } : {}) }));
  return {
    view: "diagram",
    text: `${names[1]} supplied by ${names[0]}. ${names[1]} included in ${names[2]}. ${names[2]} delivered to ${names[3]}.`,
    diagram: {
      diagram_type: type,
      vault: "business",
      title: {
        neighbourhood: "Around House blend",
        path: "Banksia Supply to Carlton store",
        impact: "Links reached from today's delivery",
        kinds: "Business kinds",
      }[type],
      nodes,
      edges,
      bands: type === "impact" ? nodes.map((n) => ({ distance: n.distance, shown: 1, omitted: 0, x: n.x })) : [],
      groups: [],
      total_nodes: 4,
      total_edges: 3,
      omitted_edges: 0,
      partial: false,
      total_partial: false,
      height: 300,
      notice:
        type === "impact"
          ? "3 linked items. Reachability follows links in both directions; it does not establish causality."
          : null,
    },
  };
}

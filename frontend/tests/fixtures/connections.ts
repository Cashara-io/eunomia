import type { Page } from "@playwright/test";
import { mockOntology } from "./ontology-ui";
import profile from "./ontology-ui.json";
export function neighbourhood(size = 4, edgeCount?: number) {
  const kinds = ["supplier", "order", "location", "product"];
  const nodes = [
    { id: "house", name: "House blend 1kg", kind: "product", facts: [] },
    ...Array.from({ length: size }, (_, i) => ({
      id: `n${i}`,
      name:
        ["North Coast Beans", "PO-1042", "Fitzroy store", "Decaf 1kg"][i] ??
        `Product ${i}`,
      kind: size > 10 ? "product" : kinds[i % 4],
      facts: [],
    })),
  ];
  const edges = Array.from({ length: edgeCount ?? size }, (_, i) => ({
    id: `edge${i}`,
    from: i === 0 ? "n0" : "house",
    to: i === 0 ? "house" : `n${i % size}`,
    label:
      i === 0
        ? "supplies"
        : i === 2
          ? "stocked_at"
          : i === 3
            ? "alternative_to"
            : "on_order",
  }));
  return { vault: "business", depth: 1, nodes, edges, truncated: false };
}
export function richNeighbourhood(depth = 1) {
  const data = neighbourhood(0);
  const kinds = [
    "order",
    "supplier",
    "location",
    "product",
    "invoice",
    "shipment",
  ];
  const names: Record<string, string[]> = {
    supplier: ["North Coast Beans", "Otway Roasters", "Merri Packaging"],
    location: ["Fitzroy store", "Carlton store", "Brunswick depot"],
    product: ["Decaf 1kg", "Espresso 250g"],
    invoice: ["INV-2081", "INV-2082"],
    shipment: ["Dispatch 18 Nov", "Dispatch 19 Nov"],
  };
  const verbs: Record<string, string> = {
    supplier: "supplies",
    order: "on_order",
    location: "stocked_at",
    product: "alternative_to",
    invoice: "billed_on",
    shipment: "shipped_in",
  };
  for (const kind of kinds) {
    const count = kind === "order" ? 24 : names[kind].length;
    for (let i = 0; i < count; i++) {
      const id = kind === "supplier" && i === 0 ? "n0" : `${kind}-${i}`;
      data.nodes.push({
        id,
        kind,
        name: kind === "order" ? `PO-${1042 + i}` : names[kind][i],
        facts: [],
      });
      data.edges.push({
        id: `link-${id}`,
        from: kind === "supplier" ? id : "house",
        to: kind === "supplier" ? "house" : id,
        label: verbs[kind],
      });
      if (depth >= 2) {
        const next = `next-${id}`;
        data.nodes.push({
          id: next,
          kind,
          name:
            kind === "order" ? `PO-${2042 + i}` : `${names[kind][i]} reserve`,
          facts: [],
        });
        data.edges.push({
          id: `second-${id}`,
          from: id,
          to: next,
          label: "related_to",
        });
        if (depth >= 3) {
          data.nodes.push({
            id: `third-${id}`,
            kind,
            name: `Archive ${kind} ${i + 1}`,
            facts: [],
          });
          data.edges.push({
            id: `third-link-${id}`,
            from: next,
            to: `third-${id}`,
            label: "related_to",
          });
        }
      }
    }
  }
  return { ...data, depth };
}
export type ConnectionState =
  | "start"
  | "populated"
  | "loading"
  | "empty"
  | "denied"
  | "error"
  | "truncated"
  | "disconnected"
  | "hub"
  | "filtered";
export async function mockConnections(
  page: Page,
  state: ConnectionState = "populated",
  data = state === "hub" ? neighbourhood(149) : richNeighbourhood(),
) {
  await mockOntology(page);
  const requests: Record<string, unknown>[] = [];
  await page.route("**/api/tools/**", async (route) => {
    const args = route.request().postDataJSON();
    requests.push(args);
    if (route.request().url().endsWith("entities_search"))
      return route.fulfill({
        json: {
          results:
            state === "empty"
              ? []
              : [
                  data.nodes[0],
                  ...data.nodes
                    .filter((n) => n.kind !== "order" && n.id !== "house")
                    .slice(0, 3),
                ],
          has_more: false,
        },
      });
    if (state === "loading") return;
    if (state === "denied" || state === "error")
      return route.fulfill({
        status: state === "denied" ? 403 : 503,
        json: {
          error: "Unavailable",
          code: state === "denied" ? "vault.forbidden" : "http.error",
        },
      });
    const loaded =
      args.depth >= 2 && data.nodes.length === 37
        ? richNeighbourhood(args.depth)
        : data;
    return route.fulfill({
      json: {
        ...loaded,
        nodes:
          state === "empty"
            ? []
            : state === "disconnected"
              ? [data.nodes[0]]
              : loaded.nodes,
        edges:
          state === "disconnected" || state === "empty" ? [] : loaded.edges,
        truncated: state === "truncated",
      },
    });
  });
  await page.route("**/ontology/objects/*", (route) => {
    const id = decodeURIComponent(
      new URL(route.request().url()).pathname.split("/").at(-1) ?? "",
    );
    const node = data.nodes.find((n) => n.id === id) ?? data.nodes[0];
    return route.fulfill({ json: { ...profile.profile, ...node } });
  });
  return requests;
}

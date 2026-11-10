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
    label: i === 0 ? "supplies" : i === 2 ? "stocked_at" : "on_order",
  }));
  return { vault: "business", depth: 1, nodes, edges, truncated: false };
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
  data = neighbourhood(state === "hub" ? 149 : 4),
) {
  await mockOntology(page);
  const requests: Record<string, unknown>[] = [];
  await page.route("**/api/tools/**", async (route) => {
    const args = route.request().postDataJSON();
    requests.push(args);
    if (route.request().url().endsWith("entities_search"))
      return route.fulfill({
        json: {
          results: state === "empty" ? [] : [data.nodes[0]],
          has_more: false,
        },
      });
    if (state === "loading") return;
    if (state === "denied" || state === "error")
      return route.fulfill({
        status: state === "denied" ? 404 : 503,
        json: {
          error: "Unavailable",
          code: state === "denied" ? "entity.not_found" : "http.error",
        },
      });
    return route.fulfill({
      json: {
        ...data,
        nodes:
          state === "empty"
            ? []
            : state === "disconnected"
              ? [data.nodes[0]]
              : data.nodes,
        edges: state === "disconnected" || state === "empty" ? [] : data.edges,
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

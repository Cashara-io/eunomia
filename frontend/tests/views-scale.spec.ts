import { test, expect, type Page } from "@playwright/test";
import type { OntologyOut, KindItem } from "../src/lib/gen";
import type { ImpactView, MetricView, ScenarioView } from "../src/components/chat/views/types";

// All APIs and the stream are intercepted. The server itself must also identify
// as this disposable fixture before a test is allowed to navigate or click.
async function fixture(page: Page) {
  const base = test.info().project.use.baseURL;
  expect(base).toBeTruthy();
  if (!base) throw new Error("baseURL must identify a disposable fixture");
  const destination = new URL(base);
  expect(["127.0.0.1", "localhost"]).toContain(destination.hostname);
  expect([3000, 8000, 8001, 3600, 8600, 3700, 18470, 18471, 18472, 18473]).not.toContain(Number(destination.port));
  const proof = await page.request.get("/healthz");
  expect(proof.headers()["x-eunomia-fixture"]).toBe("views-196");
  await page.route("**/api/**", route => {
    const path = new URL(route.request().url()).pathname;
    const json = path === "/api/auth/me" ? { id: "user:fixture", email: "fixture@example.test", onboarded: true, has_password: true, is_admin: false }
      : path === "/api/vaults" ? { results: [{ id: "vault:fixture", name: "Fixture", kind: "personal", role: "owner", is_default: true, created_at: null }] }
      : path.endsWith("/readiness") ? { vault: "vault:fixture", strict: false, ready: false, blockers: [], ...Object.fromEntries(["kinds", "properties", "links", "metrics"].map(k => [k, { approved: 0, proposed: 0, rejected: 0, observed: 0 }])) }
      : path.includes("invitations") ? { results: [] }
      : path.endsWith("/activity") ? { events: [], next_before: null } : [];
    return route.fulfill({ json, headers: { "x-eunomia-fixture": "views-196" } });
  });
}
const start = { id: "entity:incident", name: "Incident", kind: "incident", vault: "vault:fixture" };
const reach = { depth: 1, kinds: ["sample"], labels: ["touches"] };
function impact(page: number): ImpactView {
  const affected = Array.from({ length: Math.min(16, 100 - page * 16) }, (_, i) => {
    const entity = { ...start, id: `entity:item${page * 16 + i}`, name: `Item ${page * 16 + i + 1}`, kind: "sample" };
    return { entity, path: [{ id: `relates_to:r${page * 16 + i}`, from: start, to: entity, label: "touches" }], why: "" };
  });
  return { view: "impact", start, headline: "Linked items", affected, linked: true, text: "Linked items", pagination: { reach, total: 100, page, page_size: 16, has_more: page < 6, truncated: true } };
}
const metric = {
  view: "metric", text: "Metric", entities: [{ ...start, id: "entity:evidence", name: "Evidence item" }],
  metric: { metric: "score", kind: "sample", vault: start.vault, description: null, expr_words: "Sum of score", value: 4567, unit: "AUD", as_of: "2026-11-10", n_entities: 1240, entity_ids: ["entity:evidence"], truncated: true, status: "approved", warnings: [], groups: [{ group: "Department", value: 4567, n_entities: 1240, entity_ids: ["entity:evidence"], truncated: true }] },
} satisfies MetricView;
const scenario = {
  view: "scenario", headline: "A what-if", text: "Nothing saved",
  result: { vault: start.vault, as_of: "2026-11-10", changes: [], baseline: [], scenario: [], changed: [], n_changed: 0, removed: [], reached: [{ entity_id: "entity:neighbour", name: "Other neighbour", kind: "sample", depth: 1, via: "touches", from: start.id, path: [] }], reached_truncated: true, warnings: [] },
} satisfies ScenarioView;
function history(views: object[]) {
  return views.flatMap((view, i) => [
    { role: "assistant", content: "", tool_calls: [{ id: `call_${i}`, type: "function", function: { name: "show_view", arguments: "{}" } }] },
    { role: "tool", tool_call_id: `call_${i}`, name: "show_view", content: JSON.stringify(view) },
  ]);
}
test("impact pages all items, preserves the count and retries failed pages", async ({ page }) => {
  await fixture(page);
  await page.routeWebSocket("**/api/events/ws*", () => {});
  await page.route("**/api/chat/threads", route => route.fulfill({ json: [{ id: "chat_thread:fixture", title: "Views", created_at: new Date().toISOString(), updated_at: new Date().toISOString() }] }));
  await page.route("**/api/chat/threads/*/history", route => route.fulfill({ json: history([impact(0), metric, scenario]) }));
  let requested = -1;
  let fail = true;
  await page.route("**/api/tools/show_view", route => {
    const args = route.request().postDataJSON();
    expect(args.reach).toEqual(reach);
    requested = args.page;
    if (fail) return route.fulfill({ status: 503, json: { detail: "Fixture failure" } });
    return route.fulfill({ json: impact(args.page) });
  });
  await page.goto("/chat");
  const view = page.getByRole("region", { name: "What this affects" });
  await expect(view).toContainText("100 linked items");
  await expect(view).toContainText("Showing 1 to 16");
  await view.getByRole("button", { name: "Next" }).click();
  await expect(view.getByRole("alert")).toBeVisible();
  await expect(view).toContainText("Showing 1 to 16");
  fail = false;
  const seen = new Set<string>();
  for (let p = 0; p < 7; p++) {
    if (p > 0) {
      await view.getByRole("button", { name: "Next" }).click();
      await expect.poll(() => requested).toBe(p);
      await expect(view).toContainText(`Showing ${p * 16 + 1} to ${Math.min(100, (p + 1) * 16)}`);
    }
    for (const name of await view.locator("li > div a").allTextContents()) seen.add(name);
  }
  expect(seen.size).toBe(100);
  await expect(view.getByRole("button", { name: "Next" })).toBeDisabled();
  await view.getByRole("button", { name: "Previous" }).click();
  await expect(view).toContainText("Showing 81 to 96");
  const evidence = page.getByRole("region", { name: "The metric score" });
  await expect(evidence).toContainText("4567.00 AUD");
  await expect(evidence.getByRole("link", { name: "Evidence item" })).not.toBeVisible();
  await evidence.locator("summary").click();
  await expect(evidence.getByRole("link", { name: "Evidence item" })).toBeVisible();
  await expect(evidence).toContainText("and 1239 more");
  await evidence.locator("summary").click();
  await expect(evidence.getByRole("link", { name: "Evidence item" })).not.toBeVisible();
  const neighbours = page.getByRole("link", { name: /Other neighbour/ });
  await expect(neighbours).not.toBeVisible();
  await page.locator("summary").filter({ hasText: "show linked items" }).click();
  await expect(neighbours).toBeVisible();
  await expect(page.getByText("More linked items were in reach than this preview includes.")).toBeVisible();
});

const kind = (status: "proposed" | "approved"): KindItem => ({ kind: "delivery", built_in: false, status, revision: status === "proposed" ? 1 : 2, entities: 0, description: "A scheduled delivery", synonyms: [], samples: [], properties: [], metrics: [] });
test("existing ontology stream refreshes Structure, Overview and kind definitions in one debounced batch", async ({ page }) => {
  await fixture(page);
  let definition: KindItem | null = null;
  let reads = 0;
  await page.route("**/api/vaults/*/ontology", route => {
    reads++;
    const body = { vault: start.vault, strict: false, kinds: definition ? [definition] : [], links: [] } satisfies OntologyOut;
    return route.fulfill({ json: body });
  });
  let send: ((value: object) => void) | undefined;
  await page.routeWebSocket("**/api/events/ws*", ws => {
    send = value => ws.send(JSON.stringify(value));
    send({ type: "hello", vaults: { [start.vault]: 0 }, ontology: { [start.vault]: 0 } });
  });
  await page.goto(`/ontology/schema?space=${encodeURIComponent(start.vault)}`);
  await expect(page.getByRole("heading", { name: "Structure: List" })).toBeVisible();
  // Drain the initial hello invalidation, then deliver a burst of agent proposals.
  await page.waitForTimeout(700);
  const before = reads;
  definition = kind("proposed");
  for (let seq = 1; seq <= 10; seq++) send?.({ type: "ontology", vault: start.vault, event: { seq } });
  await expect(page.getByRole("row").filter({ hasText: "delivery" })).toContainText("Suggested");
  expect(reads - before).toBe(1);
  definition = kind("approved");
  send?.({ type: "ontology", vault: start.vault, event: { seq: 11 } });
  await expect(page.getByRole("row").filter({ hasText: "delivery" })).toContainText("Approved");
  await page.goto(`/ontology?space=${encodeURIComponent(start.vault)}`);
  await expect(page.getByRole("heading", { name: "Fixture: Overview" })).toBeVisible();
  definition = kind("proposed");
  send?.({ type: "ontology", vault: start.vault, event: { seq: 12 } });
  await expect(page.getByRole("link", { name: "delivery", exact: true }).first()).toBeVisible();
  await page.goto(`/ontology/kinds/delivery?space=${encodeURIComponent(start.vault)}`);
  await expect(page.getByText("A scheduled delivery", { exact: true }).first()).toBeVisible();
  definition = { ...kind("approved"), description: "A confirmed delivery" };
  send?.({ type: "ontology", vault: start.vault, event: { seq: 13 } });
  await expect(page.getByText("A confirmed delivery", { exact: true }).first()).toBeVisible();
});

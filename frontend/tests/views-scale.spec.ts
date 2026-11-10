import { test, expect, type Page } from "@playwright/test";
import type { OntologyOut, KindItem } from "../src/lib/gen";
import type { ImpactView, MetricView, ScenarioView } from "../src/components/chat/views/types";

// All APIs and the stream are intercepted. Local fixture runs can also require
// a response header. CI runs against its own disposable compose app.
async function fixture(page: Page) {
  const base = test.info().project.use.baseURL;
  expect(base).toBeTruthy();
  if (!base) throw new Error("baseURL must identify a disposable fixture");
  if (process.env.EUNOMIA_VIEWS_FIXTURE === "1") {
    const proof = await page.request.get("/healthz");
    expect(proof.headers()["x-eunomia-fixture"]).toBe("views-196");
  }
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
  return { view: "impact", start, headline: "Linked items", affected, linked: true, text: "Linked items", pagination: { reach, total: 100, page, page_size: 16, has_more: page < 6, truncated: true, total_partial: false, next_cursor: page < 6 ? `impact-${page + 1}` : null } };
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
    requested = Number(args.cursor.replace("impact-", ""));
    if (fail) return route.fulfill({ status: 503, json: { detail: "Fixture failure" } });
    return route.fulfill({ json: impact(requested) });
  });
  await page.goto("/chat");
  const view = page.getByRole("region", { name: "What this affects" });
  await expect(view).toContainText("100 linked items");
  await expect(view).toContainText("Showing 16 items on page 1");
  await view.getByRole("button", { name: "Next" }).click();
  await expect(view.getByRole("alert")).toBeVisible();
  await expect(view).toContainText("Showing 16 items on page 1");
  fail = false;
  const seen = new Set<string>();
  for (let p = 0; p < 7; p++) {
    if (p > 0) {
      await view.getByRole("button", { name: "Next" }).click();
      await expect.poll(() => requested).toBe(p);
      await expect(view).toContainText(`Showing ${Math.min(16, 100 - p * 16)} items on page ${p + 1}`);
    }
    for (const name of await view.locator("li > div a").allTextContents()) seen.add(name);
  }
  expect(seen.size).toBe(100);
  await expect(view.getByRole("button", { name: "Next" })).toBeDisabled();
  await view.getByRole("button", { name: "Previous" }).click();
  await expect(view).toContainText("Showing 16 items on page 6");
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
  let documentLoads = 0;
  page.on("request", request => {
    if (request.isNavigationRequest() && request.frame() === page.mainFrame()) documentLoads++;
  });
  await page.route("**/api/vaults/*/ontology", route => {
    reads++;
    const body = { vault: start.vault, strict: false, kinds: definition ? [definition] : [], links: [] } satisfies OntologyOut;
    return route.fulfill({ json: body });
  });
  await page.route("**/api/vaults/*/objects?*", route => route.fulfill({
    json: { rows: [], total: 0, offset: 0, limit: 20, columns: [], kind_approved: true },
  }));
  let send: ((value: object) => void) | undefined;
  await page.routeWebSocket("**/api/events/ws*", ws => {
    send = value => ws.send(JSON.stringify(value));
    send({ type: "hello", vaults: { [start.vault]: 0 }, ontology: { [start.vault]: 0 } });
  });
  await page.goto(`/ontology/schema?space=${encodeURIComponent(start.vault)}`);
  await expect(page.getByRole("heading", { name: "Structure: List" })).toBeVisible();
  // Drain the initial hello invalidation, then deliver a burst of agent proposals.
  await page.waitForTimeout(700);
  const initialDocumentLoads = documentLoads;
  let sequence = 0;
  const refresh = async (assertUpdated: () => Promise<void>) => {
    const before = reads;
    if (!send) throw new Error("The ontology stream must be connected");
    for (let i = 0; i < 10; i++) send({ type: "ontology", vault: start.vault, event: { seq: ++sequence } });
    await assertUpdated();
    // Catch extra invalidations after the first response, not just the first visible update.
    await page.waitForTimeout(700);
    expect(reads - before).toBe(1);
    expect(documentLoads).toBe(initialDocumentLoads);
  };
  definition = kind("proposed");
  await refresh(() => expect(page.getByRole("row").filter({ hasText: "delivery" })).toContainText("Suggested"));
  definition = { ...kind("approved"), entities: 3 };
  await refresh(() => expect(page.getByRole("row").filter({ hasText: "delivery" })).toContainText("Approved"));
  await page.getByRole("link", { name: "Overview", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Your business, connected." })).toBeVisible();
  await page.getByRole("button", { name: "Show table", exact: true }).click();
  const delivery = page.getByRole("table", { name: "The same kinds as the landscape" })
    .getByRole("row").filter({ has: page.getByRole("button", { name: "Deliveries", exact: true }) });
  await expect(delivery.getByRole("cell").first()).toHaveText("3");
  await page.waitForTimeout(700);
  definition = { ...kind("approved"), entities: 7 };
  await refresh(() => expect(delivery.getByRole("cell").first()).toHaveText("7"));
  await page.locator("summary").filter({ hasText: "More" }).click();
  await page.getByRole("link", { name: "Structure", exact: true }).click();
  await page.getByRole("table", { name: "Kinds and link types" }).getByRole("link", { name: "delivery", exact: true }).click();
  await expect(page.getByText("A scheduled delivery", { exact: true }).first()).toBeVisible();
  await page.waitForTimeout(700);
  definition = { ...kind("approved"), entities: 7, description: "A confirmed delivery" };
  await refresh(() => expect(page.getByText("A confirmed delivery", { exact: true }).first()).toBeVisible());
});


test("metric evidence can reach all 1240 contributors and retry a failed page", async ({ page }) => {
  await fixture(page);
  await page.routeWebSocket("**/api/events/ws*", () => {});
  await page.route("**/api/chat/threads", route => route.fulfill({ json: [{ id: "chat_thread:fixture", title: "Evidence", created_at: new Date().toISOString(), updated_at: new Date().toISOString() }] }));
  const evidencePage = (offset: number) => ({
    ...metric,
    entities: Array.from({ length: Math.min(50, 1240 - offset) }, (_, i) => ({ ...start, id: `entity:evidence${offset + i}`, name: `Evidence ${offset + i}` })),
    evidence: { args: { metric: "score", vault_id: start.vault, as_of: metric.metric.as_of }, next_cursor: offset + 50 < 1240 ? `evidence-${offset + 50}` : null },
  });
  await page.route("**/api/chat/threads/*/history", route => route.fulfill({ json: history([evidencePage(0)]) }));
  let fail = true;
  const requested: string[] = [];
  await page.route("**/api/tools/show_view", route => {
    const args = route.request().postDataJSON();
    expect(args.view).toBe("metric");
    expect(args.metric).toBe("score");
    expect(args.as_of).toBe(metric.metric.as_of);
    requested.push(args.evidence_cursor);
    if (fail) return route.fulfill({ status: 503, json: { detail: "Fixture failure" } });
    return route.fulfill({ json: evidencePage(Number(args.evidence_cursor.replace("evidence-", ""))) });
  });
  await page.goto("/chat");
  const view = page.getByRole("region", { name: "The metric score" });
  await view.locator("summary").click();
  await expect(view.getByRole("link", { name: /^Evidence / })).toHaveCount(50);
  await view.getByRole("button", { name: "Show more evidence" }).click();
  await expect(view.getByRole("alert")).toBeVisible();
  await expect(view.getByRole("link", { name: /^Evidence / })).toHaveCount(50);
  fail = false;
  for (let offset = 50; offset < 1240; offset += 50) {
    await view.getByRole("button", { name: "Show more evidence" }).click();
    await expect(view.getByRole("link", { name: /^Evidence / })).toHaveCount(Math.min(offset + 50, 1240));
  }
  expect(new Set(await view.getByRole("link", { name: /^Evidence / }).evaluateAll(links => links.map(link => link.getAttribute("href")))).size).toBe(1240);
  expect(requested.slice(0, 2)).toEqual(["evidence-50", "evidence-50"]);
  await expect(view.getByRole("button", { name: "Show more evidence" })).toHaveCount(0);
  await expect(view).toContainText("Based on 1,240 items");
  await expect(view).toContainText("4567.00 AUD");
});


test("impact traversal limits display a lower bound", async ({ page }) => {
  await fixture(page);
  await page.routeWebSocket("**/api/events/ws*", () => {});
  await page.route("**/api/chat/threads", route => route.fulfill({ json: [{ id: "chat_thread:fixture", title: "Partial", created_at: new Date().toISOString(), updated_at: new Date().toISOString() }] }));
  const partial = impact(0);
  if (!partial.pagination) throw new Error("fixture needs pagination");
  partial.pagination.total_partial = true;
  await page.route("**/api/chat/threads/*/history", route => route.fulfill({ json: history([partial]) }));
  await page.goto("/chat");
  const view = page.getByRole("region", { name: "What this affects" });
  await expect(view).toContainText("At least 100 linked items");
  await expect(view).toContainText("Traversal limit reached");
});

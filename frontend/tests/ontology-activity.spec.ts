import { test, expect } from "@playwright/test";
import { registerAndOnboard, uniqueEmail } from "./helpers";

test.use({ viewport: { width: 1440, height: 1000 } });

test("two contexts see proposals, durable reasons and stale-review conflicts live", async ({ browser, page }, testInfo) => {
  test.setTimeout(120000);
  await registerAndOnboard(page, uniqueEmail("ontology-live"));
  const vaultResponse = await page.request.post("/api/vaults", { data: { name: "Live proposal proof" } });
  expect(vaultResponse.ok()).toBeTruthy();
  const vault = await vaultResponse.json();
  const tokenResponse = await page.request.post("/api/auth/tokens", { data: { name: "Mapping app", vault_id: vault.id } });
  const { token } = await tokenResponse.json();
  const reviewer = await browser.newContext({ storageState: await page.context().storageState() });
  const review = await reviewer.newPage();
  const path = `/api/vaults/${encodeURIComponent(vault.id)}/ontology`;
  const reviewURL = `/ontology/review?space=${encodeURIComponent(vault.id)}`;
  const activityURL = `/ontology/activity?space=${encodeURIComponent(vault.id)}`;
  await page.goto(activityURL);
  await review.goto(reviewURL);
  await expect(review.getByText("Nothing is waiting for review.", { exact: false }).first()).toBeVisible();
  async function propose(description: string, kind = "delivery") {
    const ontology = await (await page.request.get(path)).json();
    const revision = ontology.kinds.find((item: { kind: string; revision: number }) => item.kind === kind)?.revision ?? 0;
    const response = await page.request.post("/mcp", { headers: { Authorization: `Bearer ${token}`, Accept: "application/json, text/event-stream" }, data: {
      jsonrpc: "2.0", id: Date.now(), method: "tools/call", params: { name: "ontology_propose", arguments: { vault_id: vault.id, kinds: [{ kind, revision, description, reason: "Matched deliveries.csv" }] } },
    } });
    expect(response.ok()).toBeTruthy();
    expect((await response.json()).result.isError).toBeFalsy();
  }
  await review.screenshot({ path: testInfo.outputPath("landing-before.png") });
  await propose("A shipment scheduled for a store");
  const notice = review.getByRole("button", { name: "1 new suggestions. Show in queue" });
  await expect(notice).toBeVisible({ timeout: 15000 });
  await review.screenshot({ path: testInfo.outputPath("landing-notice.png") });
  await notice.click();
  await review.getByRole("link", { name: /delivery.*kind/ }).click();
  await expect(review.getByText("A shipment scheduled for a store", { exact: true }).first()).toBeVisible();
  await review.screenshot({ path: testInfo.outputPath("landing-open.png") });
  // A changes the definition after B has opened revision 1. The open review stays pinned.
  await propose("A delivery with a confirmed destination and date");
  await expect(page.getByText(/proposed delivery/i)).toHaveCount(2);
  const proposals = await (await page.request.get(`${path}/activity?action=proposed`)).json();
  expect(proposals.events).toHaveLength(2);
  expect(proposals.events[0].reason).toBeNull();
  expect(proposals.events[1].reason).toBeNull();
  await review.getByLabel("Reason for decision (optional)").fill("Matches the source records");
  await review.getByRole("button", { name: "Approve", exact: true }).click();
  await expect(review.getByRole("heading", { name: "This definition changed" })).toBeVisible();
  await expect(review.getByText("Latest: proposed, A delivery with a confirmed destination and date.")).toBeVisible();
  await review.screenshot({ path: testInfo.outputPath("stale-conflict.png") });
  await review.getByRole("button", { name: "Review latest version" }).click();
  await expect(review.getByLabel("Reason for decision (optional)")).toHaveValue("Matches the source records");
  await review.getByRole("button", { name: "Approve", exact: true }).click();
  await expect(page.getByText("Matches the source records", { exact: true })).toBeVisible({ timeout: 15000 });
  const stored = await (await page.request.get(`${path}/activity?action=approved`)).json();
  expect(stored.events).toHaveLength(1);
  expect(stored.events[0].reason).toBe("Matches the source records");
  expect(stored.events[0].actor_type).toBe("user");
  // Screenshot the real data in both themes and responsive widths.
  for (const width of [1440, 390]) for (const theme of ["light", "dark"]) {
    await page.setViewportSize({ width, height: 1000 });
    await page.evaluate(value => { localStorage.setItem("eunomia-theme", value); document.documentElement.dataset.theme = value; }, theme);
    await page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
    await page.screenshot({ path: testInfo.outputPath(`activity-${width}-${theme}.png`), fullPage: true, animations: "disabled" });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBeTruthy();
  }
  // Batch decisions keep the selected revision and offer the changed evidence for re-review.
  await review.goto(reviewURL);
  await expect(review.getByText("Nothing is waiting for review.", { exact: false }).first()).toBeVisible();
  await propose("Original batch route", "delivery_route");
  await review.getByRole("button", { name: "1 new suggestions. Show in queue" }).click();
  await review.getByLabel("Select delivery route", { exact: true }).check();
  await review.getByLabel("Reason for this batch (optional)").fill("Matches route records");
  await propose("Route description changed", "delivery_route");
  await review.getByRole("button", { name: "Approve 1 selected", exact: true }).click();
  await review.getByRole("button", { name: "Approve selected", exact: true }).click();
  await expect(review.getByText("You reviewed: proposed, Original batch route.")).toBeVisible();
  await expect(review.getByText("Latest: proposed, Route description changed.")).toBeVisible();
  await review.screenshot({ path: testInfo.outputPath("batch-stale-conflict.png"), fullPage: true });
  await review.getByRole("button", { name: "Review changes for Kind delivery route" }).click();
  await expect(review.getByLabel("Reason for decision (optional)")).toHaveValue("Matches route records");
  const retryDecision = review.waitForResponse(response => response.request().method() === "POST" && response.url().endsWith("/ontology/kinds/decide"));
  await review.getByRole("button", { name: "Approve", exact: true }).click();
  expect((await retryDecision).ok()).toBeTruthy();
  const retried = await (await page.request.get(`${path}/activity?action=approved&kind=delivery_route`)).json();
  expect(retried.events).toHaveLength(1);
  expect(retried.events[0].reason).toBe("Matches route records");
  await reviewer.close();
});

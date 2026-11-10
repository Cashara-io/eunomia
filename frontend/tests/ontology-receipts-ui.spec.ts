import { test, expect } from "@playwright/test";
import { mockReceipts } from "./fixtures/receipts";
const profile = "/ontology/objects/house?space=business&section=Properties";
for (const width of [1440, 390]) test(`three sources, six states, exact rows and focus at ${width}`, async ({ page }) => {
  await page.setViewportSize({ width, height: 900 });
  const { state, requests } = await mockReceipts(page);
  await page.goto(profile);
  const receipts = page.getByRole("region", { name: "Properties", exact: true });
  for (const label of ["Recorded", "Zero", "Missing", "Stale", "Conflicting", "Source unavailable"]) await expect(receipts).toContainText(label);
  await expect(page.getByText("Verified", { exact: true })).toHaveCount(0);
  for (const [value, content] of [["Unit cost", "products.csv, row 42"], ["Discount", "products.csv, row 9"], ["Stock", "third source only"], ["Freight", "This source is unavailable"]]) {
    const button = page.getByRole("button", { name: `Inspect source for ${value}`, exact: true });
    await button.click();
    const inspector = width === 390 ? page.getByRole("dialog", { name: "Source inspector" }) : page.getByRole("complementary", { name: "Source inspector" });
    await expect(inspector).toContainText(content);
    await expect(inspector).toContainText("House blend 1kg");
    if (value === "Unit cost") {
      await expect(inspector).toContainText("AUD 12.00");
      if (width === 390) {
        await expect(inspector).toHaveJSProperty("open", true);
        for (let tab = 0; tab < 8; tab++) {
          await page.keyboard.press(tab % 2 ? "Shift+Tab" : "Tab");
          expect(await inspector.evaluate(el => el.contains(document.activeElement))).toBeTruthy();
        }
      } else await expect(receipts).toContainText("AUD 12.00");
    }
    await expect(page.getByText("PROTECTED CROSS SPACE", { exact: true })).toHaveCount(0);
    await page.goBack();
    await expect(inspector).toHaveCount(0);
    await expect(button).toBeFocused();
  }
  expect(state.profileReads).toBe(1);
  expect(requests.filter(p => /metric.*run|try_metric/.test(p))).toEqual([]);
});
test("Escape closes the mobile sheet and returns focus", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await mockReceipts(page); await page.goto(profile);
  const button = page.getByRole("button", { name: "Inspect source for Unit cost" });
  await button.click(); await expect(page.getByRole("dialog")).toBeVisible();
  await page.keyboard.press("Escape"); await expect(page.getByRole("dialog")).toHaveCount(0); await expect(button).toBeFocused();
});
test("table query, selection, horizontal scroll and profile section survive return", async ({ page }) => {
  await mockReceipts(page);
  const tableUrl = "/ontology/objects?space=business&kind=product&search=House&selected=house&section=Properties";
  await page.goto(tableUrl);
  await expect(page.getByRole("link", { name: "Open object page" })).toBeVisible();
  await page.addStyleTag({ content: '[aria-label="Objects table"] table { min-width: 1800px; min-height: 1200px; }' });
  await page.getByRole("region", { name: "Objects table" }).evaluate(el => { el.scrollLeft = 80; });
  const x = await page.getByRole("region", { name: "Objects table" }).evaluate(el => el.scrollLeft);
  expect(x).toBe(80);
  await page.getByRole("link", { name: "House blend 1kg", exact: true }).click();
  await expect(page.getByRole("region", { name: "Properties", exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Inspect source for Unit cost" }).click();
  await page.goBack();
  await page.getByRole("link", { name: "Back to Objects", exact: true }).click();
  await expect(page).toHaveURL(new RegExp("search=House&selected=house&section=Properties"));
  await expect(page.getByLabel("Search objects")).toHaveValue("House");
  await expect(page.getByRole("link", { name: "Open object page" })).toBeVisible();
  await expect.poll(() => page.getByRole("region", { name: "Objects table" }).evaluate(el => el.scrollLeft)).toBe(80);
});
for (const membership of ["pending", "denied", "error"]) test(`membership ${membership} clears cached receipt and source`, async ({ page }) => {
  const { state, revoke } = await mockReceipts(page); await page.goto(profile);
  await page.getByRole("button", { name: "Inspect source for Unit cost" }).click();
  await expect(page.getByRole("complementary", { name: "Source inspector" })).toBeVisible();
  state.membership = membership; revoke();
  await expect(page.getByRole("heading", { name: "House blend 1kg", exact: true })).toHaveCount(0);
  await expect(page.getByRole("complementary", { name: "Source inspector" })).toHaveCount(0);
  await expect(page.getByText("products.csv, row 42", { exact: true })).toHaveCount(0);
});
test("entity.not_found confirms membership with protected content hidden", async ({ page }) => {
  const { state } = await mockReceipts(page); await page.goto(profile);
  await expect(page.getByRole("heading", { name: "House blend 1kg", exact: true })).toBeVisible();
  state.mode = "missing"; state.membership = "pending";
  await page.getByRole("button", { name: "Refresh ontology" }).click();
  await expect(page.getByText("Checking space access…", { exact: true })).toBeVisible();
  await expect(page.getByRole("heading", { name: "House blend 1kg", exact: true })).toHaveCount(0);
});
test("activity exact definition, unavailable history, pause and scroll survive inspection", async ({ page }) => {
  await mockReceipts(page); await page.goto("/ontology/activity?space=business");
  const row = page.locator("li").filter({ hasText: "Unit cost" }).filter({ has: page.locator("time") }).first();
  await expect(row).toBeVisible();
  await page.getByRole("button", { name: "Pause view", exact: true }).click();
  const link = row.getByRole("link").first();
  await link.click();
  const subject = page.getByRole("complementary", { name: "Activity subject" });
  await expect(subject).toContainText("Unit cost");
  await expect(subject.getByRole("link", { name: "Open current definition" })).toHaveAttribute("href", /selected=.*property.*unit_cost/);
  await page.getByRole("button", { name: "Back to activity" }).click();
  await expect(page.getByText("View paused", { exact: true })).toBeVisible();
  await expect(link).toBeFocused();
});

test("historical activity is labelled and an object event opens the exact object", async ({ page }) => {
  const { state } = await mockReceipts(page);
  await page.route("**/api/vaults/business/ontology/activity?*", route => route.fulfill({ json: { events: [
    { seq: 2, at: "2026-11-10T03:32:00Z", actor: "owner", actor_name: "Alex", actor_type: "user", action: "approved", definition_type: "property", kind: "product", name: "unit_cost", target: "property:cost", revision: 99, reason: null },
    { seq: 1, at: "2026-11-10T03:32:00Z", actor: "owner", actor_name: "Alex", actor_type: "user", action: "changed", definition_type: "object", kind: "product", name: "House blend 1kg", target: "entity:house", revision: 1, reason: null }
  ], next: 1, more: false } }));
  await page.goto("/ontology/activity?space=business");
  await page.getByRole("link", { name: "Unit cost", exact: true }).click();
  await expect(page.getByRole("complementary", { name: "Activity subject" })).toContainText("Historical version unavailable");
  await page.goBack();
  await page.getByRole("link", { name: "House blend 1kg", exact: true }).click();
  await expect(page.getByRole("complementary", { name: "Activity subject" })).toContainText("AUD 12.00");
  expect(state.profileReads).toBe(1);
});
test("directory does not read profiles per row, source text is escaped and desktop stays non-modal", async ({ page }) => {
  const { state } = await mockReceipts(page);
  state.profile.sources[0].document!.row!.cells.unit_cost = '<img src=x onerror="window.receiptInjected=true">';
  await page.goto("/ontology/objects?space=business");
  await expect(page.getByRole("table")).toBeVisible(); expect(state.profileReads).toBe(0);
  await page.getByRole("link", { name: "House blend 1kg", exact: true }).click();
  await page.getByRole("button", { name: "Inspect source for Unit cost" }).click();
  const inspector = page.getByRole("complementary", { name: "Source inspector" });
  await expect(inspector).toContainText('<img src=x');
  await expect(inspector.locator("img")).toHaveCount(0);
  await page.getByRole("button", { name: "Properties", exact: true }).focus();
  await expect(page.getByRole("button", { name: "Properties", exact: true })).toBeFocused();
  expect(state.profileReads).toBe(1);
});

test("a chosen profile section is restored after returning through the table", async ({ page }) => {
  await mockReceipts(page); await page.goto("/ontology/objects?space=business&search=House");
  await page.getByRole("link", { name: "House blend 1kg", exact: true }).click();
  await page.getByRole("button", { name: "Numbers", exact: true }).click();
  await page.getByRole("link", { name: "Back to Objects", exact: true }).click();
  await page.getByRole("link", { name: "House blend 1kg", exact: true }).click();
  await expect(page.getByRole("region", { name: "Numbers", exact: true })).toBeVisible();
});

test("a scrolled paused activity feed keeps its exact position after subject inspection", async ({ page }) => {
  await mockReceipts(page);
  await page.route("**/api/vaults/business/ontology/activity?*", route => route.fulfill({ json: { events: Array.from({ length: 30 }, (_, i) => ({ seq: 30 - i, at: "2026-11-10T03:32:00Z", actor: "owner", actor_name: "Alex", actor_type: "user", action: "approved", definition_type: "property", kind: "product", name: "unit_cost", target: "property:cost", revision: 1, reason: `Change ${i}` })), next: 1, more: false } }));
  await page.goto("/ontology/activity?space=business");
  const feed = page.locator('[class*="activityRows"]');
  await expect(feed.getByRole("link")).toHaveCount(30);
  expect(await page.evaluate(() => document.documentElement.scrollHeight)).toBeLessThan(1200);
  await feed.evaluate(el => { el.scrollTop = 180; });
  await expect(page.getByText("View paused", { exact: true })).toBeVisible();
  await feed.getByRole("link").nth(4).click();
  const y = await feed.evaluate(el => el.scrollTop);
  expect(y).toBeGreaterThan(0);
  await page.getByRole("button", { name: "Back to activity" }).click();
  await expect.poll(() => feed.evaluate(el => el.scrollTop)).toBe(y);
});

for (const width of [1440, 390, 320]) for (const theme of ["light", "dark"]) test(`receipt and source reflow at 200% text, ${width} ${theme}`, async ({ page }) => {
  await page.setViewportSize({ width, height: 900 });
  await page.addInitScript(theme => localStorage.setItem("eunomia-theme", theme), theme);
  await mockReceipts(page); await page.goto(profile);
  await page.addStyleTag({ content: "html { font-size: 200% !important; }" });
  await page.getByRole("button", { name: "Inspect source for Unit cost" }).click();
  const inspector = page.getByRole(width <= 700 ? "dialog" : "complementary", { name: "Source inspector" });
  await expect(inspector).toContainText("products.csv, row 42");
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
  const order = await inspector.locator("h3, dl").evaluateAll(elements => elements.map(el => el.tagName));
  expect(order).toEqual(["H3", "DL"]);
});

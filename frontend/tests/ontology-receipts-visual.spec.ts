import { test, expect } from "@playwright/test";
import { mkdir, writeFile } from "node:fs/promises";
import { mockReceipts } from "./fixtures/receipts";
const output = process.env.ONTOLOGY_RECEIPT_SCREENSHOTS;
const states = ["populated", "source", "mismatch", "mismatch-source", "compact-source", "duplicate", "record", "unavailable", "loading", "empty", "error", "denied", "missing", "links", "numbers", "activity", "subject", "historical", "paused", "objects", "zoom", "summary", "timeline", "sources", "memories"];
for (const state of states) test(`receipt visual ${state}`, async ({ browser }) => {
  test.skip(!output, "Set ONTOLOGY_RECEIPT_SCREENSHOTS to capture paired evidence");
  test.setTimeout(90000);
  await mkdir(output!, { recursive: true });
  const pairs = [];
  for (const width of state === "compact-source" ? [1440] : [1440, 390]) for (const theme of ["light", "dark"]) {
    const context = await browser.newContext({ viewport: { width, height: width === 390 ? 844 : 900 }, locale: "en-AU", timezoneId: "Australia/Melbourne", reducedMotion: "reduce", hasTouch: width === 390 });
    const page = await context.newPage();
    await page.addInitScript(theme => localStorage.setItem("eunomia-theme", theme), theme);
    const { state: api } = await mockReceipts(page);
    if (["loading", "error", "denied", "missing"].includes(state)) api.mode = state;
    if (state.startsWith("mismatch")) api.profile.sources[0].document!.row!.cells.unit_cost = "99.00";
    if (state === "empty") { api.profile.properties = []; api.profile.sources = []; api.profile.summary = ""; }
    if (state === "historical") await page.route("**/api/vaults/business/ontology/activity?*", route => route.fulfill({ json: { events: [{ seq: 1, at: "2026-11-10T03:32:00Z", actor: "owner", actor_name: "Alex", actor_type: "user", action: "approved", definition_type: "property", kind: "product", name: "unit_cost", target: "property:cost", revision: 99, reason: "Cost definition reviewed" }], next: 1, more: false } }));
    const activity = ["activity", "subject", "historical", "paused"].includes(state);
    await page.goto(`${activity ? "/ontology/activity" : ["objects", "compact-source"].includes(state) ? "/ontology/objects" : "/ontology/objects/house"}?space=business&section=${state === "links" ? "Links" : state === "numbers" ? "Numbers" : state === "summary" ? "Summary" : state === "timeline" ? "Timeline" : state === "sources" ? "Sources" : state === "memories" ? "Related memories" : "Properties"}`);
    if (state === "compact-source") {
      const url = new URL(page.url()); url.searchParams.set("kind", "product"); url.searchParams.set("page", "2"); url.searchParams.set("selected", "house");
      await page.goto(url.pathname + url.search);
    }
    if (state === "missing") await expect(page.getByRole("region", { name: "Object unavailable" })).toBeVisible();
    else if (["error", "denied"].includes(state)) await expect(page.locator("main").getByRole("alert").first()).toBeVisible({ timeout: 15000 });
    else if (state === "loading") await expect(page.getByRole("status").filter({ hasText: "Loading object receipt" })).toBeVisible();
    else if (activity) {
      await expect(page.getByRole("button", { name: "Pause view", exact: true })).toBeVisible();
      if (state !== "activity") await page.getByRole("button", { name: "Pause view", exact: true }).click();
      if (["subject", "historical"].includes(state)) { await page.locator("main li").filter({ hasText: "Unit cost" }).getByRole("link").first().click(); await expect(page.getByRole("complementary", { name: "Activity subject" })).toBeVisible(); if (state === "historical") await expect(page.getByRole("complementary", { name: "Activity subject" })).toContainText("Historical version unavailable"); }
    } else if (state === "objects") await expect(page.getByRole("table")).toBeVisible();
    else await expect(page.getByRole("heading", { name: "House blend 1kg", exact: true })).toBeVisible();
    const value = new Map([["source", "Unit cost"], ["compact-source", "Unit cost"], ["mismatch-source", "Unit cost"], ["duplicate", "Discount"], ["record", "Stock"], ["unavailable", "Freight"]]).get(state);
    if (value) { await page.getByRole("button", { name: `Inspect source for ${value}` }).click(); await expect(page.getByRole(width === 390 ? "dialog" : "complementary", { name: "Source inspector" })).toBeVisible(); }
    if (state === "zoom") await page.addStyleTag({ content: "html { font-size: 200% !important; }" });
    await page.evaluate(() => document.fonts.ready);
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
    const name = `${state}-${width}-${theme}.png`;
    await page.screenshot({ path: `${output}/${name}`, fullPage: true });
    if (value) await page.screenshot({ path: `${output}/${state}-viewport-${width}-${theme}.png` });
    const before = `${activity ? state === "paused" ? "activity-paused" : "activity-populated" : state === "objects" ? "objects-populated" : ["loading", "empty", "error", "denied"].includes(state) ? `profile-${state === "denied" ? "error" : state}` : state === "numbers" ? "profile-numbers" : state === "summary" ? "profile-populated" : state === "timeline" ? "profile-timeline" : state === "sources" ? "profile-sources" : state === "memories" ? "profile-related-memories" : state === "links" ? "profile-links" : value ? "profile-sources" : "profile-properties"}-${width}-${theme}.png`;
    pairs.push(`| ${state}, ${width}, ${theme} | [Before](../../before/${before}) | [After](${name}) |`);
    await context.close();
  }
  await writeFile(`${output}/${state}-pairs.md`, pairs.join("\n"));
});

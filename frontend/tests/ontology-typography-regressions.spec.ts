import { test, expect } from "@playwright/test";
import { mockConnections, neighbourhood } from "./fixtures/connections";
import { mockOntology } from "./fixtures/ontology-ui";
import { mockReview, at } from "./fixtures/ontology-review";
import type { MetricRun } from "../src/components/chat/views/types";

for (const rootSize of ["100%", "200%"])
  test(`Strata kind names scale with the root size at ${rootSize}`, async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await mockOntology(page);
    await page.goto("/ontology?space=business");
    const tile = page.locator("[data-wire^='kind:']").first();
    await expect(tile).toBeVisible();
    await page.evaluate((size) => { document.documentElement.style.fontSize = size; }, rootSize);
    const px = await tile.evaluate((el) => parseFloat(getComputedStyle(el).fontSize));
    expect(px).toBeGreaterThanOrEqual(rootSize === "200%" ? 26 : 13);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(true);
  });

test("a kind metric Try it result uses the display scale", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.clock.setFixedTime(new Date("2026-11-11T00:00:00Z"));
  await page.addInitScript(() => localStorage.setItem("eunomia-theme", "light"));
  await mockReview(page);
  const result = {
    metric: "gross_profit", kind: "stock_item", vault: "business",
    description: "Gross profit across stock items.", expr_words: "Sum of price",
    value: 1234.5, unit: "AUD", as_of: at, n_entities: 6,
    entity_ids: [], truncated: false, status: "proposed", warnings: [],
  } satisfies MetricRun;
  await page.route("**/api/tools/metric_eval", route => route.fulfill({ json: result }));
  await page.goto("/ontology/kinds/stock_item?space=business&section=metrics");
  await page.locator("summary").filter({ hasText: /^gross profit$/i }).click();
  await page.getByText("Try calculation", { exact: true }).click();
  await page.getByRole("button", { name: "Try it", exact: true }).click();
  const number = page.getByRole("status").getByText("1234.50", { exact: true });
  await expect(number).toBeVisible();
  const output = process.env.ONTOLOGY_TYPE_SCALE_SCREENSHOTS;
  if (output) await page.screenshot({ path: `${output}/metric-result-1440-light.png`, fullPage: true });
  await expect(number).toHaveCSS("font-size", "24px");
});

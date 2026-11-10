import { test, expect } from "@playwright/test";
import { mockConnections, neighbourhood } from "./fixtures/connections";
import { mockOntology } from "./fixtures/ontology-ui";
import { mockReview, at } from "./fixtures/ontology-review";
import type { MetricRun } from "../src/components/chat/views/types";

for (const rootSize of ["100%", "200%"]) {
  test(`connection labels fit their reserved plates at root size ${rootSize}`, async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await mockConnections(page, "populated", neighbourhood(4));
    await page.goto("/ontology/connections?space=business&focus=house");
    await expect(page.locator("[data-edge-label]:visible")).toHaveCount(4);
    await page.evaluate(size => { document.documentElement.style.fontSize = size; }, rootSize);
    await expect(page.locator("[data-edge-label]:visible")).toHaveCount(4);
    const output = process.env.ONTOLOGY_TYPE_SCALE_SCREENSHOTS;
    if (output && rootSize === "200%") await page.screenshot({ path: `${output}/connections-root-200.png`, fullPage: true });
    await expect.poll(() => page.locator("[data-edge-label]:visible").evaluateAll(labels => labels.flatMap(label => {
      const text = label.querySelector("text")?.getBoundingClientRect();
      const plate = label.querySelector("rect")?.getBoundingClientRect();
      return !text || !plate || text.left < plate.left - 1 || text.right > plate.right + 1 || text.top < plate.top - 1 || text.bottom > plate.bottom + 1 ? [label.textContent] : [];
    }))).toEqual([]);
    for (const text of await page.locator("[class*=contour] text").all()) {
      await expect(text).toHaveCSS("font-size", "14px");
    }
  });

  test(`Overview SVG labels keep their coordinate size at root size ${rootSize}`, async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await mockOntology(page);
    await page.goto("/ontology?space=business");
    const label = page.locator("svg text").filter({ hasText: "Not linked yet" });
    await expect(label).toBeVisible();
    await page.evaluate(size => { document.documentElement.style.fontSize = size; }, rootSize);
    const output = process.env.ONTOLOGY_TYPE_SCALE_SCREENSHOTS;
    if (output && rootSize === "200%") await page.screenshot({ path: `${output}/overview-root-200.png`, fullPage: true });
    await expect(label).toHaveCSS("font-size", "14px");
  });
}

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

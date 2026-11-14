import { test, expect } from "@playwright/test";
import { mockOntology } from "../../../tests/fixtures/ontology-ui";
import fixture from "../../../tests/fixtures/ontology-ui.json";

for (const count of [1, 16, 60, 120])
  test(`kinds stage with ${count} kinds and 300 links stays bounded and stable`, async ({ page }) => {
    await mockOntology(page);
    await page.setViewportSize({ width: 1440, height: 900 });
    const model = structuredClone(fixture.ontology);
    model.kinds = Array.from({ length: count }, (_, i) => ({
      ...model.kinds[0],
      kind: `kind_${i}`,
      entities: count - i,
      status: "approved",
      properties: [],
      metrics: [],
    }));
    model.links = Array.from({ length: 300 }, (_, i) => ({
      ...fixture.ontology.links[0],
      name: `link_${i}`,
      from: `kind_${i % count}`,
      to: `kind_${(i * 7 + 1) % count}`,
    }));
    await page.route("**/api/vaults/business/ontology", (r) => r.fulfill({ json: model }));
    await page.goto("/ontology?space=business");
    const canvas = page.getByRole("group", { name: "Kinds canvas" });
    await expect(canvas).toBeVisible();
    await expect(canvas.locator("[data-wire^='kind:']")).toHaveCount(Math.min(count, 14));
    if (count > 14) await expect(canvas).toContainText(`Showing 14 of ${count} kinds`);
    await expect(page.getByRole("navigation", { name: "Kinds" }).getByRole("button")).toHaveCount(count);
    const box = await canvas.boundingBox();
    expect(box!.height).toBeLessThan(2000);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.getByRole("button", { name: "Show as table" }).click();
    await expect(page.getByRole("table", { name: /Kinds in this space/ })).toBeVisible();
  });

import { test, expect } from "@playwright/test";
import { mockOntology } from "../../../tests/fixtures/ontology-ui";
import fixture from "../../../tests/fixtures/ontology-ui.json";

for (const count of [16, 60, 120])
  test(`every owner issue stays reachable across ${count} kinds`, async ({ page }) => {
    await mockOntology(page);
    await page.setViewportSize({ width: 1440, height: 1000 });
    if (count === 60) await page.addInitScript(() => localStorage.setItem("eunomia-theme", "dark"));
    const model = structuredClone(fixture.ontology);
    model.kinds = Array.from({ length: count }, (_, i) => ({
      ...model.kinds[0],
      kind: `kind_${i}`,
      entities: count - i,
      status: "approved",
      properties: [],
      metrics: [],
    }));
    model.links = [];
    const affected = count === 16 ? 2 : 30;
    for (const kind of model.kinds.slice(-affected))
      kind.properties = [{ ...fixture.ontology.kinds[0].properties[0], name: "needs_owner", status: "proposed" }];
    await page.route("**/api/vaults/business/ontology", (r) => r.fulfill({ json: model }));
    await page.goto("/ontology?space=business");
    await expect(page.getByRole("link", { name: `Review ${affected} suggestions`, exact: true })).toBeVisible();
    const rail = page.getByRole("navigation", { name: "Kinds" });
    await expect(rail.getByRole("button")).toHaveCount(count);
    await expect(rail.getByRole("button").locator("[aria-label$='in review']")).toHaveCount(affected);
    await rail.getByRole("button", { name: new RegExp(`Kind ${count - 1}\\b`) }).click();
    const inspector = page.getByLabel("Inspector");
    await expect(inspector.getByRole("link", { name: /In review: Needs owner/ })).toHaveAttribute("href", new RegExp(`selected=.*kind_${count - 1}`));
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  });

import { writeFile } from "node:fs/promises";
import { test, expect } from "@playwright/test";
import { mockOntology } from "./fixtures/ontology-ui";
import { mockConnections, neighbourhood } from "./fixtures/connections";
import { mockReview } from "./fixtures/ontology-review";
import { mockReceipts } from "./fixtures/receipts";

test.skip(!process.env.ONTOLOGY_TYPE_SCALE_SCREENSHOTS, "Set ONTOLOGY_TYPE_SCALE_SCREENSHOTS to capture evidence");

for (const width of [1440, 390]) for (const theme of ["light", "dark"]) {
  for (const view of ["overview", "connections", "review", "profile"]) {
    test(`type scale ${view} ${width} ${theme}`, async ({ page, baseURL }) => {
      expect(new URL(baseURL ?? "").hostname).toBe("127.0.0.1");
      expect([3000, 8000, 8001, 3600, 8600, 3700, 18470, 18471, 18472, 18473]).not.toContain(Number(new URL(baseURL ?? "").port));
      await page.clock.setFixedTime(new Date("2026-11-11T00:00:00Z"));
      await page.setViewportSize({ width, height: width === 390 ? 844 : 900 });
      await page.emulateMedia({ reducedMotion: "reduce" });
      await page.addInitScript(theme => localStorage.setItem("eunomia-theme", theme), theme);
      await page.route("**/*", route => {
        const url = new URL(route.request().url());
        return url.origin === baseURL ? route.fallback() : route.abort();
      });
      let path = "/ontology?space=business";
      if (view === "connections") {
        await mockConnections(page, "populated", neighbourhood(4));
        path = "/ontology/connections?space=business&focus=house";
      } else if (view === "review") {
        await mockReview(page);
        path = "/ontology/review?space=business";
      } else if (view === "profile") {
        await mockReceipts(page);
        path = "/ontology/objects/house?space=business&section=Properties";
      } else await mockOntology(page);
      await page.goto(path);
      if (view === "overview") {
        await expect(page.getByRole("heading", { name: "House blend 1kg: Unit cost" })).toBeVisible();
        await expect(page.getByRole("region", { name: "Ontology activity", exact: true }).locator("li[class*=activityRow]")).toHaveCount(5);
      }
      if (view === "connections") await expect(page.locator("[data-object=house]").first()).toBeVisible();
      if (view === "review") await expect(page.locator("a[data-definition]").first()).toBeVisible();
      if (view === "profile") await expect(page.getByRole("heading", { name: "House blend 1kg", exact: true })).toBeVisible();
      await page.evaluate(() => document.fonts.ready);
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
      if (view === "connections" && width === 1440) await expect(page.locator("[data-edge-label]:visible")).toHaveCount(4);
      const labels = await page.locator('[data-testid="kind-landscape"] button, [data-pin-type], [data-object], [data-edge-label]:visible').evaluateAll(elements => {
        const boxes = elements.filter(el => el.checkVisibility()).map(el => ({ name: el.textContent, rect: el.getBoundingClientRect() }));
        return boxes.flatMap((a, i) => boxes.slice(i + 1).filter(b => a.rect.left < b.rect.right - 1 && a.rect.right > b.rect.left + 1 && a.rect.top < b.rect.bottom - 1 && a.rect.bottom > b.rect.top + 1).map(b => `${a.name} / ${b.name}`));
      });
      expect(labels).toEqual([]);
      const clippedSvgText = await page.locator("[data-edge-label]:visible").evaluateAll(labels => labels.filter(label => {
        const text = label.querySelector("text")?.getBoundingClientRect();
        const plate = label.querySelector("rect")?.getBoundingClientRect();
        return text && plate && (text.left < plate.left - 1 || text.right > plate.right + 1 || text.top < plate.top - 1 || text.bottom > plate.bottom + 1);
      }).map(label => label.textContent));
      expect(clippedSvgText).toEqual([]);
      const output = process.env.ONTOLOGY_TYPE_SCALE_SCREENSHOTS;
      if (output) {
        const text = await page.locator("main").evaluate(root => [...root.querySelectorAll("*")].filter(el => el.checkVisibility() && [...el.childNodes].some(n => n.nodeType === Node.TEXT_NODE && n.textContent?.trim())).map(el => ({ text: el.textContent?.slice(0, 100), tag: el.tagName, className: el.getAttribute("class"), camera: !!el.closest("[data-camera]"), fontSize: getComputedStyle(el).fontSize })));
        expect(text.filter(item => !item.camera && ![12, 13, 14, 16, 20, 24].includes(parseFloat(item.fontSize)))).toEqual([]);
        await writeFile(`${output}/${view}-${width}-${theme}.json`, JSON.stringify(text, null, 2));
      }
      if (output) await page.screenshot({ path: `${output}/${view}-${width}-${theme}.png`, fullPage: true, style: "nextjs-portal { display: none !important; }" });
    });
  }
}

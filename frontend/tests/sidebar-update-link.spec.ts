import { expect, test } from "@playwright/test";
import { mockOntology } from "./fixtures/ontology-ui";

for (const width of [1440, 390]) {
  for (const theme of ["light", "dark"]) {
    test(`update link stays neutral with a partial selection, ${width} ${theme}`, async ({ page }, testInfo) => {
      await page.setViewportSize({ width, height: 1000 });
      await page.addInitScript(value => localStorage.setItem("eunomia-theme", value), theme);
      const requests = await mockOntology(page);
      await page.route("**/api/update/status", route => route.fulfill({ json: {
        configured: true, current_version: "v2.2.0", latest_version: "v2.3.0",
        update_available: true, applying: false, error: null,
      } }));
      await page.goto("/ontology?space=business");
      if (width === 390) await page.getByRole("button", { name: "Open menu", exact: true }).click();
      const link = page.getByRole("link", { name: "Update available · v2.3.0", exact: true });
      await expect(link).toBeVisible();
      await expect(link).toHaveAttribute("href", "/settings?tab=updates");
      const colours = await link.evaluate(element => {
        const sidebar = element.closest(".sidebar");
        if (!sidebar) throw new Error("Sidebar not found");
        return { link: getComputedStyle(element).color, sidebar: getComputedStyle(sidebar).color };
      });
      await link.evaluate(element => {
        const text = element.firstChild;
        if (!text || text.nodeType !== Node.TEXT_NODE) throw new Error("Update label text not found");
        const range = document.createRange();
        range.setStart(text, 0);
        range.setEnd(text, (text.textContent ?? "").indexOf("available") + "availabl".length);
        window.getSelection()?.removeAllRanges();
        window.getSelection()?.addRange(range);
      });
      await link.screenshot({ path: testInfo.outputPath("partial-selection.png") });
      expect(colours.link).toBe(colours.sidebar);
      await page.evaluate(() => window.getSelection()?.removeAllRanges());
      await link.screenshot({ path: testInfo.outputPath("normal.png") });
      await link.click();
      await expect(page).toHaveURL(/\/settings\?tab=updates$/);
      expect(requests.some(request => request.startsWith("/api/update/request"))).toBe(false);
    });
  }
}

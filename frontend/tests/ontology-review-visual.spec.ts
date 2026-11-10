import { test, expect } from "@playwright/test";
import { mockReview, reviewUrl } from "./fixtures/ontology-review";

for (const theme of ["light", "dark"]) {
  test(`context is static, escaped and matches the API in ${theme}`, async ({ page }) => {
    await page.addInitScript(theme => localStorage.setItem("eunomia-theme", theme), theme);
    const fixture = await mockReview(page);
    fixture.model.kinds[0].properties[0].description = '<img src=x onerror="alert(1)">';
    await page.goto(reviewUrl);
    const context = page.getByRole("figure", { name: "Where this definition sits" });
    await expect(context).toContainText("Stock item");
    await expect(context).toContainText("6,000 objects");
    await expect(context).toContainText("Stock item → Stored at → Warehouse");
    await expect(context).toContainText("Suggested · 120 recorded links");
    await expect(context).not.toContainText("Delivery route");
    await expect(context.getByRole("button")).toHaveCount(0);
    expect(await context.locator("svg path[class*=faceTop]").first().evaluate(el => getComputedStyle(el).fill)).not.toBe("rgb(0, 0, 0)");
    await expect(page.getByRole("table", { name: "Definition comparison" })).toContainText('<img src=x onerror="alert(1)">');
    await expect(page.locator('[aria-label="Selected suggestion"] img')).toHaveCount(0);
    await expect(page.locator('[data-change="Unavailable"]').first()).toContainText("Not comparable");
    expect(fixture.mutations).toHaveLength(0);
  });

  test(`readable text contrast in ${theme}`, async ({ page }) => {
    await page.addInitScript(theme => localStorage.setItem("eunomia-theme", theme), theme);
    await mockReview(page);
    await page.goto(reviewUrl);
    await expect(page.getByRole("figure")).toBeVisible();
    const failures = await page.locator('[aria-label="Ontology review"]').evaluate(root => {
      const canvas = document.createElement("canvas");
      canvas.width = canvas.height = 1;
      const ctx = canvas.getContext("2d")!;
      const rgb = (value: string) => { ctx.clearRect(0, 0, 1, 1); ctx.fillStyle = value; ctx.fillRect(0, 0, 1, 1); return [...ctx.getImageData(0, 0, 1, 1).data]; };
      const luminance = (channels: number[]) => channels.slice(0, 3).map(v => v / 255).map(v => v <= .04045 ? v / 12.92 : ((v + .055) / 1.055) ** 2.4).reduce((sum, v, i) => sum + v * [.2126, .7152, .0722][i], 0);
      return [...root.querySelectorAll<HTMLElement>("*")].filter(el => el.checkVisibility() && [...el.childNodes].some(n => n.nodeType === Node.TEXT_NODE && n.textContent?.trim())).flatMap(el => {
        const style = getComputedStyle(el);
        if (el.closest("button:disabled") || style.visibility === "hidden") return [];
        let parent: Element | null = el;
        let bg = [255, 255, 255, 255];
        while (parent) {
          const candidate = rgb(getComputedStyle(parent).backgroundColor);
          if (candidate[3] === 255) { bg = candidate; break; }
          parent = parent.parentElement;
        }
        const fg = luminance(rgb(style.color));
        const back = luminance(bg);
        const ratio = (Math.max(fg, back) + .05) / (Math.min(fg, back) + .05);
        const large = parseFloat(style.fontSize) >= 24 || (parseFloat(style.fontSize) >= 18.66 && parseInt(style.fontWeight) >= 700);
        return ratio < (large ? 3 : 4.5) ? [`${el.textContent?.slice(0, 65)}: ${ratio.toFixed(2)}`] : [];
      });
    });
    expect(failures).toEqual([]);
  });
}

test("merge comparison marks real field differences and keyboard arming is visible", async ({ page }) => {
  const fixture = await mockReview(page);
  await page.goto(reviewUrl);
  await page.getByRole("heading", { name: "Unit cost", exact: true }).focus();
  await page.keyboard.press("a");
  await expect(page.getByText(/Keyboard decision ready/)).toBeVisible();
  expect(fixture.mutations).toHaveLength(0);
  await page.keyboard.press("Escape");
  await expect(page.getByText(/Keyboard decision ready/)).not.toBeVisible();
  await page.goto(reviewUrl);
  await page.getByText("Merge or edit definition", { exact: true }).click();
  await page.getByRole("combobox", { name: "Merge Unit cost into", exact: true }).selectOption({ label: "Quantity" });
  await page.getByText("Compare destination fields", { exact: true }).click();
  const region = page.getByRole("region", { name: "Merge preview" });
  await expect(region.locator('[data-change="Changed"]').filter({ hasText: "Data type" })).toContainText("money");
  await expect(region.locator('[data-change="Changed"]').filter({ hasText: "Data type" })).toContainText("integer");
  await expect(region.locator('[data-change="Removed"]').filter({ hasText: "Unit" })).toContainText("AUD");
  expect(fixture.mutations).toHaveLength(0);
});

for (const theme of ["light", "dark"]) for (const width of [390, 1440]) test(`200% text keeps context and comparison in flow at ${width} ${theme}`, async ({ page }) => {
  await page.setViewportSize({ width, height: 900 });
  await page.addInitScript(theme => localStorage.setItem("eunomia-theme", theme), theme);
  await page.emulateMedia({ reducedMotion: "reduce" });
  await mockReview(page);
  await page.goto(reviewUrl);
  await expect(page.getByRole("figure")).toBeVisible();
  await page.evaluate(() => document.documentElement.style.fontSize = "200%");
  await expect(page.getByRole("figure")).toContainText("Stock item → Stored at → Warehouse");
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.getByRole("button", { name: "Approve Unit cost", exact: true }).scrollIntoViewIfNeeded();
  await expect(page.getByRole("button", { name: "Approve Unit cost", exact: true })).toBeInViewport();
  if (process.env.ONTOLOGY_REVIEW_FIGURES) await page.screenshot({ path: `${process.env.ONTOLOGY_REVIEW_FIGURES}/text-200-${width}-${theme}.png`, fullPage: true });
});

for (const theme of ["light", "dark"]) for (const width of [390, 1440]) test(`armed decision ${width} ${theme}`, async ({ page }) => {
  await page.setViewportSize({ width, height: 900 });
  await page.addInitScript(theme => localStorage.setItem("eunomia-theme", theme), theme);
  const fixture = await mockReview(page);
  await page.goto(reviewUrl);
  await page.getByRole("heading", { name: "Unit cost", exact: true }).focus();
  await page.keyboard.press("a");
  await expect(page.getByText(/Keyboard decision ready/)).toBeVisible();
  expect(fixture.mutations).toHaveLength(0);
  if (process.env.ONTOLOGY_REVIEW_FIGURES) await page.screenshot({ path: `${process.env.ONTOLOGY_REVIEW_FIGURES}/armed-${width}-${theme}.png`, fullPage: true });
});

test("missing activity never delays evidence or invents proposer details", async ({ page }) => {
  await mockReview(page);
  await page.route("**/ontology/activity?**", route => route.fulfill({ status: 503, json: { detail: "Unavailable" } }));
  await page.goto(reviewUrl);
  await expect(page.getByRole("region", { name: "Source evidence" })).toContainText("AUD 12.00");
  await expect(page.locator("a[data-definition]").first()).toContainText("AI app, name unavailable");
  await expect(page.locator("a[data-definition]").first()).toContainText("Time not recorded");
});

test("inbox labels retain definition identity before type for real-backend flows", async ({ page }) => {
  await mockReview(page);
  await page.goto("/ontology/review?space=business");
  await expect(page.getByRole("link", { name: /warehouse.*kind/i }).first()).toBeVisible();
  await expect(page.getByRole("link", { name: /unit cost.*property.*stock item/i })).toBeVisible();
});

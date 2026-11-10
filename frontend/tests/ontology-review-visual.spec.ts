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
    await expect(page.getByRole("list", { name: "New definition" })).toContainText('<img src=x onerror="alert(1)">');
    await expect(page.locator('[aria-label="Selected suggestion"] img')).toHaveCount(0);
    await expect(page.getByRole("table", { name: "Definition comparison" })).toHaveCount(0);
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


for (const width of [390, 1440]) for (const theme of ["light", "dark"]) test(`decisions stay after the detail without overlapping content at ${width} ${theme}`, async ({ page }) => {
  const fixture = await mockReview(page);
  fixture.model.kinds[0].properties[0].samples = Array.from({ length: 20 }, (_, i) => `AUD ${12 + i}.00`);
  await page.setViewportSize({ width, height: 900 });
  await page.addInitScript(theme => localStorage.setItem("eunomia-theme", theme), theme);
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.goto(reviewUrl);
  const detail = page.getByRole("region", { name: "Selected suggestion", exact: true });
  const bar = detail.locator("footer");
  const approve = bar.getByRole("button", { name: "Approve Unit cost", exact: true });
  await expect(approve).not.toBeInViewport();
  if (process.env.ONTOLOGY_REVIEW_FIGURES) {
    await page.screenshot({ path: `${process.env.ONTOLOGY_REVIEW_FIGURES}/initial-${width}-${theme}.png` });
  }
  // Sample the start, middle and end of the reading path, then return upwards.
  for (const name of ["What changes", "Source evidence", "Consequence of approving", "Your reason", "Decide", "Source evidence", "What changes"]) {
    const target = name === "Your reason" ? detail.locator("textarea") : name === "Decide" ? bar : detail.getByRole("region", { name, exact: true });
    await target.scrollIntoViewIfNeeded();
    expect(await bar.evaluate(el => getComputedStyle(el).position)).toBe("static");
    const overlaps = await bar.evaluate(el => {
      const bar = el.getBoundingClientRect();
      const article = el.closest("article")!;
      return [...article.querySelectorAll("header, nav, section, figure, details, textarea")]
        .filter(other => !el.contains(other) && other.checkVisibility())
        .filter(other => {
          const box = other.getBoundingClientRect();
          return box.width > 0 && box.height > 0 && box.left < bar.right && box.right > bar.left && box.top < bar.bottom && box.bottom > bar.top;
        }).map(other => other.getAttribute("aria-label") || other.tagName);
    });
    expect(overlaps).toEqual([]);
  }
  await approve.scrollIntoViewIfNeeded();
  await expect(approve).toBeInViewport();
  expect(fixture.mutations).toHaveLength(0);
});

for (const width of [390, 1440]) test(`section index only navigates to the detail sections at ${width}`, async ({ page }) => {
  const fixture = await mockReview(page);
  await page.setViewportSize({ width, height: 900 });
  await page.goto(reviewUrl);
  const index = page.getByRole("navigation", { name: "Review sections", exact: true });
  await expect(index.getByRole("button")).toHaveCount(0);
  await expect(index.locator('form, input, [data-review-decision], [role="button"]')).toHaveCount(0);
  await expect(index.getByRole("link")).toHaveText(["What changes", "Read the source", "In your business", "If you approve", "Your reason", "Decide"]);
  for (const link of await index.getByRole("link").all()) {
    const href = await link.getAttribute("href");
    expect(href).toMatch(/^#.+/);
    await link.click();
    const target = page.locator(`[id="${href!.slice(1)}"]`);
    await expect(target).toBeInViewport();
    expect(await target.evaluate(el => el.getBoundingClientRect().top)).toBeGreaterThanOrEqual(0);
    await expect(page.getByRole("button", { name: "Approve Unit cost", exact: true })).not.toBeFocused();
    await expect(page.getByText(/Keyboard decision ready/)).not.toBeVisible();
  }
  expect(fixture.mutations).toHaveLength(0);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
});

test("new definition is a labelled proposal without invented differences", async ({ page }) => {
  await mockReview(page);
  await page.goto(reviewUrl);
  const proposal = page.getByRole("list", { name: "New definition", exact: true });
  await expect(proposal).toContainText("Status");
  await expect(proposal).toContainText("Description");
  await expect(proposal).toContainText("Data type");
  await expect(proposal).toContainText("AUD");
  await expect(page.getByText("There is nothing to compare against yet.", { exact: true })).toBeVisible();
  await expect(page.getByRole("table", { name: "Definition comparison" })).toHaveCount(0);
  await expect(page.locator('[data-change="Added"], [data-change="Changed"]')).toHaveCount(0);
});

test("navigation focus ring hugs the heading text", async ({ page }) => {
  await mockReview(page);
  await page.goto(reviewUrl);
  const heading = page.getByRole("heading", { name: "Unit cost", exact: true });
  await expect(heading).toBeFocused();
  const widths = await heading.evaluate(el => {
    const range = document.createRange();
    range.selectNodeContents(el);
    return { heading: el.getBoundingClientRect().width, text: range.getBoundingClientRect().width };
  });
  expect(widths.heading).toBeLessThanOrEqual(widths.text + 2);
});

test("inbox shares a page of identity-matched activity and cancels on navigation", async ({ page }) => {
  await mockReview(page);
  const requests: string[] = [];
  const aborted: string[] = [];
  page.on("requestfailed", request => { if (request.url().includes("/ontology/activity?") && new URL(request.url()).searchParams.get("limit") !== "5") aborted.push(request.url()); });
  await page.route("**/ontology/activity?**", route => {
    if (new URL(route.request().url()).searchParams.get("limit") === "5") return route.fallback();
    requests.push(route.request().url());
  });
  await page.goto("/ontology/review?space=business");
  await expect.poll(() => requests.length).toBeGreaterThan(0);
  expect(requests).toHaveLength(1);
  expect(new URL(requests[0]).searchParams.has("definition_type")).toBe(false);
  await page.getByRole("searchbox", { name: "Search suggestions", exact: true }).fill("warehouse");
  await expect.poll(() => aborted.length).toBeGreaterThan(0);
  await expect.poll(() => requests.length).toBe(2);
  await page.getByRole("link", { name: "Overview", exact: true }).click();
  await expect.poll(() => aborted.length).toBe(2);
});

for (const width of [390, 1440]) test(`shortcut pairs and proposal stay readable at 200% text at ${width}`, async ({ page }) => {
  await page.setViewportSize({ width, height: 900 });
  await mockReview(page);
  await page.goto(reviewUrl);
  await page.evaluate(() => document.documentElement.style.fontSize = "200%");
  const bar = page.locator('[aria-label="Selected suggestion"] footer');
  for (const text of ["Focus Approve", "Focus Reject", "Confirm"]) {
    const pair = bar.locator("span").filter({ hasText: text }).filter({ has: page.locator("kbd") }).last();
    const height = await pair.evaluate(el => ({ height: el.getBoundingClientRect().height, line: parseFloat(getComputedStyle(el).lineHeight) }));
    expect(height.height).toBeLessThanOrEqual(height.line + 6);
  }
  const proposal = page.getByRole("list", { name: "New definition", exact: true });
  expect(await proposal.evaluate(el => [...el.querySelectorAll("li")].every(row => row.getBoundingClientRect().width >= 200))).toBe(true);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.getByRole("button", { name: "Back to queue", exact: true }).click();
  await expect(page.locator("a[data-definition]").first()).toBeVisible();
  expect(await page.locator("a[data-definition]").evaluateAll(links => links.every(link => link.getBoundingClientRect().width >= 200))).toBe(true);
  if (process.env.ONTOLOGY_REVIEW_FIGURES) {
    await page.evaluate(() => window.scrollTo(0, 0));
    await page.screenshot({ path: `${process.env.ONTOLOGY_REVIEW_FIGURES}/queue-200-${width}-light.png`, fullPage: true });
  }
});

test("inbox does not attribute one definition's proposer to its neighbours", async ({ page }) => {
  await mockReview(page);
  await page.goto("/ontology/review?space=business");
  await expect(page.getByRole("link", { name: /unit cost.*property/i })).toContainText("Claude Code");
  await expect(page.getByRole("link", { name: /warehouse.*kind/i }).first()).toContainText("AI app, name unavailable");
});

test("scoped detail activity cancels when its suggestion is closed", async ({ page }) => {
  await mockReview(page);
  let scoped = "";
  let aborted = false;
  page.on("requestfailed", request => { if (request.url() === scoped) aborted = true; });
  await page.route("**/ontology/activity?**", route => {
    if (!new URL(route.request().url()).searchParams.has("definition_type")) return route.fallback();
    scoped = route.request().url();
  });
  await page.goto(reviewUrl);
  await expect.poll(() => scoped).not.toBe("");
  await page.getByRole("button", { name: "Back to queue", exact: true }).click();
  await expect.poll(() => aborted).toBe(true);
});


for (const width of [390, 1440]) test(`real comparison stays readable at 200% text at ${width}`, async ({ page }) => {
  await page.setViewportSize({ width, height: 900 });
  await mockReview(page);
  await page.goto(reviewUrl);
  await page.evaluate(() => document.documentElement.style.fontSize = "200%");
  await page.getByText("Merge or edit definition", { exact: true }).click();
  await page.getByRole("combobox", { name: "Merge Unit cost into", exact: true }).selectOption({ label: "Quantity" });
  await page.getByText("Compare destination fields", { exact: true }).click();
  const comparison = page.getByRole("table", { name: "Definition comparison", exact: true });
  await expect(comparison).toContainText("money");
  await expect(comparison).toContainText("integer");
  expect(await comparison.getByRole("cell").evaluateAll(cells => cells.every(cell => cell.getBoundingClientRect().width >= 180))).toBe(true);
  expect(await comparison.getByRole("rowheader").locator("strong, span").evaluateAll(labels => labels.every(label => label.getBoundingClientRect().height <= parseFloat(getComputedStyle(label).lineHeight) + 1))).toBe(true);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  if (process.env.ONTOLOGY_REVIEW_FIGURES) await comparison.screenshot({ path: `${process.env.ONTOLOGY_REVIEW_FIGURES}/comparison-200-${width}-light.png`, style: 'nav[aria-label="Main"] { visibility: hidden !important; }' });
});

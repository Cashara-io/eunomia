import { test, expect } from "@playwright/test";
import { mkdir, writeFile } from "node:fs/promises";
import {
  mockConnections,
  neighbourhood,
  type ConnectionState,
} from "./fixtures/connections";
const route = "/ontology/connections?space=business";
test("keyboard journey, direction, filters, same table and separate modes", async ({
  page,
}) => {
  const requests = await mockConnections(page);
  await page.goto(route);
  const search = page.getByRole("searchbox", { name: "Find an object" });
  await search.focus();
  await search.pressSequentially("House");
  const result = page.getByRole("button", {
    name: "House blend 1kg Product",
    exact: true,
  });
  await expect(result).toBeVisible();
  await search.press("Tab");
  await expect(result).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(
    page.getByRole("complementary", { name: "Selected object" }),
  ).toContainText("AUD");
  await expect(
    page.getByRole("region", { name: "Connections in words" }),
  ).toContainText("North Coast Beans supplies House blend 1kg.");
  await page.keyboard.press("Escape");
  await expect(
    page.getByRole("complementary", { name: "Selected object" }),
  ).toHaveCount(0);
  await page.getByLabel("Direction", { exact: true }).selectOption("in");
  await expect(
    page.getByRole("status").filter({ hasText: "object" }),
  ).toContainText("2 objects · 1 link");
  await page.getByRole("button", { name: "Show as table" }).click();
  await expect(
    page
      .getByRole("table", { name: "Objects in this neighbourhood (2)" })
      .locator("tbody tr"),
  ).toHaveCount(2);
  await expect(
    page.getByRole("table", { name: "Directed links (1)" }),
  ).toContainText("North Coast Beans supplies House blend 1kg.");
  const tableObject = page
    .getByRole("table")
    .first()
    .getByRole("button", { name: "House blend 1kg" });
  await tableObject.click();
  await page.keyboard.press("Escape");
  await expect(tableObject).toBeFocused();
  await page.getByLabel("Direction", { exact: true }).selectOption("out");
  await expect(
    page.getByRole("table", { name: "Directed links (3)" }),
  ).not.toContainText("North Coast Beans supplies");
  await page.getByLabel("Relation", { exact: true }).selectOption("stocked_at");
  await expect(
    page.getByRole("table", { name: "Directed links (1)" }),
  ).toContainText("House blend 1kg stocked at Fitzroy store.");
  await page.getByRole("button", { name: "Kinds", exact: true }).click();
  await expect(
    page.getByRole("region", { name: "Objects connections canvas" }),
  ).toHaveCount(0);
  await expect(page.locator('[data-kind="product"]')).toBeVisible();
  await page.getByRole("button", { name: "Objects", exact: true }).click();
  await expect(page.locator("[data-kind]")).toHaveCount(0);
  expect(
    requests
      .filter((r) => "entity_id" in r)
      .every((r) => r.vault_id === "business" && r.depth === 1),
  ).toBeTruthy();
});
test("group count precedes expand, no fetch, deduplicated objects and stable camera/selection", async ({
  page,
}) => {
  const data = neighbourhood(24);
  data.nodes.push(data.nodes[1]);
  data.edges.push(data.edges[0]);
  const requests = await mockConnections(page, "hub", data);
  await page.goto(`${route}&focus=house&selected=house`);
  const group = page.locator("[data-group]");
  await expect(group).toContainText("24 products");
  const camera = await page
    .locator("[data-camera]")
    .getAttribute("data-camera");
  const calls = requests.length;
  await group.click();
  await expect(group).toHaveAttribute("aria-expanded", "true");
  expect(await page.locator("[data-camera]").getAttribute("data-camera")).toBe(
    camera,
  );
  await expect(page).toHaveURL(/selected=house/);
  expect(requests.length).toBe(calls);
  await page.evaluate(() => {
    window.dispatchEvent(new Event("blur"));
    window.dispatchEvent(new Event("focus"));
  });
  await expect(group).toHaveAttribute("aria-expanded", "true");
  expect(await page.locator("[data-camera]").getAttribute("data-camera")).toBe(
    camera,
  );
  expect(requests.length).toBe(calls);
  await page.getByRole("button", { name: "Show as table" }).click();
  await expect(
    page
      .getByRole("table", { name: "Objects in this neighbourhood (25)" })
      .locator("tbody tr"),
  ).toHaveCount(25);
});
test("depth is bounded, reduced motion is instant, labelled camera and keyboard nodes", async ({
  page,
}) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  const requests = await mockConnections(page);
  await page.goto(`${route}&focus=house`);
  const node = page.locator('[data-object="house"]').first();
  await expect(node).toBeVisible();
  await node.focus();
  await page.keyboard.press("ArrowRight");
  await page.keyboard.press("Enter");
  await expect(
    page.getByRole("complementary", { name: "Selected object" }),
  ).toBeVisible();
  for (const name of [
    "Zoom in",
    "Zoom out",
    "Pan right",
    "Pan left",
    "Pan up",
    "Pan down",
    "Fit view",
  ])
    await page.getByRole("button", { name, exact: true }).click();
  await page.getByLabel("Depth", { exact: true }).selectOption("3");
  await expect.poll(() => requests.some((r) => r.depth === 3)).toBeTruthy();
  expect(
    await page.locator('[aria-label="Objects connections"]').evaluate((el) =>
      [el, ...el.querySelectorAll("*")].every((n) => {
        const s = getComputedStyle(n);
        return (
          s.animationName === "none" && parseFloat(s.transitionDuration) === 0
        );
      }),
    ),
  ).toBeTruthy();
});
for (const state of [
  "start",
  "populated",
  "loading",
  "empty",
  "denied",
  "error",
  "truncated",
  "disconnected",
  "hub",
  "filtered",
] satisfies ConnectionState[]) {
  test(`capture ${state}, both themes and sizes`, async ({ browser }) => {
    test.setTimeout(120000);
    const dir = process.env.ONTOLOGY_SCREENSHOTS;
    const index: {
      name: string;
      state: string;
      view: string;
      width: number;
      theme: string;
    }[] = [];
    for (const width of [1440, 390])
      for (const theme of ["light", "dark"]) {
        const context = await browser.newContext({
          viewport: { width, height: width === 1440 ? 1000 : 844 },
          reducedMotion: "reduce",
        });
        const page = await context.newPage();
        await page.addInitScript(
          (t) => localStorage.setItem("eunomia-theme", t),
          theme,
        );
        await mockConnections(page, state);
        await page.goto(
          route +
            (state === "start" ? "" : "&focus=house") +
            (state === "filtered" ? "&relation=absent" : ""),
        );
        await expect(
          page.getByRole("navigation", { name: "Ontology views" }),
        ).toBeVisible();
        if (state === "loading")
          await expect(
            page.getByText("Opening this neighbourhood…"),
          ).toBeVisible();
        else if (state === "error" || state === "denied")
          await expect(page.locator("main").getByRole("alert")).toBeVisible();
        else if (state !== "start")
          await expect(
            page.getByRole("status").filter({ hasText: "object" }),
          ).toBeVisible();
        if (state === "truncated")
          await expect(page.getByText(/Partial neighbourhood/)).toBeVisible();
        if (state === "populated" && width === 1440)
          await expect(
            page.locator('[data-object="house"]').first(),
          ).toBeVisible();
        expect(
          await page.evaluate(() => document.documentElement.scrollWidth),
        ).toBeLessThanOrEqual(width);
        if (
          width === 390 &&
          !["start", "loading", "denied", "error", "empty"].includes(state)
        )
          await expect(page.getByRole("table").first()).toBeVisible();
        await page.evaluate(() => document.fonts.ready);
        await page.addStyleTag({
          content: "nextjs-portal { display:none !important }",
        });
        async function shot(view: string) {
          if (!dir) return;
          await mkdir(dir, { recursive: true });
          const name = `connections-${state}-${view}-${width}-${theme}.png`;
          await page.evaluate(
            () =>
              new Promise<void>((resolve) =>
                requestAnimationFrame(() =>
                  requestAnimationFrame(() => resolve()),
                ),
              ),
          );
          await page.screenshot({ path: `${dir}/${name}`, fullPage: true });
          index.push({ name, state, view, width, theme });
        }
        await shot("idle");
        if (state === "populated") {
          if (width === 1440) {
            await page.locator('[data-object="n0"]').first().hover();
            await shot("hover");
            await page.locator('[data-object="house"]').first().click();
          } else
            await page
              .getByRole("table")
              .first()
              .getByRole("button", { name: "House blend 1kg" })
              .click();
          await expect(
            page.getByRole("complementary", { name: "Selected object" }),
          ).toContainText("Values and their sources");
          await shot("selected");
          if (width === 1440)
            await page.getByRole("button", { name: "Show as table" }).click();
          await shot("table");
        }
        if (state === "hub" && width === 1440) {
          await page.locator("[data-group]").click();
          await expect(page.locator("[data-group]")).toHaveAttribute(
            "aria-expanded",
            "true",
          );
          await expect(
            page.getByText("Arranging this neighbourhood…"),
          ).toHaveCount(0);
          await expect(page.locator('[data-object="n0"]')).toBeAttached();
          await shot("expanded");
        }
        await context.close();
      }
    if (dir)
      await writeFile(
        `${dir}/index-${state}.json`,
        JSON.stringify(index, null, 2),
      );
  });
}

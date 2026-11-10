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
  const requests = await mockConnections(page, "populated", neighbourhood(4));
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
          await page.evaluate(() => window.scrollTo(0, 0));
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

for (const theme of ["light", "dark"])
  for (const width of [1440, 1280, 1024]) {
    test(`rich canvas geometry and linked panels at ${width} ${theme}`, async ({
      page,
    }) => {
      await page.setViewportSize({ width, height: 1000 });
      await page.addInitScript(
        (t) => localStorage.setItem("eunomia-theme", t),
        theme,
      );
      await mockConnections(page);
      await page.goto(`${route}&focus=house`);
      await expect(page.locator('[data-object="house"]').first()).toBeVisible();
      await expect(page.locator("[data-group]")).toContainText("24 orders");
      await expect(page.getByLabel("Neighbourhood summary")).toContainText(
        "36 neighbours",
      );
      const geometry = await page.evaluate(() => {
        const labels = [
          ...document.querySelectorAll(
            "[data-object], [data-group], [data-edge-label]",
          ),
        ]
          .filter((el) => el.closest("[data-camera]"))
          .map((el) => ({
            name: el.textContent,
            box: el.getBoundingClientRect(),
          }));
        const overlaps: string[] = [];
        labels.forEach((a, i) =>
          labels.slice(i + 1).forEach((b) => {
            if (
              Math.min(a.box.right, b.box.right) -
                Math.max(a.box.left, b.box.left) >
                1 &&
              Math.min(a.box.bottom, b.box.bottom) -
                Math.max(a.box.top, b.box.top) >
                1
            )
              overlaps.push(`${a.name} / ${b.name}`);
          }),
        );
        const point = (path: SVGPathElement, at: number) =>
          path
            .getPointAtLength(at)
            .matrixTransform(path.getScreenCTM() ?? undefined);
        const distances = [
          ...document.querySelectorAll<SVGPathElement>("[data-edge-path]"),
        ].flatMap((path) =>
          ["from", "to"].map((end, i) => {
            const form = document.querySelector(
              `[data-form="${CSS.escape(path.getAttribute(`data-${end}`) ?? "")}"]`,
            );
            const top = [
              ...(form?.querySelectorAll<SVGPathElement>(
                '[class*="faceTop"]',
              ) ?? []),
            ].at(-1);
            if (!top) return Infinity;
            const p = point(path, i === 0 ? 0 : path.getTotalLength());
            const plate = form?.parentElement
              ?.querySelector("button")
              ?.getBoundingClientRect();
            if (
              plate &&
              p.x >= plate.left &&
              p.x <= plate.right &&
              p.y >= plate.top &&
              p.y <= plate.bottom
            )
              return Infinity;
            const length = top.getTotalLength();
            let closest = Infinity;
            for (let at = 0; at <= length; at += 0.5) {
              const q = point(top, at);
              closest = Math.min(closest, Math.hypot(p.x - q.x, p.y - q.y));
            }
            return closest;
          }),
        );
        const attached = [
          ...document.querySelectorAll<SVGPathElement>("[data-label-stalk]"),
        ].every((stalk) => {
          const mark = stalk.closest("[data-active]");
          const plate = mark?.querySelector("button")?.getBoundingClientRect();
          const p = point(stalk, 0);
          return (
            plate &&
            p.x >= plate.left &&
            p.x <= plate.right &&
            p.y >= plate.top &&
            p.y <= plate.bottom
          );
        });
        return { overlaps, distances, attached, routes: distances.length / 2 };
      });
      expect(geometry.overlaps).toEqual([]);
      expect(geometry.attached).toBe(true);
      expect(geometry.routes).toBe(13);
      expect(Math.max(...geometry.distances)).toBeLessThan(1);
      const dir = process.env.ONTOLOGY_SCREENSHOTS;
      if (dir)
        await page.screenshot({
          path: `${dir}/connections-rich-idle-${width}-${theme}.png`,
          fullPage: true,
        });
      await page.getByRole("button", { name: "Highlight Orders: 24" }).hover();
      await expect(page.locator("[data-group]").locator("..")).toHaveAttribute(
        "data-active",
        "true",
      );
      await page
        .getByRole("heading", {
          name: "Follow an object and the records that connect it.",
        })
        .hover();
      await page.getByLabel("Depth", { exact: true }).selectOption("2");
      await expect(page.locator('[data-hop-ring="2"]')).toBeAttached();
      await expect(
        page.getByRole("status").filter({ hasText: "objects" }),
      ).toContainText("73 objects");
      if (dir)
        await page.screenshot({
          path: `${dir}/connections-rich-two-hops-${width}-${theme}.png`,
          fullPage: true,
        });
      await page.getByLabel("Depth", { exact: true }).selectOption("3");
      await expect(page.locator('[data-hop-ring="3"]')).toBeAttached();
      await expect(
        page.getByRole("status").filter({ hasText: "objects" }),
      ).toContainText("109 objects");
      if (dir)
        await page.screenshot({
          path: `${dir}/connections-rich-three-hops-${width}-${theme}.png`,
          fullPage: true,
        });
    });
  }

test("opposing and self links keep API direction in the words, table and canvas", async ({
  page,
}) => {
  const data = neighbourhood(1);
  data.edges = [
    { id: "a", from: "house", label: "ordered_from", to: "n0" },
    { id: "b", from: "n0", label: "supplies", to: "house" },
    { id: "c", from: "house", label: "replaces", to: "house" },
  ];
  await mockConnections(page, "populated", data);
  await page.goto(`${route}&focus=house`);
  const words = page.getByRole("region", { name: "Connections in words" });
  const sentences = [
    "House blend 1kg ordered from North Coast Beans.",
    "North Coast Beans supplies House blend 1kg.",
    "House blend 1kg replaces House blend 1kg.",
  ];
  for (const sentence of sentences) await expect(words).toContainText(sentence);
  await expect(page.locator("[data-edge-path]")).toHaveCount(3);
  await page.getByRole("button", { name: "Show as table" }).click();
  for (const sentence of sentences)
    await expect(
      page.getByRole("table", { name: "Directed links (3)" }),
    ).toContainText(sentence);
});

for (const count of [1, 5])
  test(`tiny neighbourhood ${count} stays composed`, async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 1000 });
    await mockConnections(page, "populated", neighbourhood(count));
    await page.goto(`${route}&focus=house`);
    await expect(page.locator('[data-object="house"]').first()).toBeVisible();
    await expect(page.locator("[data-edge-path]")).toHaveCount(count);
    const dir = process.env.ONTOLOGY_SCREENSHOTS;
    if (dir)
      await page.screenshot({
        path: `${dir}/connections-tiny-${count}-1440-light.png`,
        fullPage: true,
      });
  });
test("shortcuts, linked highlights and recent objects are keyboard reachable", async ({
  page,
}) => {
  await mockConnections(page);
  await page.goto(`${route}&focus=house`);
  await page.getByRole("button", { name: "Keyboard shortcuts" }).click();
  await expect(page.getByText("Explore with your keyboard")).toBeVisible();
  const canvas = page.getByRole("region", {
    name: "Objects connections canvas",
  });
  await canvas.focus();
  await page.keyboard.press("?");
  await expect(page.getByText("Explore with your keyboard")).toHaveCount(0);
  const summary = page.getByRole("button", { name: "Highlight Suppliers: 3" });
  await summary.focus();
  await page.getByRole("button", { name: "Highlight Orders: 24" }).hover();
  await expect(page.locator('[data-form="n0"]').locator("..")).toHaveAttribute(
    "data-active",
    "true",
  );
  await page.locator('[data-object="house"]').first().click();
  await expect(page.getByLabel("Recently viewed objects")).toContainText(
    "House blend 1kg",
  );
  await expect(page.locator("[data-edge-path]").first()).toHaveCSS(
    "animation-duration",
    "6s",
  );
});

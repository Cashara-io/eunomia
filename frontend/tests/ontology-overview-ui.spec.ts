import { test, expect } from "@playwright/test";
import { mkdir, writeFile } from "node:fs/promises";
import { mockOntology } from "./fixtures/ontology-ui";

// All traffic is intercepted. No backend or account creation is needed.
test("overview explains real relationships and named issues, with one list and one profile", async ({
  page,
}) => {
  const requests = await mockOntology(page);
  await page.goto("/ontology?space=business");
  await expect(
    page.getByRole("heading", { name: "Your business, connected." }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "House blend 1kg: Unit cost" }),
  ).toBeVisible();
  await expect(
    page.getByRole("link", { name: "Inspect stale input" }),
  ).toHaveAttribute("href", /section=Properties/);
  await expect(
    page.getByRole("link", { name: "Review 7 suggestions" }),
  ).toBeVisible();
  await expect(
    page.getByRole("region", { name: "Structure by status" }),
  ).not.toBeVisible();
  expect(new Set(requests.filter((r) => /\/objects\?/.test(r))).size).toBe(1);
  expect(new Set(requests.filter((r) => /\/objects\/house/.test(r))).size).toBe(
    1,
  );
  await page.getByRole("button", { name: "Show table" }).click();
  await expect(
    page.getByRole("table", { name: "The same connections as the diagram" }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Show landscape" }).click();
  const node = page.getByRole("button", { name: /Orders.*objects/ }).first();
  await node.click();
  await expect(node).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByRole("link", { name: "Find Orders" })).toBeVisible();
  await page
    .getByText("Structure counts and readiness rule", { exact: false })
    .first()
    .click();
  await expect(
    page.getByRole("region", { name: "Structure by status" }),
  ).toBeVisible();
});

test("empty overview offers mapping and never a primary zero-review action", async ({
  page,
}) => {
  await mockOntology(page, "empty");
  await page.goto("/ontology?space=business");
  await expect(
    page.getByRole("heading", { name: "Give your records a shared language." }),
  ).toBeVisible();
  await expect(page.getByRole("link", { name: /0.*review/ })).toHaveCount(0);
  await page
    .getByRole("button", { name: "Add data", exact: true })
    .last()
    .click();
  await expect(
    page.getByRole("region", { name: "Add data", exact: true }),
  ).toContainText("does not import rows");
});

test("failed object-kind prerequisite has local recovery and no dependent loading", async ({
  page,
}) => {
  await mockOntology(page, "error", "/ontology/objects");
  await page.goto("/ontology/objects?space=business");
  await expect(page.locator("main").getByRole("alert")).toContainText(
    "Could not load object kinds",
  );
  await expect(page.getByText("Loading objects", { exact: true })).toHaveCount(
    0,
  );
  await page.route("**/api/vaults/business/ontology/objects", (route) =>
    route.fulfill({ json: { kinds: [] } }),
  );
  await page.getByRole("button", { name: "Retry", exact: true }).click();
  await expect(page.getByText(/No objects or kinds recorded/)).toBeVisible();
});

test("filtered objects offer clear filters", async ({ page }) => {
  await mockOntology(page, "filtered");
  await page.goto(
    "/ontology/objects?space=business&kind=product&search=absent",
  );
  await expect(page.getByText("No objects match these filters.")).toBeVisible();
  await page
    .getByRole("button", { name: "Clear filters", exact: true })
    .last()
    .click();
  await expect(page).not.toHaveURL(/search=/);
});

test("keyboard More, old routes, history, theme and selected space survive", async ({
  page,
}) => {
  await mockOntology(page);
  await page.addInitScript(() => localStorage.setItem("eunomia-theme", "dark"));
  await page.goto("/ontology?space=business");
  const more = page.locator("summary").filter({ hasText: "More" });
  await more.focus();
  await page.keyboard.press("Enter");
  await expect(
    page.getByRole("link", { name: "Structure", exact: true }),
  ).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(more).toBeFocused();
  await more.click();
  await page.getByRole("link", { name: "Structure", exact: true }).click();
  await expect(page).toHaveURL(/ontology\/schema\?space=business/);
  await page.goBack();
  await expect(
    page.getByRole("heading", { name: "Your business, connected." }),
  ).toBeVisible();
  await page.goForward();
  await expect(page).toHaveURL(/ontology\/schema/);
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await page.goto("/ontology/kinds/product?space=business&section=properties");
  await expect(
    page.getByRole("link", { name: "Back to Structure" }),
  ).toBeVisible();
});

test("last object return is reauthorised and preserves section", async ({
  page,
}) => {
  await mockOntology(page);
  await page.goto("/ontology/objects/house?space=business&section=Properties");
  await page.getByRole("link", { name: "Overview", exact: true }).click();
  await expect(page.getByText("Last object opened in this tab")).toBeVisible();
  await expect(
    page.getByRole("link", { name: /Continue reading.*House blend/ }),
  ).toHaveAttribute("href", /section=Properties/);
});

for (const width of [1440, 390, 320])
  for (const theme of ["light", "dark"])
    test(`reflow ${width} ${theme}, text 200%, touch and reading order`, async ({
      page,
    }) => {
      await mockOntology(page);
      await page.setViewportSize({ width, height: 900 });
      await page.addInitScript(
        (theme) => localStorage.setItem("eunomia-theme", theme),
        theme,
      );
      await page.goto("/ontology?space=business");
      await expect(
        page.getByRole("heading", { name: "Your business, connected." }),
      ).toBeVisible();
      await expect(
        page.getByRole("heading", { name: "House blend 1kg: Unit cost" }),
      ).toBeVisible();
      await page.evaluate(() => {
        const elements = [
          ...document.querySelectorAll<HTMLElement>("main, main *"),
        ];
        const sizes = elements.map(
          (element) => getComputedStyle(element).fontSize,
        );
        elements.forEach((element, index) => {
          element.style.fontSize = `${parseFloat(sizes[index]) * 2}px`;
        });
      });
      expect(
        await page.evaluate(() => ({
          width: innerWidth,
          overflow: [...document.querySelectorAll("main *")]
            .filter((e) => e.getBoundingClientRect().right > innerWidth + 1)
            .map((e) => ({
              tag: e.tagName,
              text: e.textContent?.slice(0, 40),
              right: e.getBoundingClientRect().right,
            })),
        })),
      ).toEqual({ width, overflow: [] });
      await expect(
        page.getByRole("link", {
          name: "Change space, current space Banksia Coffee",
        }),
      ).toBeVisible();
      await expect(
        page.getByRole("navigation", { name: "Ontology views" }),
      ).toBeVisible();
      const headings = await page.locator("main h1, main h2").allTextContents();
      expect(headings.indexOf("How it fits together")).toBeLessThan(
        headings.indexOf("Needs your judgement"),
      );
      if (process.env.ONTOLOGY_SCREENSHOTS) {
        await mkdir(process.env.ONTOLOGY_SCREENSHOTS, { recursive: true });
        await page.addStyleTag({
          content: "nextjs-portal { display:none!important }",
        });
        await writeFile(
          `${process.env.ONTOLOGY_SCREENSHOTS}/reading-order-${width}-${theme}.txt`,
          await page.locator("main").ariaSnapshot(),
        );
        await page.screenshot({
          path: `${process.env.ONTOLOGY_SCREENSHOTS}/overview-text200-${width}-${theme}.png`,
          fullPage: true,
        });
      }
    });

test("space change removes the previous content, unavailable links do not select another space", async ({
  page,
}) => {
  await mockOntology(page);
  await page.goto("/ontology?space=business");
  await expect(
    page.getByRole("heading", { name: "House blend 1kg: Unit cost" }),
  ).toBeVisible();
  await page.goto("/ontology?space=no-access");
  await expect(
    page.getByText("This space is unavailable. Choose a space you can access."),
  ).toBeVisible();
  await expect(page.getByText("House blend 1kg", { exact: true })).toHaveCount(
    0,
  );
  await page.goto("/vaults");
  await expect(
    page.getByRole("heading", { name: "Memory spaces", exact: true }),
  ).toBeVisible();
});

for (const width of [1440, 390])
  test(`single landscape, linked panels and keyboard selection at ${width}`, async ({
    page,
  }) => {
    await mockOntology(page);
    await page.setViewportSize({ width, height: 900 });
    await page.goto("/ontology?space=business");
    const nodes = page.locator("[data-kind]");
    await expect(nodes).toHaveCount(16);
    expect(
      new Set(
        await nodes.evaluateAll((elements) =>
          elements.map((el) => el.getAttribute("data-kind")),
        ),
      ).size,
    ).toBe(16);
    const node = page.locator('[data-kind="customer"]');
    await node.focus();
    await page.keyboard.press("ArrowRight");
    await expect(page.locator('[data-kind="delivery"]')).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(page.locator('[data-kind="delivery"]')).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    await expect(
      page.getByRole("link", { name: "Find Deliveries", exact: true }),
    ).toHaveAttribute("href", /kind=delivery/);
    await page.keyboard.press("Escape");
    await expect(page.locator('[data-kind="delivery"]')).toHaveAttribute(
      "aria-pressed",
      "false",
    );
    await node.hover();
    await expect(
      page
        .getByRole("region", { name: "Objects per kind" })
        .getByRole("button", { name: "Customers: 2,100 objects" }),
    ).toHaveAttribute("data-dim", "false");
    await expect(
      page.locator('[data-kind="warehouse"]').locator(".."),
    ).toHaveAttribute("data-dim", "true");
    await page
      .getByRole("region", { name: "Objects per kind" })
      .getByRole("button", { name: "Products: 1,240 objects" })
      .focus();
    await expect(
      page.locator('[data-kind="product"]').locator(".."),
    ).toHaveAttribute("data-dim", "false");
    await expect(
      page.locator('[data-kind="warehouse"]').locator(".."),
    ).toHaveAttribute("data-dim", "true");
    if (width === 390) {
      expect(
        await page.locator('[data-testid="kind-landscape"] > svg').isVisible(),
      ).toBe(false);
      await expect(
        page.locator('[data-kind="customer"]').locator("../ul"),
      ).toContainText("Has invoice");
    }
    await page.getByRole("button", { name: "Show table", exact: true }).click();
    await expect(
      page
        .getByRole("table", { name: "The same kinds as the landscape" })
        .locator("tbody tr"),
    ).toHaveCount(16);
    await expect(
      page
        .getByRole("table", { name: "The same connections as the diagram" })
        .locator("tbody tr"),
    ).toHaveCount(48);
  });

test("reduced motion stops routes, pin destinations stay scoped, and activity keeps import progress", async ({
  page,
}) => {
  await mockOntology(page);
  await page.clock.install({ time: new Date("2026-11-10T03:34:00Z") });
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.goto("/ontology?space=business");
  await page.locator('[data-kind="product"]').click();
  const animations = await page
    .locator('[data-active="true"]')
    .evaluateAll((elements) =>
      elements.map((el) => getComputedStyle(el).animationName),
    );
  expect(animations.every((name) => name === "none")).toBe(true);
  await expect(
    page
      .getByRole("link", {
        name: "Inspect stale Unit cost on House blend 1kg",
      })
      .last(),
  ).toHaveAttribute("href", /space=business&section=Properties/);
  await page.route("**/api/vaults/business/ontology/activity?*", (route) =>
    route.fulfill({
      json: {
        events: [
          {
            seq: 101,
            at: "2026-11-09T03:32:00Z",
            actor: "app",
            actor_name: "Import app",
            actor_type: "token",
            action: "import_progress",
            definition_type: "import",
            kind: "product",
            name: "products.csv",
            target: "",
            revision: 1,
            reason: null,
            after: { processed: 100, total: 1240 },
          },
        ],
        next: 100,
        more: false,
      },
    }),
  );
  await page.goto("/ontology/activity?space=business");
  await expect(page.getByText("100 of 1,240 rows processed")).toBeVisible();
  await expect(page.getByText("Yesterday", { exact: true })).toBeVisible();
  await expect(page.locator("main time")).toHaveAttribute(
    "datetime",
    "2026-11-09T03:32:00Z",
  );
  await expect(page.locator("main time")).toHaveAttribute("title", /2026/);
});

test("landscape hover and selection captures, contrast and frame timing", async ({
  page,
}, testInfo) => {
  await mockOntology(page);
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.addInitScript(() => localStorage.setItem("eunomia-theme", "dark"));
  await page.goto("/ontology?space=business");
  await expect(page.locator("[data-kind]")).toHaveCount(16);
  await page.addStyleTag({ content: "nextjs-portal{display:none!important}" });
  await page.locator('[data-kind="order"]').hover();
  if (process.env.ONTOLOGY_SCREENSHOTS)
    await page.screenshot({
      path: `${process.env.ONTOLOGY_SCREENSHOTS}/overview-hover-1440-dark.png`,
      fullPage: true,
      animations: "disabled",
    });
  await page.locator('[data-kind="order"]').click();
  await page.mouse.move(20, 20);
  await expect(
    page.getByRole("link", { name: "Find Orders", exact: true }),
  ).toBeVisible();
  if (process.env.ONTOLOGY_SCREENSHOTS)
    await page.screenshot({
      path: `${process.env.ONTOLOGY_SCREENSHOTS}/overview-selected-1440-dark.png`,
      fullPage: true,
      animations: "disabled",
    });
  for (const theme of ["light", "dark"]) {
    const contrast = await page.evaluate((theme) => {
      document.documentElement.dataset.theme = theme;
      const read = (token: string) => {
        const el = document.createElement("span");
        el.style.color = `var(${token})`;
        document.body.appendChild(el);
        const channels =
          getComputedStyle(el)
            .color.match(/\d+(?:\.\d+)?/g)
            ?.slice(0, 3)
            .map(Number) ?? [];
        el.remove();
        const [r, g, b] = channels.map((v) => {
          v /= 255;
          return v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
        });
        return 0.2126 * r + 0.7152 * g + 0.0722 * b;
      };
      const warning = read("--warning");
      return ["--canvas", "--surface"].map((token) => {
        const background = read(token);
        return (
          (Math.max(warning, background) + 0.05) /
          (Math.min(warning, background) + 0.05)
        );
      });
    }, theme);
    expect(Math.min(...contrast)).toBeGreaterThanOrEqual(4.5);
    await testInfo.attach(`stale-contrast-${theme}`, {
      body: JSON.stringify(contrast),
      contentType: "application/json",
    });
  }
  const idle = await page.evaluate(async () => {
    const frames: number[] = [];
    let previous = performance.now();
    for (let i = 0; i < 60; i++)
      await new Promise<void>((resolve) =>
        requestAnimationFrame((now) => {
          frames.push(now - previous);
          previous = now;
          resolve();
        }),
      );
    const sorted = frames.slice(5).sort((a, b) => a - b);
    return { p95Ms: sorted[Math.floor(sorted.length * 0.95)] };
  });
  const timing = await page.evaluate(async () => {
    const nodes = [...document.querySelectorAll("[data-kind]")];
    const frames: number[] = [];
    let previous = performance.now();
    for (let i = 0; i < 90; i++)
      await new Promise<void>((resolve) =>
        requestAnimationFrame((now) => {
          frames.push(now - previous);
          previous = now;
          nodes[i % nodes.length].dispatchEvent(
            new MouseEvent("mouseover", { bubbles: true }),
          );
          resolve();
        }),
      );
    const sorted = frames.slice(5).sort((a, b) => a - b);
    return {
      samples: sorted.length,
      p95Ms: sorted[Math.floor(sorted.length * 0.95)],
      maxMs: sorted.at(-1),
    };
  });
  await testInfo.attach("landscape-frame-timing", {
    body: JSON.stringify(timing),
    contentType: "application/json",
  });
  if (process.env.ONTOLOGY_SCREENSHOTS)
    await writeFile(
      `${process.env.ONTOLOGY_SCREENSHOTS}/landscape-timing.json`,
      JSON.stringify({ idle, hover: timing }, null, 2),
    );
});

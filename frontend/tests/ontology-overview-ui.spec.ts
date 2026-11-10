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
    page.getByRole("heading", { name: "House blend 1kg: unit cost" }),
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
  await page.getByRole("button", { name: "Show diagram" }).click();
  const node = page.getByRole("button", { name: /order.*objects/ }).first();
  await node.click();
  await expect(node).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByRole("link", { name: "Find order" })).toBeVisible();
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
        page.getByRole("heading", { name: "House blend 1kg: unit cost" }),
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
    page.getByRole("heading", { name: "House blend 1kg: unit cost" }),
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

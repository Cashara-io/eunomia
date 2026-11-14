import { test, expect } from "@playwright/test";
import { mkdir, writeFile } from "node:fs/promises";
import { mockOntology } from "./fixtures/ontology-ui";


test("kinds stage lists every kind, keeps review visible and offers the same data as a table", async ({ page }) => {
  const requests = await mockOntology(page);
  await page.goto("/ontology?space=business");
  const rail = page.getByRole("navigation", { name: "Kinds" });
  await expect(rail.getByRole("button", { name: /Stock item/ })).toBeVisible();
  await expect(rail.getByRole("button")).toHaveCount(16);
  await expect(rail.getByRole("link", { name: /Review/ })).toContainText("7");
  await expect(page.getByRole("link", { name: "Review 7 suggestions" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Needs your judgement" })).toBeVisible();
  await expect(page.getByRole("group", { name: "Kinds canvas" })).toContainText("Showing 14 of 16 kinds");
  await expect(page.getByRole("region", { name: "Structure by status" })).not.toBeVisible();
  expect(requests.filter((r) => /\/objects\/house/.test(r))).toHaveLength(0);
  await page.getByRole("button", { name: "Show as table" }).click();
  await expect(page.getByRole("table", { name: /Kinds in this space/ })).toBeVisible();
  await expect(page.getByRole("table", { name: /Recorded link types/ })).toBeVisible();
  await page.getByRole("button", { name: "Show canvas" }).click();
  const tile = page.locator('[data-wire="kind:order"]');
  await tile.click();
  await expect(tile).toHaveAttribute("aria-pressed", "true");
  await expect(page).toHaveURL(/mode=kinds&kind=order|kind=order&mode=kinds|kind=order/);
  await expect(page.getByLabel("Inspector")).toContainText("Order");
  await expect(page.getByRole("link", { name: "Read the definition" })).toBeVisible();
  await page.goBack();
  await expect(tile).toHaveAttribute("aria-pressed", "false");
  await page.getByText("Structure counts and readiness rule", { exact: false }).first().click();
  await expect(page.getByRole("region", { name: "Structure by status" })).toBeVisible();
});

test("empty overview offers mapping and never a primary zero-review action", async ({ page }) => {
  await mockOntology(page, "empty");
  await page.goto("/ontology?space=business");
  await expect(page.getByRole("heading", { name: "Start with the things your business knows." })).toBeVisible();
  await expect(page.getByRole("link", { name: /Review \d+ suggestion/ })).toHaveCount(0);
  await page.getByRole("button", { name: "Add data", exact: true }).last().click();
  await expect(page.getByRole("region", { name: "Add data", exact: true })).toContainText("does not import rows");
});

test("a failed ontology read keeps the stage and offers Retry in the canvas", async ({ page }) => {
  await mockOntology(page, "error");
  await page.goto("/ontology?space=business");
  const canvas = page.getByRole("group", { name: "Kinds canvas" });
  await expect(canvas.getByRole("alert")).toContainText("Could not load the ontology");
  await expect(page.getByRole("button", { name: "Kinds", exact: true })).toBeVisible();
  await expect(page.getByRole("navigation", { name: "Ontology views" })).toBeVisible();
});

test("loading keeps the stage as placeholders", async ({ page }) => {
  await mockOntology(page, "loading");
  await page.goto("/ontology?space=business");
  await expect(page.getByRole("status").filter({ hasText: "Reading the kinds" })).toBeVisible();
  await expect(page.getByRole("navigation", { name: "Kinds" })).toBeVisible();
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
  await expect(page.getByRole("group", { name: "Kinds canvas" })).toBeVisible();
  await page.goForward();
  await expect(page).toHaveURL(/ontology\/schema/);
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await page.goto("/ontology/kinds/product?space=business&section=properties");
  await expect(
    page.getByRole("link", { name: "Back to Structure" }),
  ).toBeVisible();
});


for (const width of [1440, 390, 320])
  for (const theme of ["light", "dark"])
    test(`stage reflows at ${width} ${theme}, text 200% and no horizontal overflow`, async ({ page }) => {
      await mockOntology(page);
      await page.setViewportSize({ width, height: 900 });
      await page.addInitScript((theme) => localStorage.setItem("eunomia-theme", theme), theme);
      await page.goto("/ontology?space=business");
      await expect(page.getByRole("group", { name: "Kinds canvas" })).toBeVisible();
      await page.evaluate(() => {
        const elements = [...document.querySelectorAll<HTMLElement>("main, main *")];
        const sizes = elements.map((element) => getComputedStyle(element).fontSize);
        elements.forEach((element, index) => {
          element.style.fontSize = `${parseFloat(sizes[index]) * 2}px`;
        });
      });
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await expect(page.getByRole("link", { name: "Change space, current space Banksia Coffee" })).toBeVisible();
      await expect(page.getByRole("navigation", { name: "Ontology views" })).toBeVisible();
    });

test("space change removes the previous content, unavailable links do not select another space", async ({ page }) => {
  await mockOntology(page);
  await page.goto("/ontology?space=business");
  await expect(page.getByRole("navigation", { name: "Kinds" }).getByRole("button", { name: /Stock item/ })).toBeVisible();
  await page.goto("/ontology?space=no-access");
  await expect(page.getByText("This space is unavailable. Choose a space you can access.")).toBeVisible();
  await expect(page.getByText("Stock item", { exact: true })).toHaveCount(0);
  await page.goto("/vaults");
  await expect(page.getByRole("heading", { name: "Memory spaces", exact: true })).toBeVisible();
});

test("activity keeps import progress and exact times", async ({ page }) => {
  await mockOntology(page);
  await page.clock.install({ time: new Date("2026-11-10T03:34:00Z") });
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

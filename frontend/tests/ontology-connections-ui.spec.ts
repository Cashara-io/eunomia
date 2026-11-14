import { test, expect } from "@playwright/test";
import {
  mockConnections,
  neighbourhood,
  type ConnectionState,
} from "./fixtures/connections";
const route = "/ontology/connections?space=business";
const quiet = (page: import("@playwright/test").Page) =>
  page.routeWebSocket("**/api/events/ws*", (ws) =>
    ws.send(JSON.stringify({ type: "hello", vaults: { business: 0 } })),
  );

test("keyboard journey, direction, filters, same table and separate levels", async ({ page }) => {
  const requests = await mockConnections(page, "populated", neighbourhood(4));
  await page.goto(route);
  const search = page.getByRole("searchbox", { name: "Find an object" });
  await search.focus();
  await search.pressSequentially("House");
  const result = page.getByRole("button", { name: "House blend 1kg Product", exact: true });
  await expect(result).toBeVisible();
  await result.focus();
  await page.keyboard.press("Enter");
  await expect(page.getByRole("complementary", { name: "Selected object" })).toContainText("AUD");
  await page.getByRole("button", { name: "Show as table" }).click();
  await expect(page.getByRole("table", { name: "Directed links (4)" })).toContainText(
    "North Coast Beans supplies House blend 1kg.",
  );
  await page.getByLabel("Direction", { exact: true }).selectOption("in");
  await expect(page.getByRole("table", { name: "Objects in this neighbourhood (2 objects)" }).locator("tbody tr")).toHaveCount(2);
  await page.getByLabel("Direction", { exact: true }).selectOption("out");
  await expect(page.getByRole("table", { name: "Directed links (3)" })).not.toContainText("North Coast Beans supplies");
  await page.getByLabel("Relation", { exact: true }).selectOption("stocked_at");
  await expect(page.getByRole("table", { name: "Directed links (1)" })).toContainText(
    "House blend 1kg stocked at Fitzroy store.",
  );
  await page.getByRole("button", { name: "Kinds", exact: true }).click();
  await expect(page.getByRole("group", { name: "Kinds canvas" })).toBeVisible();
  await expect(page.getByRole("group", { name: "Object connections canvas" })).toHaveCount(0);
  await page.getByRole("button", { name: "Objects", exact: true }).click();
  await expect(page.getByRole("group", { name: "Kinds canvas" })).toHaveCount(0);
  expect(
    requests.filter((r) => "entity_id" in r).every((r) => r.vault_id === "business" && r.depth === 1),
  ).toBeTruthy();
});

test("group count precedes expand, no refetch and deduplicated objects", async ({ page }) => {
  const data = neighbourhood(24);
  data.nodes.push(data.nodes[1]);
  data.edges.push(data.edges[0]);
  const requests = await mockConnections(page, "hub", data);
  await quiet(page);
  await page.goto(`${route}&focus=house&selected=house`);
  const stack = page.locator("[data-wire^='stack:']").first();
  await expect(stack).toContainText(/\d+ products/);
  const calls = () => requests.filter((r) => "entity_id" in r).length;
  const before = calls();
  await stack.click();
  await expect(stack).toHaveAttribute("aria-expanded", "true");
  await expect(page).toHaveURL(/selected=house/);
  await page.locator('[data-object="n0"]').first().click();
  await expect(page).toHaveURL(/selected=n0/);
  await page.waitForTimeout(450);
  expect(calls()).toBe(before);
  await page.getByRole("button", { name: "Show as table" }).click();
  await expect(page.getByRole("table", { name: "Objects in this neighbourhood (25 objects)" }).locator("tbody tr")).toHaveCount(25);
});

for (const state of [
  "start", "populated", "loading", "empty", "denied", "error", "truncated", "disconnected", "hub", "filtered",
] satisfies ConnectionState[])
  for (const width of [1440, 390])
    for (const theme of ["light", "dark"])
      test(`objects ${state} stays inside the viewport at ${width} ${theme}`, async ({ page }) => {
        await page.setViewportSize({ width, height: width === 390 ? 844 : 900 });
        await page.addInitScript((t) => localStorage.setItem("eunomia-theme", t), theme);
        await mockConnections(page, state);
        await page.goto(`${route}&focus=house`);
        await expect(page.locator("main")).toBeVisible();
        await page.waitForTimeout(state === "loading" ? 300 : 800);
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      });

test("opposing and self links keep API direction in the words and table", async ({ page }) => {
  const data = neighbourhood(1);
  data.edges = [
    { id: "a", from: "house", label: "ordered_from", to: "n0" },
    { id: "b", from: "n0", label: "supplies", to: "house" },
    { id: "c", from: "house", label: "replaces", to: "house" },
  ];
  await mockConnections(page, "populated", data);
  await page.goto(`${route}&focus=house`);
  await page.getByRole("button", { name: "Show as table" }).click();
  for (const sentence of [
    "House blend 1kg ordered from North Coast Beans.",
    "North Coast Beans supplies House blend 1kg.",
    "House blend 1kg replaces House blend 1kg.",
  ])
    await expect(page.getByRole("table", { name: "Directed links (3)" })).toContainText(sentence);
});

for (const count of [1, 5])
  test(`tiny neighbourhood ${count} stays composed`, async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 1000 });
    await mockConnections(page, "populated", neighbourhood(count));
    await page.goto(`${route}&focus=house`);
    await expect(page.locator('[data-object="house"]').first()).toBeVisible();
    await expect(page.locator("[data-lane]")).not.toHaveCount(0);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  });

test("objects are keyboard reachable and recent objects are listed", async ({ page }) => {
  await mockConnections(page);
  await page.goto(`${route}&focus=house`);
  const focus = page.locator('[data-object="house"]').first();
  await focus.focus();
  await page.keyboard.press("Enter");
  await expect(page.getByLabel("Selected object", { exact: true })).toContainText("House blend 1kg");
  await expect(page.getByRole("navigation", { name: "Kinds" })).toContainText("House blend 1kg");
});

for (const width of [1440, 1280, 390])
  test(`table objects remain clickable at ${width}px with a supported-size scene`, async ({ page }) => {
    await page.setViewportSize({ width, height: 720 });
    await mockConnections(page, "populated", neighbourhood(149, 600));
    await quiet(page);
    await page.goto(`${route}&focus=house`);
    await page.getByRole("button", { name: "Show as table" }).click();
    const objects = page.getByRole("table", { name: "Objects in this neighbourhood (150 objects)" });
    for (const name of ["House blend 1kg", "North Coast Beans", "House blend 1kg"]) {
      await objects.getByRole("button", { name, exact: true }).click({ timeout: 5000 });
      await expect(page.getByLabel("Selected object", { exact: true })).toContainText(name);
      await expect(objects.getByRole("button", { name, exact: true })).toHaveAttribute("aria-pressed", "true");
    }
  });

import { test, expect } from "@playwright/test";
import { registerAndOnboard, uniqueEmail } from "./helpers";

test("Help page shows the repo docs, the same ones the MCP docs tool serves", async ({ page }) => {
  await registerAndOnboard(page, uniqueEmail("docs"));
  await page.getByRole("link", { name: "Help", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Quickstart", level: 1 })).toBeVisible();
  // a link to a sibling doc navigates in-app
  await page.getByRole("article").getByRole("link", { name: "Concepts" }).click();
  await expect(page.getByRole("heading", { name: "Concepts", level: 1 })).toBeVisible();
  await page.getByRole("navigation", { name: "Help topics" }).getByRole("button", { name: "AI agents & MCP" }).click();
  await expect(page.getByRole("heading", { name: "AI agents & MCP", level: 1 })).toBeVisible();
  await expect(page.getByRole("cell", { name: "`vault_merge`" }).or(page.locator("td code", { hasText: "vault_merge" })).first()).toBeVisible();
});

test("combining two memory spaces creates a new space and keeps both", async ({ page }) => {
  await registerAndOnboard(page, uniqueEmail("merge"));
  await page.request.post("/api/tools/vault_create", { data: { name: "Alpha" } });
  await page.goto("/vaults");
  await page.getByRole("button", { name: "Advanced: combine two spaces" }).click();
  await page.getByRole("combobox", { name: "First space" }).click();
  await page.getByRole("option", { name: "Personal" }).click();
  await page.getByRole("combobox", { name: "Second space" }).click();
  await page.getByRole("option", { name: "Alpha" }).click();
  await page.getByPlaceholder("New space name (optional)").fill("Combined");
  await page.getByRole("button", { name: "Merge" }).click();
  await expect(page.getByText("Created “Combined”")).toBeVisible();
  for (const name of ["Alpha", "Combined"]) await expect(page.getByText(name, { exact: true }).first()).toBeVisible();
});

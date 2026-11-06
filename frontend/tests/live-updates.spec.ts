// #132: a change made outside the open page (an AI tool over MCP, another tab) shows up without a reload,
// carried by the live stream (/api/events/ws) or, while it is down, by polling.
import { test, expect } from "@playwright/test";
import { registerAndOnboard, uniqueEmail } from "./helpers";

async function tool(page: import("@playwright/test").Page, name: string, args: Record<string, unknown>) {
  const res = await page.request.post(`/api/tools/${name}`, { data: args });
  expect(res.ok(), await res.text()).toBeTruthy();
  return res.json();
}

test("a space created, renamed and deleted by a tool call appears and disappears on an open Vaults page", async ({ page }) => {
  await registerAndOnboard(page, uniqueEmail("live"));
  await page.goto("/vaults");
  await expect(page.getByRole("status").filter({ hasText: "Up to date" })).toBeVisible({ timeout: 15_000 });

  const name = `Agent space ${Date.now()}`;
  const created = await tool(page, "vault_create", { name });
  // no reload: the live stream invalidates the list (well inside the 12 s poll)
  await expect(page.getByRole("button", { name: new RegExp(`^${name}`) })).toBeVisible({ timeout: 5_000 });

  await tool(page, "vault_rename", { vault_id: created.id, name: `${name} renamed` });
  await expect(page.getByRole("button", { name: new RegExp(`^${name} renamed`) })).toBeVisible({ timeout: 5_000 });

  await tool(page, "vault_delete", { vault_id: created.id });
  await expect(page.getByRole("button", { name: new RegExp(`^${name}`) })).toHaveCount(0, { timeout: 5_000 });
});

test("a change in one tab appears in another tab of the same account", async ({ page, context }) => {
  await registerAndOnboard(page, uniqueEmail("tabs"));
  const other = await context.newPage();
  await other.goto("/vaults");
  await expect(other.getByRole("status").filter({ hasText: "Up to date" })).toBeVisible({ timeout: 15_000 });

  const name = `Other tab ${Date.now()}`;
  await tool(page, "vault_create", { name });
  await expect(other.getByRole("button", { name: new RegExp(`^${name}`) })).toBeVisible({ timeout: 5_000 });
});

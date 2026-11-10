import { test, expect, type Page } from "@playwright/test";
import { registerAndOnboard, uniqueEmail } from "./helpers";
import type { TokenCreate } from "../src/lib/gen";

const personal = { id: "vault:personal", name: "Personal", kind: "personal", role: "owner", is_default: true };

async function fixture(page: Page, vaults: "ok" | "failed" | "missing" | "empty" = "ok") {
  const minted: TokenCreate[] = [];
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    let json: unknown = [];
    if (path === "/api/auth/me") json = { id: "user:fixture", email: "fixture@example.test", onboarded: true, is_admin: true, has_password: true };
    else if (path === "/api/auth/bootstrap") json = { has_users: true, auth_mode: "password" };
    else if (path === "/api/settings") json = { theme: {}, model_configured: true, sync_intervals: {} };
    else if (path === "/api/vaults") {
      if (vaults === "failed") return route.fulfill({ status: 503, json: { code: "internal", detail: "Fixture vault query failed" } });
      json = { results: vaults === "missing" ? [] : [{ ...personal, id: vaults === "empty" ? "" : personal.id }] };
    } else if (path === "/api/vaults/invitations") json = { results: [] };
    else if (path === "/api/update/status") json = { configured: false };
    else if (path === "/api/auth/tokens" && route.request().method() === "POST") {
      const body: TokenCreate = route.request().postDataJSON();
      minted.push(body);
      json = { ...body, id: "api_token:fixture", token: "fixture-value" };
    }
    await route.fulfill({ json });
  });
  await page.routeWebSocket("**/api/**", (socket) => socket.close());
  return minted;
}

test("Settings mints Personal, minimal scopes and 90 days by default", async ({ page }) => {
  const minted = await fixture(page);
  await page.goto("/settings?tab=tokens");
  await page.getByLabel("Token name", { exact: true }).fill("Laptop");
  const before = Date.now();
  await page.getByRole("button", { name: "Create token", exact: true }).click();
  await expect.poll(() => minted.length).toBe(1);
  expect(minted[0].vault_id).toBe(personal.id);
  expect(minted[0].scopes).toEqual(["memory:read", "memory:write"]);
  const expiry = Date.parse(minted[0].expires_at ?? "");
  expect(expiry).toBeGreaterThanOrEqual(before + 90 * 86_400_000);
  expect(expiry).toBeLessThanOrEqual(Date.now() + 90 * 86_400_000);
});

for (const vaults of ["failed", "missing", "empty"] as const) {
  test(`Settings blocks minting when Personal is ${vaults}`, async ({ page }) => {
    const minted = await fixture(page, vaults);
    await page.goto("/settings?tab=tokens");
    await page.getByLabel("Token name", { exact: true }).fill("Laptop");
    await expect(page.getByRole("button", { name: "Create token", exact: true })).toBeDisabled();
    await page.getByLabel("Token name", { exact: true }).press("Enter");
    expect(minted).toEqual([]);
  });
}

test("Settings keeps privileged options explicit", async ({ page }) => {
  const minted = await fixture(page);
  await page.goto("/settings?tab=tokens");
  await page.getByLabel("Token name", { exact: true }).fill("Automation");
  await page.getByRole("combobox", { name: "Restrict to a memory space" }).click();
  await page.getByRole("option", { name: "All my memory spaces" }).click();
  await page.getByRole("combobox", { name: "Expiry", exact: true }).click();
  await page.getByRole("option", { name: "Never expires" }).click();
  await page.getByLabel("Manage memory spaces", { exact: false }).check();
  await page.getByLabel("Connected sources", { exact: false }).check();
  await page.getByRole("button", { name: "Create token", exact: true }).click();
  await expect.poll(() => minted.length).toBe(1);
  expect(minted[0].vault_id).toBeNull();
  expect(minted[0].expires_at).toBeNull();
  expect(minted[0].scopes).toEqual(["memory:read", "memory:write", "vaults:admin", "connectors"]);
});

for (const vaults of ["ok", "failed", "empty"] as const) {
  test(`Advanced mint uses safe explicit fields with vault query ${vaults}`, async ({ page }) => {
    const minted = await fixture(page, vaults);
    await page.goto("/connect/claude-code");
    await page.getByText("Advanced: use an API token").click();
    const button = page.getByRole("button", { name: "Generate a token", exact: true });
    if (vaults !== "ok") {
      await expect(button).toBeDisabled();
      expect(minted).toEqual([]);
    } else {
      const before = Date.now();
      await button.click();
      await expect.poll(() => minted.length).toBe(1);
      expect(minted[0].vault_id).toBe(personal.id);
      expect(minted[0].scopes).toEqual(["memory:read", "memory:write"]);
      expect(Date.parse(minted[0].expires_at ?? "")).toBeGreaterThanOrEqual(before + 90 * 86_400_000);
    }
  });
}


test("team-space deletion still requires the exact name", async ({ page }) => {
  await registerAndOnboard(page, uniqueEmail("delete-guard"));
  const name = "Deletion guard";
  const created = await page.request.post("/api/vaults", { data: { name, kind: "org" } });
  expect(created.ok()).toBe(true);
  await page.goto("/vaults");
  await page.getByRole("button", { name: new RegExp(`^${name}`) }).click();
  await page.getByRole("button", { name: "Delete space…", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: `Delete ${name}?` });
  const confirm = dialog.getByRole("button", { name: `Delete ${name}`, exact: true });
  await expect(confirm).toBeDisabled();
  const input = dialog.getByRole("textbox");
  await input.fill(name.toLowerCase());
  await expect(confirm).toBeDisabled();
  await input.fill(name);
  await expect(confirm).toBeEnabled();
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
});

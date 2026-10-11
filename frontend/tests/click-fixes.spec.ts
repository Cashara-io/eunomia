import { test, expect, type Browser, type Page } from "@playwright/test";
import { registerAndOnboard, uniqueEmail } from "./helpers";

const PASSWORD = "correct-horse-battery-staple";

async function tool(page: Page, name: string, args: object) {
  const res = await page.request.post(`/api/tools/${name}`, { data: args });
  expect(res.ok()).toBeTruthy();
  return res.json();
}

async function newUser(browser: Browser, prefix: string): Promise<{ page: Page; email: string }> {
  const page = await (await browser.newContext()).newPage();
  const email = uniqueEmail(prefix);
  await registerAndOnboard(page, email);
  return { page, email };
}

test.describe("Click-through fixes", () => {
  test("a wrong password shows the error with its code", async ({ page, context }) => {
    const email = uniqueEmail("wrongpw");
    await registerAndOnboard(page, email);
    await context.clearCookies();
    await page.goto("/login");
    await page.getByLabel("Email").fill(email);
    await page.getByLabel("Password").fill("not-the-password");
    await page.getByRole("button", { name: "Sign in" }).click();
    await expect(page.getByRole("alert").filter({ hasText: /auth\./ })).toBeVisible();
  });

  test("the page a visitor asked for survives register and onboarding", async ({ browser }) => {
    const page = await (await browser.newContext()).newPage();
    await page.goto("/vaults");
    await page.waitForURL(/\/login\?next=%2Fvaults/);
    await page.getByRole("link", { name: "Create one" }).click();
    await page.waitForURL(/\/register\?next=%2Fvaults/);
    await page.getByLabel("Email").fill(uniqueEmail("next"));
    await page.getByLabel("Password", { exact: true }).fill(PASSWORD);
    await page.getByLabel("Confirm password").fill(PASSWORD);
    await page.getByRole("button", { name: "Create account" }).click();
    await page.waitForURL(/\/onboarding\?next=%2Fvaults/);
    await page.getByRole("button", { name: "Let's go" }).click();
    const skip = page.getByRole("button", { name: "Skip & finish" });
    if (await skip.isVisible({ timeout: 2000 }).catch(() => false)) await skip.click();
    await page.waitForURL((url) => url.pathname === "/vaults");
  });

  test("vaults can be renamed, and blank or taken names are refused", async ({ browser }) => {
    const { page } = await newUser(browser, "rename");
    await page.goto("/vaults");
    const create = async (name: string) => {
      await page.getByRole("button", { name: "New vault" }).click();
      await page.getByLabel(/^Name the vault/).fill(name);
      await page.getByRole("button", { name: "Create", exact: true }).click();
    };
    await create("Alpha");
    await page.getByRole("button", { name: /^Alpha/ }).click();

    await page.getByRole("button", { name: "Rename vault" }).click();
    await page.getByRole("textbox", { name: "Vault name", exact: true }).fill("Beta");
    await page.getByRole("button", { name: "Save", exact: true }).click();
    await expect(page.getByRole("heading", { name: "Beta", level: 2 })).toBeVisible();

    // a duplicate (any case) and the personal vault's own name are taken
    await create("beta");
    await expect(page.getByRole("alert").filter({ hasText: "vault.name_taken" })).toBeVisible();
    await page.getByLabel(/^Name the vault/).fill("Personal");
    await page.getByRole("button", { name: "Create", exact: true }).click();
    await expect(page.getByRole("alert").filter({ hasText: "vault.name_taken" })).toBeVisible();
  });

  test("the sessions list marks this device and warns before revoking it", async ({ browser }) => {
    const { page } = await newUser(browser, "sessions");
    await page.goto("/settings?tab=sessions");
    await expect(page.getByText("This device")).toBeVisible();
    await page.getByRole("button", { name: "Revoke" }).first().click();
    await expect(page.getByText("Signs you out here.")).toBeVisible();
    await page.getByRole("button", { name: "Keep" }).click();
    await expect(page.getByText("Signs you out here.")).toHaveCount(0);
  });

  test("an API token is renamed inline: Enter saves, Esc cancels, a bad name shows the error", async ({ browser }) => {
    const { page } = await newUser(browser, "tokrename");
    expect((await page.request.post("/api/auth/tokens", { data: { name: "laptop" } })).ok()).toBeTruthy();
    await page.goto("/settings?tab=tokens");
    const row = page.getByRole("row").filter({ hasText: "laptop" });

    await row.getByRole("button", { name: "Rename laptop" }).click();
    await page.getByLabel("New token name").fill("work laptop");
    await page.getByLabel("New token name").press("Enter");
    await expect(page.getByRole("button", { name: "Rename work laptop" })).toBeVisible();
    const saved = await (await page.request.get("/api/auth/tokens")).json();
    expect(saved.map((t: { name: string }) => t.name)).toContain("work laptop");

    // clicking the name edits it too; Esc puts it back untouched
    await page.getByRole("button", { name: "work laptop", exact: true }).click();
    await page.getByLabel("New token name").fill("discarded");
    await page.getByLabel("New token name").press("Escape");
    await expect(page.getByRole("button", { name: "Rename work laptop" })).toBeVisible();
    expect((await (await page.request.get("/api/auth/tokens")).json())[0].name).toBe("work laptop");

    // the server's refusal is shown (the field caps typing at 80, so rename a token deleted meanwhile)
    const id = saved[0].id;
    await page.getByRole("button", { name: "Rename work laptop" }).click();
    expect((await page.request.delete(`/api/auth/tokens/${id}`)).ok()).toBeTruthy();
    await page.getByLabel("New token name").fill("gone");
    await page.getByLabel("New token name").press("Enter");
    await expect(page.getByRole("alert").filter({ hasText: "auth.not_found" })).toBeVisible();
  });

  test("chat says up front when no model is configured", async ({ browser }) => {
    const { page } = await newUser(browser, "nomodel");
    const settings = await (await page.request.get("/api/settings")).json();
    test.skip(settings.model_configured, "this stack has a model configured");
    await page.goto("/chat");
    await expect(page.getByText("No model is configured")).toBeVisible();
    await expect(page.getByLabel("Message")).toBeDisabled();
    expect(await page.locator("article").count()).toBe(0);
  });

  test("leaving a vault does not refetch its members (no 403s) and ignores a double click", async ({ browser }) => {
    const owner = await newUser(browser, "leaveowner");
    const bob = await newUser(browser, "leaver");
    const vaultName = `Leave ${Date.now()}`;
    await owner.page.goto("/vaults");
    await owner.page.getByRole("button", { name: "New vault" }).click();
    await owner.page.getByLabel(/^Name the vault/).fill(vaultName);
    await owner.page.getByRole("button", { name: "Create", exact: true }).click();
    await owner.page.getByRole("button", { name: new RegExp(`^${vaultName}`) }).click();
    await owner.page.getByPlaceholder("person@example.com").fill(bob.email);
    await owner.page.getByRole("button", { name: "Invite" }).click();
    await expect(owner.page.getByPlaceholder("person@example.com")).toHaveValue("");

    await bob.page.goto("/vaults");
    await bob.page.locator(".ledger", { hasText: vaultName }).getByRole("button", { name: "Join" }).click();
    await bob.page.getByRole("button", { name: new RegExp(`^${vaultName}`) }).click();

    const forbidden: string[] = [];
    bob.page.on("response", (r) => {
      if (r.status() === 403) forbidden.push(r.url());
    });
    await bob.page.getByRole("button", { name: "Leave this vault" }).click();
    await bob.page.getByRole("button", { name: "Leave", exact: true }).dblclick();
    await expect(bob.page.getByRole("button", { name: new RegExp(`^${vaultName}`) })).toHaveCount(0);
    await bob.page.waitForTimeout(500);
    expect(forbidden).toEqual([]);
  });

  test("Space opens a focused graph node, and deleting an entity confirms inline", async ({ browser }) => {
    const { page } = await newUser(browser, "space");
    await tool(page, "memory_write", { subject_name: "Ada Lovelace", subject_kind: "person", text: "Wrote the first published algorithm." });
    await page.goto("/entities");
    const node = page.getByRole("img", { name: "Entity relationship graph" }).getByRole("button", { name: "Ada Lovelace" });
    await node.focus();
    await page.keyboard.press("Space");
    await expect(page.getByRole("heading", { name: "Ada Lovelace", level: 2 })).toBeVisible();

    page.on("dialog", () => {
      throw new Error("a native confirm dialog opened");
    });
    await page.getByRole("button", { name: "Delete", exact: true }).click();
    await page.getByRole("button", { name: "Delete", exact: true }).click(); // the armed, danger one
    await expect(page.getByRole("heading", { name: "Ada Lovelace", level: 2 })).toHaveCount(0);
  });

  test("a source that is not in the catalogue says it was removed", async ({ browser }) => {
    const { page } = await newUser(browser, "orphan");
    await page.goto("/connectors/long-gone");
    await expect(page.getByText(/This source was removed/)).toBeVisible();
    await expect(page.getByText("Unknown source")).toHaveCount(0);
  });
});

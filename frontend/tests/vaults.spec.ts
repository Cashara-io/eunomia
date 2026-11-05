import { test, expect, type Browser, type Page } from "@playwright/test";
import { registerAndOnboard, uniqueEmail } from "./helpers";

async function newUser(browser: Browser, prefix: string): Promise<{ page: Page; email: string }> {
  const page = await (await browser.newContext()).newPage();
  const email = uniqueEmail(prefix);
  await registerAndOnboard(page, email);
  return { page, email };
}

async function createVaultAndInvite(owner: Page, vaultName: string, inviteeEmail: string) {
  await owner.goto("/vaults");
  await owner.getByRole("button", { name: "Create space" }).click();
  await owner.getByPlaceholder(/^Space name/).fill(vaultName);
  await owner.getByRole("button", { name: "Create", exact: true }).click();

  await owner.getByRole("button", { name: new RegExp(`^${vaultName}`) }).click();
  await owner.getByPlaceholder("person@example.com").fill(inviteeEmail);
  await owner.getByRole("button", { name: "Invite" }).click();
  // The field is cleared only once the invite request succeeds.
  await expect(owner.getByPlaceholder("person@example.com")).toHaveValue("");
}

test.describe("Vault invitations", () => {
  test("an invitee can join from the Vaults page, and only then becomes a member", async ({ browser }) => {
    const owner = await newUser(browser, "owner");
    const invitee = await newUser(browser, "invitee");
    const vaultName = `Team ${Date.now()}`;

    await createVaultAndInvite(owner.page, vaultName, invitee.email);
    // Pending invitees aren't listed as members yet.
    await expect(owner.page.getByText(invitee.email)).toHaveCount(0);

    await invitee.page.goto("/vaults");
    const invite = invitee.page.locator(".ledger", { hasText: vaultName });
    await expect(invite.getByRole("button", { name: "Join", exact: true })).toBeVisible();
    await invite.getByRole("button", { name: "Join", exact: true }).click();

    await expect(invitee.page.getByRole("button", { name: "Join", exact: true })).toHaveCount(0);
    await expect(invitee.page.getByRole("button", { name: new RegExp(`^${vaultName}`) })).toBeVisible();

    await owner.page.reload();
    await owner.page.getByRole("button", { name: new RegExp(`^${vaultName}`) }).click();
    await expect(owner.page.getByText(invitee.email)).toBeVisible();
  });

  test("declining removes the invitation without joining", async ({ browser }) => {
    const owner = await newUser(browser, "owner2");
    const invitee = await newUser(browser, "decliner");
    const vaultName = `Declined ${Date.now()}`;

    await createVaultAndInvite(owner.page, vaultName, invitee.email);

    await invitee.page.goto("/vaults");
    await invitee.page.locator(".ledger", { hasText: vaultName }).getByRole("button", { name: "Decline" }).click();
    await expect(invitee.page.getByText(vaultName)).toHaveCount(0);

    const vaults = await invitee.page.request.get("/api/vaults");
    const names = (await vaults.json()).results.map((v: { name: string }) => v.name);
    expect(names).not.toContain(vaultName);
  });

  test("a pending invitee can't read the vault's members", async ({ browser }) => {
    const owner = await newUser(browser, "owner3");
    const invitee = await newUser(browser, "pending");
    const vaultName = `Locked ${Date.now()}`;

    await createVaultAndInvite(owner.page, vaultName, invitee.email);

    const invites = await invitee.page.request.get("/api/vaults/invitations");
    const { vault_id } = (await invites.json()).results.find((i: { vault_name: string }) => i.vault_name === vaultName);
    const members = await invitee.page.request.get(`/api/vaults/${encodeURIComponent(vault_id)}/members`);
    expect(members.status()).toBe(403);
  });
});

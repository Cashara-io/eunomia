import { test, expect, type Page } from "@playwright/test";
import { uniqueEmail } from "./helpers";

// Settings > People invitations and the /invite page, against the real API. Inviting needs an owner or
// admin of the organisation: the install's first user (E2E_ADMIN_EMAIL / E2E_ADMIN_PASSWORD, as CI's
// e2e-setup.sh registers). Without them the spec skips itself.
const admin = { email: process.env.E2E_ADMIN_EMAIL, password: process.env.E2E_ADMIN_PASSWORD };
const PASSWORD = "correct-horse-battery-staple";

async function signIn(page: Page, email: string, password: string) {
  const res = await page.request.post("/api/auth/login", { data: { email, password } });
  expect(res.status()).toBe(200);
}

/** Invites `email` from Settings > People and returns the link shown once. */
async function inviteFromPeople(page: Page, email: string, role: "member" | "admin" = "member"): Promise<string> {
  await page.goto("/settings?tab=people");
  await page.getByLabel("Email").fill(email);
  await page.getByLabel("Role").selectOption(role);
  await page.getByRole("button", { name: "Invite", exact: true }).click();
  const link = page.getByTestId("invite-link-text");
  await expect(link).toBeVisible();
  const text = (await link.textContent()) ?? "";
  expect(text).toMatch(/\/invite#[A-Za-z0-9_-]{40,}$/);
  return text;
}

test.describe("Organisation invitations", () => {
  test.skip(!admin.email || !admin.password, "set E2E_ADMIN_EMAIL and E2E_ADMIN_PASSWORD to the first user's login");

  test("an admin invites by email and the invitee signs up through the link into the organisation", async ({ page, browser }) => {
    await signIn(page, admin.email!, admin.password!);
    const email = uniqueEmail("invitee");
    const link = await inviteFromPeople(page, email);

    // pending, with its expiry, and the seat count follows
    const row = page.getByTestId(`invitation-${email}`);
    await expect(row).toContainText("expires");
    await expect(page.getByTestId("org-seats")).toContainText("pending");

    // a fresh browser, signed out: the page names the organisation and the invited email, fixed
    const other = await browser.newContext();
    const invitee = await other.newPage();
    await invitee.goto(new URL(link).pathname + new URL(link).hash);
    // first the organisation and the role, and nothing happens until the person says yes
    await expect(invitee.getByRole("heading", { name: /^Join .+\?$/ })).toBeVisible();
    await expect(invitee.getByTestId("invite-summary")).toContainText("as a member");
    // the inviter is named by organisation only: their address was never verified
    await expect(invitee.getByTestId("invite-summary")).toContainText("An admin of");
    await expect(invitee.getByTestId("invite-summary")).not.toContainText(admin.email!);
    await expect(invitee.getByLabel("Password", { exact: true })).toHaveCount(0);
    await invitee.getByRole("button", { name: /^Join / }).click();
    await expect(invitee.getByLabel("Email")).toHaveValue(email);
    // the token left the address bar (it stays in this tab only)
    await expect.poll(() => new URL(invitee.url()).hash).toBe("");
    await invitee.getByLabel("Password", { exact: true }).fill(PASSWORD);
    await invitee.getByLabel("Confirm password").fill(PASSWORD);
    await invitee.getByRole("button", { name: "Create account and join" }).click();
    await invitee.waitForURL(/\/onboarding/);
    const me = await (await invitee.request.get("/api/auth/me")).json();
    expect(me.email).toBe(email);
    expect(me.org_role).toBe("member");
    await other.close();

    // the invitation is used up: it leaves the list, and the link no longer works
    await page.reload();
    await expect(page.getByTestId(`invitation-${email}`)).toHaveCount(0);
    const used = await page.request.post("/api/invitations/lookup", { data: { token: new URL(link).hash.slice(1) } });
    expect(used.status()).toBe(404);
  });

  test("resend makes a new link, revoke withdraws it, and the page says so", async ({ page, browser }) => {
    await signIn(page, admin.email!, admin.password!);
    const email = uniqueEmail("resend");
    const first = await inviteFromPeople(page, email);
    await page.getByRole("button", { name: "Done" }).click();

    const row = page.getByTestId(`invitation-${email}`);
    await row.getByRole("button", { name: "Resend" }).click();
    const second = (await page.getByTestId("invite-link-text").textContent()) ?? "";
    expect(second).not.toBe(first);

    const other = await browser.newContext();
    const visitor = await other.newPage();
    await visitor.goto(new URL(first).pathname + new URL(first).hash);
    await expect(visitor.getByRole("heading", { name: "This link doesn't work" })).toBeVisible();

    await row.getByRole("button", { name: "Revoke" }).click();
    await row.getByRole("button", { name: "Revoke" }).click(); // confirm
    await expect(row).toHaveCount(0);
    const fresh = await other.newPage();
    await fresh.goto(new URL(second).pathname + new URL(second).hash);
    await expect(fresh.getByRole("heading", { name: "This link doesn't work" })).toBeVisible();
    await other.close();
  });

  test("a signed-in account with another email is told the invitation is not for it", async ({ page, browser }) => {
    await signIn(page, admin.email!, admin.password!);
    const link = await inviteFromPeople(page, uniqueEmail("someone"));
    // the admin is signed in with a different email
    await page.goto(new URL(link).pathname + new URL(link).hash);
    await expect(page.getByText(/This invitation is for/)).toBeVisible();
    await expect(page.getByRole("button", { name: /^Sign out and use / })).toBeVisible();
    void browser;
  });
});

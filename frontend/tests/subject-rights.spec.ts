import { expect, test } from "@playwright/test";
import { registerAndOnboard, showAdminSettings, uniqueEmail } from "./helpers";

// Settings > Data: the full export and "Delete my account and data" against the real API,
// and Settings > People's data panel (export for someone, legal hold, delete with their email typed).
// docs/privacy/subject-rights.md

const PASSWORD = "correct-horse-battery-staple";

test("export my data, then delete my account with my password", async ({ page }) => {
  const email = uniqueEmail("erase-me");
  await registerAndOnboard(page, email, PASSWORD);
  await page.goto("/settings?tab=data");

  await page.getByRole("button", { name: "Export my data" }).click();
  const line = page.getByTestId("subject-export").first();
  await expect(line.getByRole("button", { name: "Download" })).toBeVisible({ timeout: 60_000 });
  const download = page.waitForEvent("download");
  await line.getByRole("button", { name: "Download" }).click();
  expect((await download).suggestedFilename()).toBe("eunomia-data-export.zip");

  const zone = page.getByTestId("delete-account");
  await zone.getByRole("button", { name: "Delete my account" }).click();
  await expect(zone.getByText("What you wrote in spaces you share stays for the others")).toBeVisible();
  await zone.getByLabel("Your password").fill("not-the-password");
  await zone.getByRole("button", { name: "Delete my account and data" }).click();
  await expect(zone.getByText("That password isn't right. Nothing was deleted.")).toBeVisible();

  await zone.getByLabel("Your password").fill(PASSWORD);
  await zone.getByRole("button", { name: "Delete my account and data" }).click();
  // the very first account of a fresh stack set up the server and cannot be erased in the app
  const refused = zone.getByText("This is the account that set up the server");
  const erased = page.getByTestId("account-erased");
  await expect(refused.or(erased)).toBeVisible();
  test.skip(await refused.isVisible(), "this stack's first account cannot be erased in the app");
  await expect(page).toHaveURL(/\/login\?erased=1/);
  expect((await page.request.get("/api/auth/me")).status()).toBe(401);

  // signing in again is refused: the account is suspended until the job removes it, then gone
  await page.getByLabel("Email").fill(email);
  await page.getByLabel("Password", { exact: true }).fill(PASSWORD);
  await page.getByRole("button", { name: "Sign in" }).click();
  await expect(page).toHaveURL(/\/login/);
});

test("People: an admin exports, holds and deletes someone's data from their row", async ({ page }) => {
  await registerAndOnboard(page, uniqueEmail("people-admin"), PASSWORD);
  await showAdminSettings(page);
  const person = {
    id: "user:target",
    email: "target@example.com",
    created_at: new Date().toISOString(),
    last_active: null,
    shared_spaces: 1,
    admin: "",
    role: "member",
    suspended_at: null,
    legal_hold_at: null as string | null,
    is_you: false,
    tokens: 0,
  };
  const calls: { method: string; url: string; body: unknown }[] = [];
  await page.route("**/api/admin/people", (route) =>
    route.fulfill({ json: { people: [person], signup: { mode: "invite", summary: "Invite only", allowlist: [], operators: [] } } })
  );
  await page.route("**/api/admin/erasures", (route) => route.fulfill({ json: [] }));
  await page.route("**/api/admin/people/user%3Atarget/**", async (route) => {
    const req = route.request();
    calls.push({ method: req.method(), url: req.url(), body: req.postDataJSON() });
    if (req.url().endsWith("/legal-hold")) {
      const hold = (req.postDataJSON() as { hold: boolean }).hold;
      person.legal_hold_at = hold ? new Date().toISOString() : null;
      return route.fulfill({ json: { legal_hold_at: person.legal_hold_at } });
    }
    if (req.url().endsWith("/erasure")) {
      const typed = (req.postDataJSON() as { confirm_email: string }).confirm_email;
      if (typed !== person.email)
        return route.fulfill({ status: 403, json: { code: "subject.confirmation_failed", detail: "Type the person's email address exactly to confirm. Nothing was deleted." } });
      return route.fulfill({ json: { id: "erasure:x", status: "pending", requested_at: new Date().toISOString() } });
    }
    return route.fulfill({
      json: { id: "subject_export:x", subject: "user:target", requested_by: "user:me", status: "pending", manifest: {}, size_bytes: 0, created_at: new Date().toISOString(), expires_at: new Date().toISOString() },
    });
  });

  await page.setViewportSize({ width: 1280, height: 900 });
  await page.goto("/settings?tab=people");
  const row = page.getByTestId(`person-${person.email}`);
  await row.getByRole("button", { name: `Data of ${person.email}` }).click();
  const panel = page.getByTestId(`person-data-${person.email}`);
  await panel.getByRole("button", { name: "Export their data" }).click();
  await expect(panel.getByText("Export started.")).toBeVisible();

  await panel.getByRole("button", { name: "Place legal hold" }).click();
  await expect(panel.getByText("Legal hold placed.")).toBeVisible();
  await expect(row.getByText("Legal hold")).toBeVisible();
  await expect(panel.getByRole("button", { name: "Delete account and data" })).toBeDisabled();
  await panel.getByRole("button", { name: "Lift legal hold" }).click();
  await expect(panel.getByText("Legal hold lifted.")).toBeVisible();

  await panel.getByRole("button", { name: "Delete account and data" }).click();
  await panel.getByLabel(`Type ${person.email} to confirm`).fill("someone@example.com");
  await panel.getByRole("button", { name: "Delete", exact: true }).click();
  await expect(page.getByText("Type the person's email address exactly to confirm.")).toBeVisible();
  await panel.getByLabel(`Type ${person.email} to confirm`).fill(person.email);
  await panel.getByRole("button", { name: "Delete", exact: true }).click();
  await expect(panel.getByText("Deletion started.")).toBeVisible();

  expect(calls.map((c) => `${c.method} ${new URL(c.url).pathname}`)).toEqual([
    "POST /api/admin/people/user%3Atarget/export",
    "PUT /api/admin/people/user%3Atarget/legal-hold",
    "PUT /api/admin/people/user%3Atarget/legal-hold",
    "POST /api/admin/people/user%3Atarget/erasure",
    "POST /api/admin/people/user%3Atarget/erasure",
  ]);
  await expect(page.getByTestId("erasure-records").getByText("No accounts have been erased.")).toBeVisible();
});

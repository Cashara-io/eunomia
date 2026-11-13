import { createHmac } from "node:crypto";
import { test, expect } from "@playwright/test";
import { registerAndOnboard, uniqueEmail } from "./helpers";

const PASSWORD = "correct-horse-battery-staple";

/** RFC 6238 TOTP (SHA-1, 6 digits, 30 s) for a base32 secret: what the authenticator app would show. */
function totp(secretB32: string, at = Date.now()): string {
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
  let bits = "";
  for (const c of secretB32.replace(/=+$/, "").toUpperCase()) bits += alphabet.indexOf(c).toString(2).padStart(5, "0");
  const key = Buffer.from(bits.match(/.{8}/g)!.map((b) => parseInt(b, 2)));
  const counter = Buffer.alloc(8);
  counter.writeBigUInt64BE(BigInt(Math.floor(at / 1000 / 30)));
  const h = createHmac("sha1", key).update(counter).digest();
  const o = h[h.length - 1] & 0xf;
  return String((h.readUInt32BE(o) & 0x7fffffff) % 1_000_000).padStart(6, "0");
}

test.describe("Two-step verification", () => {
  test("set it up in Settings, then sign in with a code and with a recovery code", async ({ page, context }) => {
    // three sign-ins and a setup: longer than the default 30 s on a cold dev server
    test.setTimeout(90_000);
    const email = uniqueEmail("mfa");
    await registerAndOnboard(page, email, PASSWORD);

    await page.goto("/settings?tab=security");
    await expect(page.getByRole("heading", { name: "Two-step verification" })).toBeVisible();
    await page.getByRole("button", { name: "Set up two-step verification" }).click();
    // a wrong password stops it; the right one shows the QR code
    await page.getByLabel("Your password").fill("not-my-password");
    await page.getByRole("button", { name: "Continue" }).click();
    await expect(page.getByText("That password isn't right.")).toBeVisible();
    await page.getByLabel("Your password").fill(PASSWORD);
    await page.getByRole("button", { name: "Continue" }).click();
    const secret = (await page.getByTestId("mfa-secret").innerText()).trim();
    expect(secret).toMatch(/^[A-Z2-7]{32}$/);
    await expect(page.getByAltText("QR code for your authenticator app")).toBeVisible();

    // a wrong code is refused in words, and nothing changes
    await page.getByLabel("Code from your authenticator app").fill("000000");
    await page.getByRole("button", { name: "Turn on two-step verification" }).click();
    await expect(page.getByTestId("mfa-enrol").getByRole("alert")).toContainText("didn't work");

    await page.getByLabel("Code from your authenticator app").fill(totp(secret));
    await page.getByRole("button", { name: "Turn on two-step verification" }).click();
    const list = page.getByTestId("mfa-recovery-codes");
    await expect(list).toBeVisible();
    const codes = (await list.locator("li").allInnerTexts()).map((c) => c.trim());
    expect(codes).toHaveLength(10);
    await page.getByLabel("I've saved my recovery codes").check();
    await page.getByRole("button", { name: "Done" }).click();
    await expect(page.getByRole("status").filter({ hasText: "On · 10 recovery codes left" })).toBeVisible();
    // the secret is gone from the page for good
    await expect(page.getByTestId("mfa-secret")).toHaveCount(0);

    // sign out, then the password alone is not enough
    await page.goto("about:blank");
    await context.clearCookies();
    await page.goto("/login");
    await page.getByLabel("Email").fill(email);
    await page.getByLabel("Password").fill(PASSWORD);
    await page.getByRole("button", { name: "Sign in" }).click();
    await expect(page.getByRole("heading", { name: "Two-step verification" })).toBeVisible();
    await page.goto("/settings");
    await expect(page).toHaveURL(/\/login/);

    // the second step with the next step's code (the current one was spent at setup)
    await page.getByLabel("Email").fill(email);
    await page.getByLabel("Password").fill(PASSWORD);
    await page.getByRole("button", { name: "Sign in" }).click();
    await page.getByLabel("Code", { exact: true }).fill(totp(secret, Date.now() + 30_000));
    await page.getByRole("button", { name: "Verify" }).click();
    // back where the sign-in started (the redirect kept ?next=/settings)
    await expect(page).toHaveURL((url) => url.pathname === "/settings");

    // and once more with a recovery code
    await page.goto("about:blank");
    await context.clearCookies();
    await page.goto("/login");
    await page.getByLabel("Email").fill(email);
    await page.getByLabel("Password").fill(PASSWORD);
    await page.getByRole("button", { name: "Sign in" }).click();
    await page.getByRole("button", { name: /Use a recovery code/ }).click();
    await page.getByLabel("Recovery code").fill(codes[0]);
    await page.getByRole("button", { name: "Verify" }).click();
    await expect(page).toHaveURL((url) => url.pathname === "/");
    await page.goto("/settings?tab=security");
    await expect(page.getByRole("status").filter({ hasText: "On · 9 recovery codes left" })).toBeVisible();
  });
});

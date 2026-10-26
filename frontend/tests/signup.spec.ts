import { test, expect } from "@playwright/test";
import { registerAndOnboard, uniqueEmail } from "./helpers";

const PASSWORD = "correct-horse-battery-staple";

async function fillRegister(page: import("@playwright/test").Page, email: string, password: string, confirm = password) {
  await page.goto("/register");
  await page.getByLabel("Email").fill(email);
  await page.getByLabel("Password", { exact: true }).fill(password);
  await page.getByLabel("Confirm password").fill(confirm);
  await page.getByRole("button", { name: "Create account" }).click();
}

test.describe("Sign up", () => {
  test("mismatched passwords are rejected without creating an account", async ({ page }) => {
    const email = uniqueEmail("mismatch");
    await fillRegister(page, email, PASSWORD, `${PASSWORD}-typo`);

    await expect(page.getByText("Passwords don't match.")).toBeVisible();
    await expect(page).toHaveURL(/\/register/);

    // Nothing was created: logging in with that email fails.
    const res = await page.request.post("/api/auth/login", { data: { email, password: PASSWORD } });
    expect(res.status()).toBe(401);
  });

  test("the session cookie set by sign up works on the same origin", async ({ page }) => {
    const email = uniqueEmail("cookie");
    await fillRegister(page, email, PASSWORD);
    await page.waitForURL(/\/onboarding/);

    const me = await page.request.get("/api/auth/me");
    expect(me.status()).toBe(200);
    expect((await me.json()).email).toBe(email);
  });

  test("signing up twice with the same email shows a conflict", async ({ page, context }) => {
    const email = uniqueEmail("dupe");
    await registerAndOnboard(page, email, PASSWORD);

    // leave the app first: an open page would react to the dropped cookie by redirecting to /login itself, racing the goto below
    await page.goto("about:blank");
    await context.clearCookies();
    await fillRegister(page, email.toUpperCase(), PASSWORD);
    await expect(page.getByText("A user with that email already exists.")).toBeVisible();
  });

  test("email is case- and whitespace-insensitive at login", async ({ page, context }) => {
    const email = uniqueEmail("casing");
    await registerAndOnboard(page, email, PASSWORD);

    // leave the app first: an open page would react to the dropped cookie by redirecting to /login itself, racing the goto below
    await page.goto("about:blank");
    await context.clearCookies();
    await page.goto("/login");
    await page.getByLabel("Email").fill(`  ${email.toUpperCase()} `);
    await page.getByLabel("Password").fill(PASSWORD);
    await page.getByRole("button", { name: "Sign in" }).click();
    await expect(page).toHaveURL((url) => url.pathname === "/");
  });
});

test.describe("Sign up API validation", () => {
  const cases: Array<[string, { email: string; password: string }, RegExp]> = [
    ["empty password", { email: "x@example.com", password: "" }, /at least 8/],
    ["short password", { email: "x@example.com", password: "1234567" }, /at least 8/],
    ["password over bcrypt's 72-byte limit", { email: "x@example.com", password: "a".repeat(73) }, /at most 72/],
    ["empty email", { email: "", password: PASSWORD }, /valid email/],
    ["not an email", { email: "notanemail", password: PASSWORD }, /valid email/],
  ];

  for (const [name, data, detail] of cases) {
    test(`rejects ${name}`, async ({ request }) => {
      const res = await request.post("/api/auth/register", { data });
      expect(res.status()).toBe(400);
      expect((await res.json()).detail).toMatch(detail);
    });
  }
});

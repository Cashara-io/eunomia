import type { Page } from "@playwright/test";

/** Where in-test mock servers (providers, model endpoints) listen, and how the backend reaches
 * them. Locally the backend runs beside the tests on 127.0.0.1. In CI it runs in a container, so
 * the mocks bind every interface and the backend reaches the host via host.docker.internal. */
export const MOCK_BIND = process.env.E2E_MOCK_BIND ?? "127.0.0.1";
export const mockBase = (port: number) => `http://${process.env.E2E_MOCK_HOST ?? "127.0.0.1"}:${port}`;

export function uniqueEmail(prefix: string): string {
  return `${prefix}-${Date.now()}-${Math.random().toString(36).slice(2)}@example.com`;
}

/** Registers a fresh user through the real UI (not an API shortcut, so the
 * session cookie lands in the browser context the same way a real visitor's
 * would) and clicks through the onboarding wizard, skipping the Connect
 * step and the optional OpenAI-key step, landing on the dashboard. */
export async function registerAndOnboard(page: Page, email: string, password = "correct-horse-battery-staple") {
  await page.goto("/register");
  await page.getByLabel("Email").fill(email);
  await page.getByLabel("Password", { exact: true }).fill(password);
  await page.getByLabel("Confirm password").fill(password);
  await page.getByRole("button", { name: "Create account" }).click();

  await page.waitForURL(/\/onboarding/);
  await page.getByRole("button", { name: "Let's go" }).click();
  await page.getByRole("button", { name: "Skip", exact: true }).click(); // Connect your AI

  // The OpenAI-key step only appears when no key is configured server-side.
  const skipFinish = page.getByRole("button", { name: "Skip & finish" });
  if (await skipFinish.isVisible({ timeout: 2000 }).catch(() => false)) {
    await skipFinish.click();
  }

  await page.waitForURL((url) => url.pathname === "/");
}

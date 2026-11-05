import { test, expect } from "@playwright/test";
import { registerAndOnboard, uniqueEmail } from "./helpers";

test.describe("Home", () => {
  test("a fresh user is asked which AI app they use, with nothing preselected", async ({ page }) => {
    await registerAndOnboard(page, uniqueEmail("home"));

    await expect(page.getByRole("heading", { name: "Home", level: 1 })).toBeVisible();
    await expect(page.getByRole("heading", { name: "Which AI app do you use?" })).toBeVisible();
    for (const app of ["ChatGPT", "Claude Code", "Codex", "Other apps"]) await expect(page.getByRole("link", { name: new RegExp(`^${app}`) })).toBeVisible();
    // Nothing connected yet, so no success is claimed.
    await expect(page.getByText(/is connected$/)).toHaveCount(0);
    await expect(page.getByRole("heading", { name: "Or start with your own information" })).toBeVisible();
    await expect(page.getByRole("link", { name: "Upload a document" })).toHaveAttribute("href", "/documents");

    // Each choice opens that app's own steps; checking before signing in says so honestly.
    await page.getByRole("link", { name: /^Claude Code/ }).click();
    await expect(page.getByRole("heading", { name: "Connect Claude Code", level: 1 })).toBeVisible();
    await expect(page.getByLabel("Command", { exact: true })).toContainText(`claude mcp add --transport http --scope user eunomia ${new URL(page.url()).origin}/mcp`);
    await page.getByRole("button", { name: "Check connection" }).click();
    await expect(page.getByText(/Not connected yet/)).toBeVisible();
    await expect(page.getByRole("heading", { name: "Connection approved" })).toHaveCount(0);

    // A brand-new user has no entities yet; the graph renders its empty state rather than erroring.
    await page.goto("/entities");
    await expect(page.getByText(/No people or topics yet/)).toBeVisible();
  });
});

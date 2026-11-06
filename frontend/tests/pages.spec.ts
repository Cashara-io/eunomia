import { test, expect } from "@playwright/test";
import { registerAndOnboard, showAdminSettings, uniqueEmail } from "./helpers";

// Every page and settings tab renders without an uncaught error or a React
// error in the console.
test("every page and tab renders cleanly", async ({ page }) => {
  test.setTimeout(90_000);
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(`pageerror: ${e.message}`));
  page.on("console", (m) => {
    // a 4xx/5xx shows up as "Failed to load resource" -- some are expected (e.g. chat with no key)
    if (m.type() === "error" && !m.text().startsWith("Failed to load resource")) errors.push(m.text());
  });
  await showAdminSettings(page); // so every Settings group renders
  await registerAndOnboard(page, uniqueEmail("pages"));

  const pages: [string, RegExp | string][] = [
    ["/", "Which AI app do you use?"],
    ["/connect/claude-code", "Copy and run this command"],
    ["/connect/help", "Connection help"],
    ["/chat", "Chat"],
    ["/entities", "People & topics"],
    ["/code", "Code map"],
    ["/vaults", "Memory spaces"],
    ["/connectors", "Connected sources"],
    ["/docs", "Quickstart"],
    ["/skill", "AI instructions"],
    ["/settings", "Settings"],
  ];
  for (const [path, text] of pages) {
    await page.goto(path);
    await expect(page.getByText(text).first(), path).toBeVisible();
  }
  for (const tab of ["Connected apps", "API tokens", "Sessions", "Your data", "People", "Audit log", "Usage", "HTTPS", "Updates", "Backups", "AI provider"]) {
    await page.getByRole("tab", { name: tab }).click();
    await expect(page.getByRole("tab", { name: tab })).toHaveAttribute("aria-selected", "true");
  }
  await page.goto("/entities");
  await page.getByRole("tab", { name: "Memory map" }).click();
  // the Memory map uses the one space picker shared with List and Connections
  await expect(page.getByLabel("Memory space").first()).toBeVisible();
  expect(errors).toEqual([]);
});

test("the sidebar's update badge opens Settings → Updates, from any page including Settings", async ({ page }) => {
  await page.route("**/api/update/status", (route) =>
    route.fulfill({
      json: { configured: true, current_version: "v1.0.0", latest_version: "v9.9.9", update_available: true, checked_at: new Date().toISOString(), applying: false, error: null },
    })
  );
  await showAdminSettings(page); // Updates sits in the admin-only Server group
  await registerAndOnboard(page, uniqueEmail("badge"));
  const badge = page.getByRole("link", { name: /Update available · v9\.9\.9/ });

  await badge.click(); // from Home
  await expect(page.getByRole("tab", { name: "Updates" })).toHaveAttribute("aria-selected", "true");
  await expect(page.getByText("v9.9.9 is available")).toBeVisible();

  await page.getByRole("tab", { name: "Connected apps" }).click();
  await expect(page).toHaveURL(/\/settings$/);
  await badge.click(); // already on Settings: must still switch tabs
  await expect(page.getByRole("tab", { name: "Updates" })).toHaveAttribute("aria-selected", "true");
  await expect(page).toHaveURL(/tab=updates/);
});

import { expect, test, type Page } from "@playwright/test";

// UI contract tests use explicit API fixtures. Backend integration tests cover auth, ZIP and privacy.
async function settings(page: Page, admin = true) {
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const data = path === "/api/auth/me" ? { id: "user:fixture", email: "fixture@example.test", is_admin: admin, onboarded: true }
      : path === "/api/settings" ? { sync_intervals: {}, theme: {}, openai_base_url: "", onboarded: true }
      : path === "/api/update/status" ? { configured: false }
      : path === "/api/secret-requests/pending" ? [] : [];
    await route.fulfill({ json: data });
  });
  await page.goto("/settings?tab=support");
  await expect(page.getByRole("heading", { name: "Diagnostics", exact: true })).toBeVisible();
}

test("admin can inspect inclusions, see progress and download", async ({ page }) => {
  await settings(page);
  let finish!: () => void;
  const waiting = new Promise<void>((resolve) => { finish = resolve; });
  await page.route("**/api/diagnostics", async (route) => {
    await waiting;
    await route.fulfill({ contentType: "application/zip", headers: { "content-disposition": 'attachment; filename="eunomia-diagnostics.zip"' }, body: Buffer.from("PK\x05\x06" + "\0".repeat(18), "binary") });
  });
  await page.getByText("What's included", { exact: true }).click();
  await expect(page.getByText(/No memory, document or chat text/)).toBeVisible();
  await page.getByRole("button", { name: "Download diagnostics", exact: true }).click();
  await expect(page.getByRole("button", { name: "Collecting diagnostics" })).toBeDisabled();
  await expect(page.getByRole("status")).toContainText("waiting for container logs");
  const download = page.waitForEvent("download");
  finish();
  expect((await download).suggestedFilename()).toBe("eunomia-diagnostics.zip");
  await expect(page.getByRole("status")).toContainText("Review the zip");
});

test("member sees who can download", async ({ page }) => {
  await settings(page, false);
  await expect(page.getByRole("button", { name: "Download diagnostics" })).toHaveCount(0);
  await expect(page.getByText(/Only an instance admin can download/)).toBeVisible();
});

test("failure and capped download have clear messages", async ({ page }) => {
  await settings(page);
  await page.route("**/api/diagnostics", (route) => route.fulfill({ status: 500, json: { detail: "Could not collect diagnostics" } }));
  await page.getByRole("button", { name: "Download diagnostics" }).click();
  await expect(page.getByRole("region", { name: "Diagnostics" }).getByRole("alert")).toContainText("Could not collect diagnostics");
  await page.route("**/api/diagnostics", (route) => route.fulfill({ contentType: "application/zip", headers: { "x-diagnostics-capped": "true" }, body: "PK" }));
  await page.getByRole("button", { name: "Download diagnostics" }).click();
  await expect(page.getByRole("status")).toContainText("LIMITS.txt");
});

for (const theme of ["light", "dark"]) {
  for (const width of [1440, 375]) {
    test(`card ${theme} at ${width}px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 1000 });
      await page.addInitScript((theme) => localStorage.setItem("eunomia-theme", theme), theme);
      await settings(page);
      await page.getByText("What's included", { exact: true }).click();
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await page.screenshot({ path: `test-results/diagnostics-${theme}-${width}.png`, fullPage: true });
    });
  }
}

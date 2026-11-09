import { expect, test } from "@playwright/test";
import fs from "node:fs";
import { z } from "zod";
import { registerAndOnboard, uniqueEmail } from "./helpers";

test.use({ launchOptions: { ignoreDefaultArgs: ["--disable-popup-blocking"] } });

test("Composio consumer sign-in, selected accounts, partial sync and disconnect", async ({ page }) => {
  test.setTimeout(90000);
  test.skip(process.env.E2E_COMPOSIO_MOCK !== "1", "Requires the isolated consumer MCP mock stack.");
  await registerAndOnboard(page, uniqueEmail("composio"));
  await page.goto("/connectors");
  const card = page.getByRole("region", { name: "Composio", exact: true });
  await card.getByRole("button", { name: "Connect Composio", exact: true }).click();
  await expect(card.getByText("Signed in as mock-person@example.test")).toBeVisible({ timeout: 30000 });
  await card.getByRole("checkbox", { name: "Sync Gmail: Fixture gmail", exact: true }).check();
  await card.getByRole("checkbox", { name: "Sync Google Drive: Fixture googledrive", exact: true }).check();
  await card.getByRole("checkbox", { name: "Sync Google Drive: Second Drive", exact: true }).check();
  await card.getByRole("button", { name: "Save app choices" }).click();
  await expect(card.getByText("App choices saved.")).toBeVisible();
  let hideFirstProgress = true;
  const sourceRows = z.array(z.object({ key: z.string(), sync_status: z.object({ last_report: z.object({ status: z.string().optional() }).passthrough().nullable().optional() }).passthrough() }).passthrough());
  await page.route("**/api/sources", async (route) => {
    const response = await route.fetch();
    const rows = sourceRows.parse(await response.json());
    const json = rows.map((row) => {
      if (hideFirstProgress && row.key === "composio" && row.sync_status.last_report?.status === "partial") {
        hideFirstProgress = false;
        return { ...row, sync_status: { ...row.sync_status, last_report: {}, last_error: "", last_ok: null } };
      }
      return row;
    });
    await route.fulfill({ response, json });
  });
  await card.getByRole("button", { name: "Sync now", exact: true }).click();
  await expect(card.getByText(/rate limited, \d+ imported, more remaining/)).toBeVisible({ timeout: 45000 });
  const evidence = process.env.E2E_COMPOSIO_SCREENSHOTS;
  if (evidence) {
    fs.mkdirSync(evidence, { recursive: true });
    await card.screenshot({ path: `${evidence}/partial-light.png`, animations: "disabled" });
    await page.evaluate(() => { document.documentElement.dataset.theme = "dark"; });
    await card.screenshot({ path: `${evidence}/partial-dark.png`, animations: "disabled" });
    await page.setViewportSize({ width: 390, height: 844 });
    await page.evaluate(() => window.scrollTo(0, 0));
    await page.screenshot({ path: `${evidence}/partial-mobile-dark.png`, fullPage: false, animations: "disabled" });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    await page.setViewportSize({ width: 1280, height: 720 });
  }
  await card.getByRole("button", { name: "Sync now", exact: true }).click();
  await expect(card.getByText(/Healthy\. Last completed sync:/)).toBeVisible({ timeout: 45000 });
  await expect(card).toContainText("TimelinesAI");
  await expect(card).toContainText("delivery history");
  await page.route("**/api/connectors/composio/manage", async (route) => {
    const response = await route.fetch();
    await new Promise((resolve) => setTimeout(resolve, 5500));
    await route.fulfill({ response });
  });
  const [popup] = await Promise.all([page.waitForEvent("popup"), card.getByRole("button", { name: "Connect another app" }).click()]);
  await expect(popup).toHaveURL(/authorise-app/, { timeout: 15000 }); await popup.close();
  let removeDrive = false;
  await page.route("**/api/connectors/composio/status", async (route) => {
    const response = await route.fetch();
    const value = z.object({ accounts: z.array(z.object({ id: z.string(), status: z.string() }).passthrough()) }).passthrough().parse(await response.json());
    await route.fulfill({ response, json: { ...value, accounts: value.accounts.filter((a) => !removeDrive || a.id !== "googledrive-mine").map((a) => a.id === "googledrive-second" ? { ...a, status: "EXPIRED" } : a) } });
  });
  await card.getByRole("button", { name: "Refresh apps", exact: true }).click();
  const expired = card.getByRole("checkbox", { name: "Sync Google Drive: Second Drive", exact: true });
  await expect(card.getByText("EXPIRED", { exact: true })).toBeVisible();
  await expect(expired).toBeEnabled();
  await expired.uncheck();
  await card.getByRole("button", { name: "Save app choices" }).click();
  await expect(card.getByText("App choices saved.")).toBeVisible();
  removeDrive = true;
  await card.getByRole("button", { name: "Refresh apps", exact: true }).click();
  const removed = card.getByRole("checkbox", { name: "Sync Google Drive: Previously selected account 1", exact: true });
  await expect(card.getByText("Unavailable", { exact: true })).toBeVisible();
  await expect(removed).toBeEnabled();
  await removed.click();
  await expect(removed).toHaveCount(0);
  await card.getByRole("button", { name: "Save app choices" }).click();
  await expect(card.getByText("App choices saved.")).toBeVisible();
  await card.getByRole("button", { name: "Disconnect", exact: true }).click();
  await expect(card.getByRole("button", { name: "Connect Composio", exact: true })).toBeVisible();
  await expect(card.getByRole("status")).toHaveText(/tokens were revoked and deleted/);
  const response = await page.request.get("/api/connectors/composio/status");
  expect((await response.json()).connected).toBe(false);
});

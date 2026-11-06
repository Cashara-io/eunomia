import { test, expect } from "@playwright/test";
import { registerAndOnboard, uniqueEmail } from "./helpers";

test.describe("Connectors", () => {
  test("saving a connector's credentials marks it connected", async ({ page }) => {
    await registerAndOnboard(page, uniqueEmail("connectors"));
    await page.goto("/connectors");

    // Not connected yet: listed under "Add a source" as a "Connect Up Bank" row.
    const upBankCard = page.getByRole("link", { name: "Connect Up Bank" });
    await expect(upBankCard).toBeVisible();
    await upBankCard.click();

    await page.getByPlaceholder("up:yeah:…").fill("up:yeah:e2e-fake-token");
    await page.getByRole("button", { name: "Save", exact: true }).click();
    await expect(page.getByRole("button", { name: "Saved" })).toBeVisible();

    // Reflected on the list after a fresh load, not just optimistic local state.
    await page.goto("/connectors");
    await expect(page.getByRole("link", { name: /Up Bank/ }).getByText("Connected", { exact: true })).toBeVisible();
  });
});

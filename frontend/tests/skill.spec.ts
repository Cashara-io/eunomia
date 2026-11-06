import { test, expect } from "@playwright/test";
import { registerAndOnboard, uniqueEmail } from "./helpers";

test("the memory skill is viewable, editable and resettable, and edits reach MCP clients", async ({ page }) => {
  await registerAndOnboard(page, uniqueEmail("skill"));
  const token = (await (await page.request.post("/api/auth/tokens", { data: { name: "skill" } })).json()).token;
  const instructions = async () =>
    (
      await (
        await page.request.post("/mcp", {
          headers: { Authorization: `Bearer ${token}` },
          data: { jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: "2025-06-18" } },
        })
      ).json()
    ).result.instructions as string;

  expect(await instructions()).toContain("# Eunomia memory");

  // AI instructions is under Advanced, which the sidebar shows to admins; go there directly
  await page.goto("/skill");
  await expect(page.getByRole("heading", { name: "Eunomia memory", level: 1 })).toBeVisible();
  await expect(page.getByLabel("Skill source")).toHaveText("Built-in default");

  await page.getByRole("button", { name: "Edit", exact: true }).click();
  const box = page.getByLabel("Skill markdown");
  await expect(box).toHaveValue(/^---\nname: eunomia-memory/);
  await box.fill((await box.inputValue()) + "\n## House rules\n\nAlways remember project deadlines.\n");
  await page.getByRole("button", { name: "Save" }).click();
  await expect(page.getByRole("status")).toContainText("Saved");
  await expect(page.getByLabel("Skill source")).toHaveText("Customised");
  await expect(page.getByRole("heading", { name: "House rules" })).toBeVisible();
  expect(await instructions()).toContain("Always remember project deadlines.");

  await page.getByRole("button", { name: "Reset to default" }).click();
  await page.getByRole("button", { name: "Replace with built-in" }).click();
  await expect(page.getByLabel("Skill source")).toHaveText("Built-in default");
  expect(await instructions()).not.toContain("House rules");
});

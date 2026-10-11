import { test, expect, type Page } from "@playwright/test";
import { uniqueEmail } from "./helpers";

const PASSWORD = "correct-horse-battery-staple";

async function registerToConnectStep(page: Page) {
  await page.goto("/register");
  await page.getByLabel("Email").fill(uniqueEmail("connect"));
  await page.getByLabel("Password", { exact: true }).fill(PASSWORD);
  await page.getByLabel("Confirm password").fill(PASSWORD);
  await page.getByRole("button", { name: "Create account" }).click();
  await page.waitForURL(/\/onboarding/);
  await page.getByRole("button", { name: "Let's go" }).click();
  await expect(page.getByRole("heading", { name: "Connect your AI" })).toBeVisible();
}

const tokenNames = async (page: Page) =>
  ((await (await page.request.get("/api/auth/tokens")).json()) as { name: string }[]).map((t) => t.name);

test.describe("Onboarding: Connect your AI", () => {
  test.use({ permissions: ["clipboard-read", "clipboard-write"] });

  test("each client gets its own token and a ready-to-paste setup with this URL", async ({ page }) => {
    await registerToConnectStep(page);
    const mcpUrl = `${new URL(page.url()).origin}/mcp`;
    const panel = page.getByRole("tabpanel");

    const cases = [
      { tab: "Claude Code", token: "Claude Code", label: "Command", copy: "Copy command" },
      { tab: "Codex", token: "Codex", label: "Config", copy: "Copy config" },
      { tab: "Other", token: "MCP", label: "Config", copy: "Copy config" },
    ];
    for (const c of cases) {
      await page.getByRole("tab", { name: c.tab }).click();
      await expect(page.getByRole("tab", { name: c.tab })).toHaveAttribute("aria-selected", "true");
      await panel.getByRole("button", { name: "Generate a token" }).click();

      const snippet = panel.getByLabel(c.label, { exact: true });
      await expect(snippet).toContainText(mcpUrl);
      const text = (await snippet.textContent()) ?? "";
      const token = /Bearer ([A-Za-z0-9_-]+)/.exec(text)?.[1];
      expect(token, `${c.tab} snippet carries the token`).toBeTruthy();
      if (c.tab === "Claude Code")
        expect(text).toBe(`claude mcp add --transport http --scope user eunomia ${mcpUrl} --header "Authorization: Bearer ${token}"`);
      if (c.tab === "Codex") expect(text).toContain("[mcp_servers.eunomia]");

      await panel.getByRole("button", { name: c.copy }).click();
      expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(text);
      await panel.getByRole("button", { name: "Copy token" }).click();
      expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(token);

      // The minted token really works against /mcp.
      const init = await page.request.post("/mcp", {
        headers: { Accept: "application/json, text/event-stream", Authorization: `Bearer ${token}` },
        data: { jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "e2e", version: "1" } } },
      });
      expect(init.status()).toBe(200);
    }
    expect(await tokenNames(page)).toEqual(expect.arrayContaining(["Claude Code", "Codex", "MCP"]));

    // ChatGPT signs in with OAuth: no token, the URL to paste, and on localhost an honest warning.
    await page.getByRole("tab", { name: "ChatGPT" }).click();
    await expect(panel.getByRole("button", { name: "Generate a token" })).toHaveCount(0);
    await expect(panel.getByText(mcpUrl, { exact: true })).toBeVisible();
    await panel.getByRole("button", { name: "Copy URL" }).click();
    expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(mcpUrl);
    if (!mcpUrl.startsWith("https://")) await expect(panel.getByText(/needs a public https:\/\/ address/)).toBeVisible();

    // Tabs are a real tablist: arrows move the selection.
    await page.getByRole("tab", { name: "ChatGPT" }).press("ArrowLeft");
    await expect(page.getByRole("tab", { name: "Codex" })).toHaveAttribute("aria-selected", "true");
    // A token minted here survives the trip there and back.
    await expect(panel.getByLabel("Config", { exact: true })).toContainText("Bearer ");

    // Once something is connected, Skip becomes Continue.
    await page.getByRole("button", { name: /^(Continue|Finish)$/ }).click();
    await expect(page.getByRole("heading", { name: "Connect your AI" })).toBeHidden();
  });

  test("skip leaves without minting a token", async ({ page }) => {
    await registerToConnectStep(page);
    await page.getByRole("button", { name: "Skip", exact: true }).click();
    await expect(page.getByRole("heading", { name: "Connect your AI" })).toBeHidden();
    const skipFinish = page.getByRole("button", { name: "Skip & finish" });
    if (await skipFinish.isVisible({ timeout: 2000 }).catch(() => false)) await skipFinish.click();
    await page.waitForURL((url) => url.pathname === "/");
    expect(await tokenNames(page)).toEqual([]);

    // The same guides live on the dashboard afterwards.
    const card = page.locator("#connect");
    await card.getByRole("tab", { name: "Codex" }).click();
    await card.getByRole("button", { name: "Generate a token" }).click();
    await expect(card.getByLabel("Config", { exact: true })).toContainText(`${new URL(page.url()).origin}/mcp`);
    expect(await tokenNames(page)).toEqual(["Codex"]);
  });
});

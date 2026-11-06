import { test, expect, type Page } from "@playwright/test";
import { showAdminSettings, uniqueEmail } from "./helpers";

const PASSWORD = "correct-horse-battery-staple";

/** `admin` shows the admin-only token method (members connect through an app's guide). */
async function registerToConnectStep(page: Page, { admin = false } = {}) {
  if (admin) await showAdminSettings(page);
  await page.goto("/register");
  await page.getByLabel("Email").fill(uniqueEmail("connect"));
  await page.getByLabel("Password", { exact: true }).fill(PASSWORD);
  await page.getByLabel("Confirm password").fill(PASSWORD);
  await page.getByRole("button", { name: "Create account" }).click();
  await page.waitForURL(/\/onboarding/);
  await expect(page.getByRole("heading", { name: "Connect your AI app" })).toBeVisible();
}

const tokenNames = async (page: Page) =>
  ((await (await page.request.get("/api/auth/tokens")).json()) as { name: string }[]).map((t) => t.name);

test.describe("Onboarding: Connect your AI", () => {
  test.use({ permissions: ["clipboard-read", "clipboard-write"] });

  test("lands on the app picker with nothing preselected, and an app opens its own guide", async ({ page }) => {
    await registerToConnectStep(page, { admin: true });
    for (const app of ["ChatGPT", "Claude Code", "Codex", "Other apps"]) await expect(page.getByRole("link", { name: new RegExp(`^${app}`) })).toBeVisible();
    // The token method waits behind the Advanced disclosure, with no app chosen.
    await expect(page.getByRole("button", { name: "Generate a token" })).toBeHidden();
    await page.getByText("Advanced: use an API token").click();
    await expect(page.getByRole("tab", { selected: true })).toHaveCount(0);
    await page.getByText("Advanced: use an API token").click();

    await page.getByRole("link", { name: /^Claude Code/ }).click();
    await expect(page.getByRole("heading", { name: "Connect Claude Code", level: 1 })).toBeVisible();
    // Choosing an app finished setup: the guide stays put rather than bouncing back to onboarding.
    expect((await (await page.request.get("/api/auth/me")).json()).onboarded).toBe(true);
  });

  test("each client gets its own token and a ready-to-paste setup with this URL", async ({ page }) => {
    await registerToConnectStep(page, { admin: true });
    const mcpUrl = `${new URL(page.url()).origin}/mcp`;
    await page.getByText("Advanced: use an API token").click();
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

    // Tabs are a real tablist: arrows move the selection.
    // ChatGPT can only sign in, so it has no token tab.
    await expect(page.getByRole("tab", { name: "ChatGPT" })).toHaveCount(0);
    await page.getByRole("tab", { name: "Other" }).press("ArrowLeft");
    await expect(page.getByRole("tab", { name: "Codex" })).toHaveAttribute("aria-selected", "true");
    // A token minted here survives the trip there and back.
    await expect(panel.getByLabel("Config", { exact: true })).toContainText("Bearer ");

    // Once something is connected, a Continue (or Finish) appears beside the one "Set up later" exit.
    await page.getByRole("button", { name: /^(Continue|Finish)$/ }).click();
    await expect(page.getByRole("heading", { name: "Connect your AI app" })).toBeHidden();
  });

  test("set up later leaves without minting a token, and members see no token method", async ({ page }) => {
    await registerToConnectStep(page);
    await expect(page.getByText("Advanced: use an API token")).toHaveCount(0);
    await page.getByRole("button", { name: "Set up later" }).click();
    await expect(page.getByRole("heading", { name: "Connect your AI app" })).toBeHidden();
    const skipFinish = page.getByRole("button", { name: "Skip & finish" });
    if (await skipFinish.isVisible({ timeout: 2000 }).catch(() => false)) await skipFinish.click();
    await page.waitForURL((url) => url.pathname === "/");
    expect(await tokenNames(page)).toEqual([]);

    // The same token guides live on Home's connection steps afterwards, as the advanced option.
    await page.getByRole("link", { name: /^Codex/ }).click();
    await page.getByText("Advanced: use an API token").click();
    const card = page.getByRole("tabpanel");
    await expect(page.getByRole("tab", { name: "Codex" })).toHaveAttribute("aria-selected", "true");
    await card.getByRole("button", { name: "Generate a token" }).click();
    await expect(card.getByLabel("Config", { exact: true })).toContainText(`${new URL(page.url()).origin}/mcp`);
    expect(await tokenNames(page)).toEqual(["Codex"]);
  });
});

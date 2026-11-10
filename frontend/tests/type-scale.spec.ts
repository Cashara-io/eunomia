import { test, expect, type Page } from "@playwright/test";
import { mkdir } from "node:fs/promises";
import type { AppSettings, Connector } from "../src/lib/types";
import type { DocumentDetail, DocumentOut } from "../src/lib/gen";

const settings: AppSettings = {
  theme: {}, embedding_model: "text-embedding-3-small", chat_model: "",
  openai_base_url: "https://api.openai.com/v1", openai_api_key_set: false,
  model_configured: true, sync_intervals: {}, sync_check_connectivity: true,
  observations_mission: "", memory_skill: "# AI instructions\n\nRemember project decisions.", memory_skill_custom: false,
};
const space = { id: "vault:personal", name: "Personal", kind: "personal", role: "owner", is_default: true };
const connector: Connector = {
  id: "connector:github", kind: "github", label: "Studio", source_key: "github",
  enabled: true, credentials_set: true, config: {}, updated_at: null,
  routing: { rules: [] }, routing_fields: [],
};
const sampleDocument: DocumentOut = {
  id: "document:notes", vault: space.id, filename: "field-notes.md", media_type: "text/markdown",
  size_bytes: 240, status: "ready", chunk_count: 2, revision: 1,
  sha256: "1234567890abcdef".repeat(4), created_at: "2026-11-10T00:00:00Z", updated_at: "2026-11-10T00:00:00Z",
  download_url: "/api/documents/document%3Anotes/download",
};
const text = "The studio agreed to keep one upload action.\n\nPassages stay readable without a second frame.";
const detail: DocumentDetail = {
  document: sampleDocument, text, text_truncated: false,
  chunks: [0, 44].map((start, part) => ({ chunk_id: `chunk:${part}`, part, char_start: start, char_end: part ? text.length : 44, has_embedding: true })),
};

async function fixture(page: Page, { onboarded = true, storage = true } = {}) {
  await page.routeWebSocket(/\/api\/events\/ws/, (socket) => socket.close());
  await page.route("**/api/**", async (route) => {
    const path = decodeURIComponent(new URL(route.request().url()).pathname);
    let json: unknown = [];
    if (path === "/api/auth/me") json = { id: "fixture", email: "fixture@example.test", onboarded, is_admin: true, has_password: true };
    else if (path === "/api/auth/bootstrap") json = { has_users: true };
    else if (path === "/api/settings") json = settings;
    else if (path === "/api/settings/openai-models") json = { models: [], error: null };
    else if (path === "/api/update/status") json = { configured: false };
    else if (path === "/api/vaults") json = { results: [space] };
    else if (path === "/api/vaults/invitations") json = { results: [] };
    else if (path === "/api/documents" && route.request().method() === "POST") json = sampleDocument;
    else if (path === "/api/auth/tokens" && route.request().method() === "POST") json = { token: "fixture-token", id: "token:fixture" };
    else if (path === "/api/documents") json = { results: [sampleDocument], total: 1, has_more: false, storage, max_bytes: 10000000, media_types: ["text/markdown"] };
    else if (path === `/api/documents/${sampleDocument.id}`) json = detail;
    else if (path === "/api/connectors") json = [connector];
    else if (path === "/api/connectors/composio/status") json = { connected: false, account: "", apps: [], accounts: [], vault: null, enabled: false, health: null };
    else if (path.endsWith("/ontology")) json = { vault: space.id, strict: false, kinds: [], links: [] };
    else if (path.endsWith("/readiness")) json = { ready: true, blockers: [], kinds: {}, properties: {}, links: {}, metrics: {} };
    else if (path === "/api/entities/cloud") json = { points: [], links: [], axes: [], space: "semantic", total: 0 };
    else if (path === "/api/entities/graph") json = { nodes: [], edges: [] };
    else if (path === "/api/entities") json = { results: [], total: 0, has_more: false };
    else if (path.endsWith("/members")) json = { results: [] };
    else if (path === "/api/secrets/me") json = { manage_providers: false, manage_refs: false, vaults: [] };
    else if (path === "/api/secrets/requests") json = [];
    else if (path === "/api/secrets/requests/fixture") json = {
      id: "fixture", label: "Studio API key", name: "studio_key", description: "Read project updates.",
      status: "pending", can_fill: true, target_hosts: ["api.example.test"], placements: ["header"],
      vault_id: space.id, handle: "secret:studio_key", actor_kind: "token", client_name: "Claude Code", channel: "mcp",
      created_at: "2026-11-10T00:00:00Z", expires_at: "2026-11-11T00:00:00Z",
    };
    else if (path === "/api/oauth/consent") json = { client: { name: "Fixture app" }, scopes: [{ scope: "memory:read", description: "Read memory" }], loopback: false, redirect_host: "example.test", user_email: "fixture@example.test" };
    else if (path === "/api/sources") json = [{ key: "github", label: "GitHub", connector: connector.id, provider: "github", record_types: [], connected: true, record_count: 0, sync_status: { cursor: "", last_run: null, last_ok: null, last_error: "", consecutive_failures: 0 } }];
    else if (path === "/api/tools/docs") json = { docs: [] };
    else if (path === "/api/tools/list") json = { results: [], total: 0, has_more: false };
    else if (path === "/api/tools/metrics") json = { total_records: 0, sources: [] };
    await route.fulfill({ json });
  });
}

const surfaces = [
  ["documents", "/documents?id=document%3Anotes"],
  ["provider", "/settings?tab=model"],
  ["data", "/settings?tab=data"],
  ["connectors", "/connectors"],
  ["connector-setup", "/connectors/setup/github"],
  ["connector-detail", "/connectors/github"],
  ["onboarding", "/onboarding"],
  ["onboarding-key", "/onboarding"],
  ["secret-request", "/secrets/requests/fixture"],
  ["chat", "/chat"],
  ["vaults", "/vaults"],
  ["memory-map", "/entities?view=map"],
  ["help", "/docs"],
  ["skill", "/skill"],
  ["connect-guide", "/connect/claude-code"],
  ["connect-help", "/connect/help"],
  ["login", "/login"],
  ["register", "/register"],
  ["consent", "/consent?client_id=fixture&redirect_uri=https%3A%2F%2Fexample.test&response_type=code&scope=memory:read"],
] as const;

for (const width of [1440, 390]) {
  for (const theme of ["light", "dark"] as const) {
    test(`readable layouts at ${width} in ${theme}`, async ({ page }) => {
      test.setTimeout(180_000);
      const errors: string[] = [];
      page.on("pageerror", (error) => errors.push(error.message));
      await page.setViewportSize({ width, height: 1000 });
      await page.addInitScript((mode) => localStorage.setItem("eunomia-theme", mode), theme);
      await fixture(page);
      for (const [name, url] of surfaces.filter(([name]) => !process.env.TYPE_SCALE_ONLY || process.env.TYPE_SCALE_ONLY.split(",").includes(name))) {
        // Onboarding and sign-in screens need their own session state.
        await page.route("**/api/auth/me", (route) => route.fulfill({ json: {
          id: "fixture", email: "fixture@example.test", onboarded: !name.startsWith("onboarding"), is_admin: true, has_password: true,
        } }));
        await page.goto(url);
        await expect(page.locator("h1:visible").first(), name).toBeVisible();
        await page.waitForLoadState("networkidle");
        await page.evaluate(() => document.fonts.ready);
        if (name === "onboarding-key") {
          await page.getByText("Advanced: use an API token", { exact: true }).click();
          await page.getByRole("tab", { name: "Other", exact: true }).click();
          await page.getByRole("button", { name: "Generate a token", exact: true }).click();
          await page.getByRole("button", { name: "Continue", exact: true }).click();
          await expect(page.getByRole("heading", { name: "Add an OpenAI key" })).toBeVisible();
        }
        if (name === "connectors") {
          await page.getByText("Advanced: custom integrations", { exact: true }).click();
          await page.getByRole("button", { name: "New extension", exact: true }).click();
        }
        if (process.env.TYPE_SCALE_SHOTS) {
          await mkdir(process.env.TYPE_SCALE_SHOTS, { recursive: true });
          await page.screenshot({ path: `${process.env.TYPE_SCALE_SHOTS}/${name}-${width}-${theme}.png`, fullPage: true });
        }
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), name).toBe(true);
        const headings = await page.locator("h1:visible, h2:visible, h3:visible").evaluateAll((nodes) => nodes.map((node) => Number.parseFloat(getComputedStyle(node).fontSize)));
        expect(headings.every((size) => size >= 13), name).toBe(true);
      }
      expect(errors).toEqual([]);
    });
  }
}

test("one primary upload, keyboard picker, drag and drop, and unframed passages", async ({ page }) => {
  await fixture(page);
  await page.goto("/documents");
  // The header exists before hydration. Wait for the client-loaded fixture before using the keyboard.
  await expect(page.getByTestId("document-row")).toHaveCount(1);
  await expect(page.locator(".btn-primary")).toHaveCount(1);
  const upload = page.getByRole("button", { name: "Upload", exact: true });
  const successfulPicker = page.waitForEvent("filechooser");
  await upload.press("Enter");
  const uploaded = page.waitForRequest((request) => request.method() === "POST" && new URL(request.url()).pathname === "/api/documents");
  await (await successfulPicker).setFiles({ name: "field-notes.md", mimeType: "text/markdown", buffer: Buffer.from(text) });
  expect((await uploaded).postData()).toBe(text);
  await expect(page).toHaveURL(/id=document%3Anotes/);
  await expect(page.getByTestId("document-passage")).toHaveCount(2);
  await expect(page.getByTestId("document-status")).toHaveText("Ready");
  const picker = page.waitForEvent("filechooser");
  await upload.press("Enter");
  await (await picker).setFiles({ name: "installer.exe", mimeType: "application/octet-stream", buffer: Buffer.from("MZ") });
  await expect(page.getByRole("alert").filter({ hasText: "installer.exe is not a supported type" })).toBeVisible();
  const passages = page.getByTestId("document-panel").locator("ol");
  expect(await passages.evaluate((node) => getComputedStyle(node).borderTopWidth)).toBe("0px");
  const transfer = await page.evaluateHandle(() => {
    const data = new DataTransfer();
    data.items.add(new File(["MZ"], "dropped.exe", { type: "application/octet-stream" }));
    return data;
  });
  await page.getByTestId("document-drop-zone").dispatchEvent("drop", { dataTransfer: transfer });
  await expect(page.getByRole("alert").filter({ hasText: "dropped.exe is not a supported type" })).toBeVisible();
});

test("disabled storage has no upload prompt", async ({ page }) => {
  await fixture(page, { storage: false });
  await page.goto("/documents");
  await expect(page.getByTestId("documents-storage-off")).toBeVisible();
  await expect(page.getByRole("button", { name: "Upload", exact: true })).toHaveCount(0);
  await expect(page.getByTestId("document-drop-zone")).toHaveCount(0);
});


test("short intros keep setup detail in Help and use readable shared headings", async ({ page }) => {
  await fixture(page);
  await page.goto("/settings?tab=model");
  await expect(page.getByText("Add an OpenAI-compatible provider for meaning search and in-app chat.")).toBeVisible();
  expect(await page.locator(".page-title").evaluate((node) => getComputedStyle(node).fontSize)).toBe("22px");
  expect(await page.locator(".section-title").first().evaluate((node) => getComputedStyle(node).fontSize)).toBe("16px");
  await page.getByRole("link", { name: "About AI providers" }).click();
  await expect(page.getByRole("heading", { name: "AI provider", exact: true })).toBeVisible();
  await expect(page.getByText(/An AI provider is optional/)).toBeVisible();
});

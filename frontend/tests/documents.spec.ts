import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { test, expect, type APIRequestContext, type Page } from "@playwright/test";
import { registerAndOnboard, uniqueEmail } from "./helpers";

// Uploaded documents end to end against the running stack: the original goes into the database's file
// bucket, the indexing job turns its text into passages that search and recall find, and every hit
// points back at the file. Needs a stack whose SurrealDB allows the experimental file buckets
// (docker-compose.yml does).

const sha256 = (b: Buffer) => createHash("sha256").update(b).digest("hex");

async function newToken(request: APIRequestContext): Promise<string> {
  const reg = await request.post("/api/auth/register", { data: { email: uniqueEmail("docs"), password: "correct-horse-battery-staple" } });
  expect(reg.ok()).toBeTruthy();
  return (await (await request.post("/api/auth/tokens", { data: { name: "MCP" } })).json()).token;
}

/** One MCP tools/call through the frontend's /mcp proxy; returns the tool's JSON result. */
async function tool(request: APIRequestContext, token: string, name: string, args: object) {
  const res = await request.post("/mcp", {
    headers: { Accept: "application/json, text/event-stream", Authorization: `Bearer ${token}` },
    data: { jsonrpc: "2.0", id: 1, method: "tools/call", params: { name, arguments: args } },
  });
  expect(res.status()).toBe(200);
  const body = await res.json();
  return JSON.parse(body.result.content[0].text);
}

async function savedDownload(page: Page, click: () => Promise<void>): Promise<{ name: string; bytes: Buffer }> {
  const [download] = await Promise.all([page.waitForEvent("download"), click()]);
  return { name: download.suggestedFilename(), bytes: await readFile((await download.path())!) };
}

test.describe("Documents", () => {
  test("upload in the UI, see it indexed, download it, export it, delete it", async ({ page }) => {
    await registerAndOnboard(page, uniqueEmail("docsui"));
    await page.getByRole("link", { name: "Documents" }).first().click();
    await expect(page.getByRole("heading", { name: "Documents", level: 1 })).toBeVisible();
    await expect(page.getByText(/^No documents yet/)).toBeVisible();

    const content = Buffer.from("# Field notes\n\nThe heron census found forty-two nests along the eastern marsh.\n");
    await page.getByTestId("document-file-input").setInputFiles({ name: "field-notes.md", mimeType: "text/markdown", buffer: content });

    // the upload selects the new document; it turns Ready once the background job has indexed it
    const panel = page.getByTestId("document-panel");
    await expect(panel.getByRole("heading", { name: "field-notes.md" })).toBeVisible();
    await expect(panel.getByTestId("document-status")).toHaveText("Ready", { timeout: 30_000 });
    await expect(panel.getByTestId("document-passage")).toHaveCount(1);
    await expect(panel.getByTestId("document-passage")).toContainText("heron census");
    const row = page.getByTestId("document-row").filter({ hasText: "field-notes.md" });
    await expect(row).toContainText("Markdown");
    await expect(row).toContainText("Ready");

    const original = await savedDownload(page, () => panel.getByRole("button", { name: "Download" }).click());
    expect(original.name).toBe("field-notes.md");
    expect(sha256(original.bytes)).toBe(sha256(content));

    const exported = await savedDownload(page, () => panel.getByRole("button", { name: "Export" }).click());
    const manifest = JSON.parse(exported.bytes.toString("utf8"));
    expect(manifest.format).toBe("eunomia.document-export");
    expect(manifest.document.sha256).toBe(sha256(content));
    expect(manifest.chunks).toHaveLength(1);
    expect(manifest.chunks[0].text).toContain("heron census");
    expect(manifest.embedding.dimension).toBe(1536);

    // the palette's record search finds the passage and opens the document at it
    await page.keyboard.press("ControlOrMeta+k");
    await page.getByPlaceholder("Jump to a page…").fill("heron census");
    const hit = page.getByRole("dialog", { name: "Search" }).getByText("field-notes.md").first();
    await expect(hit).toBeVisible({ timeout: 10_000 });
    await hit.click();
    await expect(page).toHaveURL(/\/documents\?id=document%3A.*&chunk=/);
    await expect(page.getByTestId("document-passage").first()).toHaveClass(/frame-selected/);

    // delete asks first, naming the file, and unlocks only once its name is typed
    await panel.getByRole("button", { name: "Delete" }).click();
    const dialog = page.getByRole("dialog", { name: "Delete field-notes.md?" });
    const confirm = dialog.getByRole("button", { name: "Delete document" });
    await expect(confirm).toBeDisabled();
    await dialog.getByLabel("Type field-notes.md to confirm").fill("field-notes.md");
    await confirm.click();
    await expect(page.getByTestId("document-row")).toHaveCount(0);
    await expect(page.getByText(/^No documents yet/)).toBeVisible();

    // gone from search too
    const search = await page.request.post("/api/tools/search", { data: { query: "heron census", mode: "keyword" } });
    expect(JSON.stringify(await search.json())).not.toContain("heron");
  });

  test("an unsupported or oversized file is refused with a reason", async ({ page }) => {
    await registerAndOnboard(page, uniqueEmail("docsbad"));
    await page.goto("/documents");
    await page.getByTestId("document-file-input").setInputFiles({ name: "installer.exe", mimeType: "application/octet-stream", buffer: Buffer.from("MZ") });
    await expect(page.getByRole("alert").filter({ hasText: "installer.exe is not a supported type" })).toBeVisible();
    await expect(page.getByTestId("document-row")).toHaveCount(0);
  });

  test("an agent uploads over MCP, recall and search find the passage, get resolves the document", async ({ request }) => {
    const token = await newToken(request);
    const text = "Quarterly planning: the lighthouse restoration budget is 48,000 euros, approved by the harbour board.";
    const up = await tool(request, token, "document_upload", { filename: "planning.txt", text });
    expect(up.error).toBeUndefined();
    expect(up.status).toBe("indexing");

    await expect
      .poll(async () => (await tool(request, token, "document_get", { id: up.id })).document.status, { timeout: 30_000 })
      .toBe("ready");
    const got = await tool(request, token, "document_get", { id: up.id });
    expect(got.text).toBe(text);
    const chunkId: string = got.chunks[0].chunk_id;

    const recalled = await tool(request, token, "recall", { query: "lighthouse restoration budget" });
    const recallHit = recalled.results.find((h: { id: string }) => h.id === chunkId);
    expect(recallHit, JSON.stringify(recalled)).toBeTruthy();
    expect(recallHit.document.document_id).toBe(up.id);
    expect(recallHit.document.char_start).toBe(0);

    const searched = await tool(request, token, "search", { query: "lighthouse restoration", mode: "keyword" });
    expect(searched.results[0].document.document_id).toBe(up.id);

    const record = await tool(request, token, "get", { id: chunkId });
    expect(record.document.document_id).toBe(up.id);
    expect(record.document.filename).toBe("planning.txt");
    expect(record.document.status).toBe("ready");

    // the bytes come over HTTP with the same token, exactly as uploaded
    const where = await tool(request, token, "document_download", { id: up.id });
    const res = await request.get(where.download.path, { headers: { Authorization: `Bearer ${token}` } });
    expect(res.status()).toBe(200);
    expect(sha256(Buffer.from(await res.body()))).toBe(where.sha256);

    // another account cannot reach it
    const other = await newToken(request);
    expect((await tool(request, other, "document_get", { id: up.id })).code).toBe("document.not_found");
    const theirs = await request.get(where.download.path, { headers: { Authorization: `Bearer ${other}` } });
    expect(theirs.status()).toBe(404);

    const deleted = await tool(request, token, "document_delete", { id: up.id });
    expect(deleted.deleted).toBe(true);
    const after = await tool(request, token, "recall", { query: "lighthouse restoration budget" });
    expect(JSON.stringify(after)).not.toContain(chunkId);
  });
});

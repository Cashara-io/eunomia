import http from "node:http";
import type { AddressInfo } from "node:net";
import { test, expect, type Page } from "@playwright/test";
import { registerAndOnboard, uniqueEmail, MOCK_BIND, mockBase } from "./helpers";

// Connector routing (docs/connectors.md "Which vault records go to") against a stand-in for Pocket's API
// (the backend reaches it through the connector's base URL, honoured only with
// EUNOMIA_ALLOW_CONNECTOR_BASE_URL=1): one rule sends recordings tagged "work" to a work vault, the rest
// stay personal. Set in the UI, then Sync now, then checked with the `search` tool per vault.
function recording(id: string, title: string, tags: string[], summary: string) {
  return {
    id,
    title,
    recording_at: new Date().toISOString(),
    duration: 600,
    tags: tags.map((name) => ({ id: name, name })),
    summarizations: { s: { v2: { summary: { markdown: summary } } } },
    transcript: { segments: [] },
  };
}

function mockPocket(): Promise<{ base: string; close: () => void }> {
  const recordings = [
    recording("rec_work", "Paperless standup", ["work"], "Agreed the tamarind importer fix."),
    recording("rec_home", "Dentist call", [], "Booked the rambutan appointment."),
  ];
  const server = http.createServer((req, res) => {
    res.setHeader("content-type", "application/json");
    if (req.headers.authorization !== "Bearer pk_e2e_routing") {
      res.statusCode = 401;
      res.end(JSON.stringify({ message: "Bad key" }));
      return;
    }
    const path = (req.url ?? "").split("?")[0];
    if (path === "/public/recordings") return res.end(JSON.stringify({ data: recordings, pagination: { has_more: false } }));
    const one = recordings.find((r) => path === `/public/recordings/${r.id}`);
    if (one) return res.end(JSON.stringify({ data: one }));
    res.statusCode = 404;
    res.end("{}");
  });
  return new Promise((resolve) =>
    server.listen(0, MOCK_BIND, () => {
      const { port } = server.address() as AddressInfo;
      resolve({ base: mockBase(port), close: () => server.close() });
    }),
  );
}

async function search(page: Page, query: string, vaultIds: string[]): Promise<string[]> {
  const r = await page.request.post("/api/tools/search", { data: { query, vault_ids: vaultIds } });
  expect(r.ok()).toBeTruthy();
  return (await r.json()).results.map((h: { title: string }) => h.title);
}

async function choose(page: Page, combobox: string, option: string) {
  await page.getByRole("combobox", { name: combobox }).click();
  await page.getByRole("option", { name: option }).click();
}

test("a routing rule files tagged recordings in their own vault", async ({ page }) => {
  const pocket = await mockPocket();
  try {
    await registerAndOnboard(page, uniqueEmail("routing"));
    expect((await page.request.post("/api/tools/vault_create", { data: { name: "Work e2e", kind: "org" } })).ok()).toBeTruthy();

    await page.goto("/connectors/setup/pocketai");
    await page.getByPlaceholder("pk_…").fill("pk_e2e_routing");
    await page.getByPlaceholder("https://public.heypocketai.com/api/v1").fill(pocket.base);
    await page.getByRole("button", { name: "Save", exact: true }).click();
    await expect(page.getByRole("button", { name: "Saved" })).toBeVisible();

    // destination: personal (the default); one rule: tag "work" -> Work e2e
    const routing = page.getByRole("region", { name: "Where records go" });
    await expect(routing.getByRole("combobox", { name: "Destination vault" })).toHaveText(/Personal/);
    await routing.getByRole("button", { name: "Add rule" }).click();
    await choose(page, "Rule 1 field", "Tag");
    await routing.getByRole("textbox", { name: "Rule 1 value" }).fill("work");
    await choose(page, "Rule 1 vault", "Work e2e");
    await routing.getByRole("button", { name: "Save routing" }).click();
    await expect(routing.getByRole("button", { name: "Saved" })).toBeVisible();

    // the saved routing comes back from the server
    const connector = await (await page.request.get("/api/connectors/pocketai")).json();
    expect(connector.routing.rules).toEqual([{ field: "tag", value: "work", vault: expect.stringMatching(/^vault:/) }]);
    expect(connector.routing.default_vault).toBeUndefined();

    await page.getByRole("button", { name: "Sync now" }).click();
    await expect(page.getByRole("status").filter({ hasText: /Synced|Sync/ })).toHaveText(/Synced: 2 new or changed/);

    // the work recording is in the work vault, the other one stays personal
    expect(await search(page, "tamarind", ["Work e2e"])).toEqual(["Paperless standup"]);
    expect(await search(page, "rambutan", ["Work e2e"])).toEqual([]);
    expect(await search(page, "rambutan", ["personal"])).toEqual(["Dentist call"]);
    expect(await search(page, "tamarind", ["personal"])).toEqual([]);

    // already where the rules send it: moving existing data changes nothing
    await routing.getByRole("button", { name: "Move existing data" }).click();
    await routing.getByRole("button", { name: "Move now" }).click();
    await expect(routing.getByRole("status")).toHaveText("Everything is already where these rules send it.");
  } finally {
    pocket.close();
  }
});

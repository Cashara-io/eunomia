import http from "node:http";
import type { AddressInfo } from "node:net";
import { test, expect } from "@playwright/test";
import { registerAndOnboard, showAdminSettings, uniqueEmail, MOCK_BIND, mockBase } from "./helpers";

// Pocket end to end against one stand-in server playing both Pocket's
// public API and an OpenAI-compatible model server (reached through
// `config.base_url`, which needs EUNOMIA_ALLOW_CONNECTOR_BASE_URL=1 -- CI sets
// it): a sync stores the whole recording, chunks + embeds it, extracts
// entities and relations with the server's own chat model, the full
// transcript reads back through `get`, and "Delete all data" removes it.
function mockPocketAndModels(): Promise<{ base: string; chatModels: string[]; embedded: string[]; extractionPrompts: string[]; close: () => void }> {
  const chatModels: string[] = [];
  const embedded: string[] = [];
  const recording = {
    id: "rec_e2e",
    title: "Zanzibar importer review",
    duration: 1200,
    recording_at: new Date().toISOString(),
    tags: [{ id: "t1", name: "work", color: "#000" }],
    speakers: { s1: { name: "Ada Lovelace", speakerId: "s1" } },
  };
  const detail = {
    ...recording,
    language: "en",
    transcript: {
      segments: [
        { speaker: "s1", start: 0, end: 4000, text: "The quarterly importer drops the last CSV row." },
        // Two ~3.5k-char turns: the transcript lands in two chunks.
        { speaker: "s2", speakerName: "Grace Hopper", start: 4000, end: 9000, text: "I will pair with Ada on the fix. " + "detail ".repeat(500) },
        { speaker: "s2", speakerName: "Grace Hopper", start: 9000, end: 14000, text: "detail ".repeat(500) },
        // an unnamed speaker (labelled by its id)
        { speaker: "s3", start: 14000, end: 15000, text: "We should meet at the Clayton lab on Friday." },
      ],
    },
    summarizations: { sm1: { v2: { summary: { markdown: "Ada and Grace agreed to fix the importer." }, actionItems: { items: [{ title: "Ship the importer fix" }] } } } },
  };
  // What a model pulls out of the conversation -- including a placeholder
  // speaker, which must not become a person.
  const extraction = {
    people: [
      { name: "Ada Lovelace", facts: ["Ada found the importer bug"] },
      { name: "Grace Hopper", facts: ["Grace pairs on the importer fix"] },
      { name: "Unknown", facts: ["Unknown suggested meeting at the lab"] },
    ],
    locations: [{ name: "Clayton lab", facts: ["The team meets at the Clayton lab on Friday"] }],
    relations: [
      { from: "Grace Hopper", from_kind: "person", to: "Ada Lovelace", to_kind: "person", label: "works_with" },
      { from: "Ada Lovelace", from_kind: "person", to: "Clayton lab", to_kind: "location", label: "meets_at" },
    ],
  };
  const extractionPrompts: string[] = [];
  const server = http.createServer((req, res) => {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      res.setHeader("content-type", "application/json");
      const path = (req.url ?? "").split("?")[0];
      const send = (v: unknown) => res.end(JSON.stringify(v));
      if (path === "/public/recordings") {
        if (req.headers.authorization !== "Bearer pk_e2e") {
          res.statusCode = 401;
          return send({ error: "invalid key" });
        }
        return send({ success: true, data: [recording], pagination: { has_more: false } });
      }
      if (path === "/public/recordings/rec_e2e") return send({ success: true, data: detail });
      if (path === "/models") return send({ data: [{ id: "nomic-embed-text" }, { id: "llama3.1-e2e" }] });
      if (path === "/embeddings") {
        const input = JSON.parse(body).input as string[];
        embedded.push(...input);
        return send({ data: input.map((_, index) => ({ index, embedding: Array(1536).fill(0.01) })) });
      }
      if (path === "/chat/completions") {
        const req = JSON.parse(body);
        chatModels.push(req.model);
        if (body.includes("Extract entities")) extractionPrompts.push(req.messages[0].content);
        const content = body.includes("Extract entities") ? JSON.stringify(extraction) : JSON.stringify({ belief: "Ada works on the importer." });
        return send({ choices: [{ message: { content } }] });
      }
      res.statusCode = 404;
      send({});
    });
  });
  return new Promise((resolve) =>
    server.listen(0, MOCK_BIND, () => {
      const { port } = server.address() as AddressInfo;
      resolve({ base: mockBase(port), chatModels, embedded, extractionPrompts, close: () => server.close() });
    })
  );
}

test("a Pocket recording is stored whole, embedded, graphed, and deletable", async ({ page }) => {
  const mock = await mockPocketAndModels();
  try {
    await registerAndOnboard(page, uniqueEmail("pocket"));
    // This user's model server is the mock; no chat model set, so it's discovered.
    expect((await page.request.patch("/api/settings", { data: { openai_base_url: mock.base, openai_api_key: "sk-e2e" } })).ok()).toBeTruthy();
    expect(
      (await page.request.put("/api/connectors/pocketai", { data: { enabled: true, credentials: { api_key: "pk_e2e" }, config: { base_url: mock.base } } })).ok()
    ).toBeTruthy();

    const sync = await (await page.request.post("/api/sources/heypocket/sync")).json();
    expect(sync.failed ?? 0, JSON.stringify(sync)).toBe(0);
    expect(sync.written, JSON.stringify(sync)).toBe(3); // the recording + 2 transcript chunks

    // The whole recording reads back, from any of its records.
    const got = await (await page.request.post("/api/tools/get", { data: { id: "heypocket:heypocket.transcript_chunk:rec_e2e:1" } })).json();
    expect(got.recording, JSON.stringify(got).slice(0, 800)).toBeTruthy();
    expect(got.recording.summary).toBe("Ada and Grace agreed to fix the importer.");
    expect(got.recording.action_items).toEqual(["Ship the importer fix"]);
    expect(got.recording.tags).toEqual(["work"]);
    expect(got.recording.transcript).toMatch(/^Ada Lovelace: The quarterly importer drops the last CSV row\.\nGrace Hopper: I will pair/);
    expect(got.recording.transcript.match(/detail/g)).toHaveLength(1000);

    // Every record was embedded (the recording + both chunks) and is
    // searchable; the graph has both people and their relation, drawn by
    // the model server's own chat model.
    expect(mock.embedded.filter((t) => t.includes("Zanzibar importer review"))).toHaveLength(3);
    const found = await (await page.request.post("/api/tools/search", { data: { query: "importer CSV row" } })).json();
    expect(found.results.map((r: { id: string }) => r.id)).toContain("heypocket:heypocket.transcript_chunk:rec_e2e:0");
    const graph = JSON.stringify(await (await page.request.get("/api/entities/graph")).json());
    expect(graph).toContain("Ada Lovelace");
    expect(graph).toContain("Grace Hopper");
    expect(graph).toContain("works_with");
    // The call's content is graphed -- the place discussed and who meets
    // there -- but not a placeholder speaker, nor the meeting itself.
    expect(graph).toContain("Clayton lab");
    expect(graph).toContain("meets_at");
    expect(graph).not.toMatch(/"name":"Unknown"/);
    expect(graph).not.toContain("Zanzibar importer review");
    // Extraction is told it reads a dated conversation.
    const today = new Date().toISOString().slice(0, 10);
    expect(mock.extractionPrompts.some((p) => p.includes("transcript") && p.includes(`dated ${today}`) && p.includes("We should meet at the Clayton lab"))).toBe(true);
    expect(mock.chatModels.length).toBeGreaterThan(0);
    expect(new Set(mock.chatModels)).toEqual(new Set(["llama3.1-e2e"]));

    // The UI shows the full transcript...
    await page.goto("/connectors/heypocket");
    await page.getByRole("button", { name: /Zanzibar importer review \(transcript 1\/2\)/ }).click();
    await page.getByText("Full transcript").click();
    await expect(page.getByText(/Grace Hopper: I will pair with Ada/)).toBeVisible();

    // ...and "Delete all data" takes two confirmations in a modal.
    await page.getByRole("button", { name: "Delete all heypocket data" }).click();
    const dialog = page.getByRole("dialog");
    await expect(dialog).toContainText("3 heypocket records");
    const confirm = dialog.getByRole("button", { name: "Delete everything" });
    await expect(confirm).toBeDisabled();
    await dialog.getByRole("textbox").fill("heypocket");
    await confirm.click();
    await expect(dialog).toBeHidden();
    await expect(page.getByText(/No records yet/)).toBeVisible();

    const gone = await (await page.request.post("/api/tools/get", { data: { id: "heypocket:heypocket.recording:rec_e2e" } })).json();
    expect(gone.error).toBe("not found");
    const after = JSON.stringify(await (await page.request.get("/api/entities/graph")).json());
    expect(after).not.toContain("works_with");
    // The connection itself is kept.
    expect((await (await page.request.get("/api/connectors/pocketai")).json()).credentials_set).toBe(true);

    // The chat model can also be pinned in Settings, Server, AI provider (blank = automatic).
    await showAdminSettings(page);
    await page.goto("/settings?tab=model");
    await expect(page.getByLabel("Base URL")).toHaveValue(mock.base);
    await page.waitForLoadState("networkidle"); // dev mode loads settings twice
    await page.getByLabel("Chat model").click(); // the endpoint lists its models, so it is a select
    await page.getByRole("option", { name: "llama3.1-e2e" }).click();
    await page.getByRole("button", { name: "Save settings" }).click();
    await expect(page.getByRole("button", { name: "Saved" })).toBeVisible();
    const saved = await (await page.request.get("/api/settings")).json();
    expect(saved.chat_model, JSON.stringify(saved)).toBe("llama3.1-e2e");
  } finally {
    mock.close();
  }
});

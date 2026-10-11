import { test, expect, type APIRequestContext } from "@playwright/test";
import { registerAndOnboard, uniqueEmail } from "./helpers";

// Exercises /mcp through the frontend's proxy, the URL the dashboard shows.
async function newToken(request: APIRequestContext): Promise<string> {
  const reg = await request.post("/api/auth/register", {
    data: { email: uniqueEmail("mcp"), password: "correct-horse-battery-staple" },
  });
  expect(reg.ok()).toBeTruthy();
  const res = await request.post("/api/auth/tokens", { data: { name: "MCP" } });
  return (await res.json()).token;
}

function rpc(request: APIRequestContext, token: string | null, body: unknown) {
  return request.post("/mcp", {
    headers: {
      Accept: "application/json, text/event-stream",
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
    },
    data: body,
  });
}

test.describe("MCP server", () => {
  test("the dashboard shows this app's /mcp URL and a ready-to-run Claude Code command", async ({ page }) => {
    await registerAndOnboard(page, uniqueEmail("mcpcard"));
    const url = `${new URL(page.url()).origin}/mcp`;
    await expect(page.getByText(url, { exact: true })).toBeVisible();

    await page.getByRole("button", { name: "Generate a token" }).click();
    await expect(page.getByText(`claude mcp add --transport http --scope user eunomia ${url} --header`)).toBeVisible();
  });

  test("initialize, list tools, and call one", async ({ request }) => {
    const token = await newToken(request);

    const init = await rpc(request, token, {
      jsonrpc: "2.0",
      id: 1,
      method: "initialize",
      params: { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "e2e", version: "1" } },
    });
    expect(init.status()).toBe(200);
    const initBody = await init.json();
    expect(initBody.result.protocolVersion).toBe("2025-06-18");
    expect(initBody.result.capabilities.tools).toBeTruthy();

    const notified = await rpc(request, token, { jsonrpc: "2.0", method: "notifications/initialized" });
    expect(notified.status()).toBe(202);

    const list = await (await rpc(request, token, { jsonrpc: "2.0", id: 2, method: "tools/list" })).json();
    const names = list.result.tools.map((t: { name: string }) => t.name);
    expect(names).toEqual(expect.arrayContaining(["recall", "memory_write", "vault_list", "search"]));
    for (const t of list.result.tools) expect(t.description.length).toBeGreaterThan(10);

    const call = await (
      await rpc(request, token, { jsonrpc: "2.0", id: 3, method: "tools/call", params: { name: "vault_list", arguments: {} } })
    ).json();
    expect(call.result.isError).toBe(false);
    expect(call.result.content[0].text).toContain("Personal");
  });

  test("a written memory is recalled through MCP", async ({ request }) => {
    const token = await newToken(request);
    const call = (id: number, name: string, args: object) =>
      rpc(request, token, { jsonrpc: "2.0", id, method: "tools/call", params: { name, arguments: args } }).then((r) => r.json());

    const written = await call(1, "memory_write", {
      subject_name: "Ada Lovelace",
      subject_kind: "person",
      text: "Wrote the first published algorithm for the Analytical Engine.",
    });
    expect(written.result.isError).toBe(false);

    const found = await call(2, "entities_search", { query: "Ada" });
    expect(found.result.content[0].text).toContain("Ada Lovelace");
  });

  test("memory CRUD and recall work through MCP, whether or not the server has an OpenAI key", async ({ request }) => {
    const token = await newToken(request);
    let n = 0;
    const call = async (name: string, args: object) => {
      const r = await (
        await rpc(request, token, { jsonrpc: "2.0", id: ++n, method: "tools/call", params: { name, arguments: args } })
      ).json();
      const text = r.result.content[0].text;
      return { isError: r.result.isError as boolean, data: JSON.parse(text) };
    };

    // create
    const written = await call("memory_write", {
      subject_name: "Grace Hopper",
      subject_kind: "person",
      text: "Invented the first compiler, the A-0 system.",
    });
    expect(written.isError).toBe(false);
    const memoryId: string = written.data.memory.id;
    const entityId: string = written.data.entity.id;

    // read: found by what it says, not just by the entity's name
    const recalled = await call("recall", { query: "compiler" });
    expect(recalled.data.results.map((r: { text: string }) => r.text)).toContain("Invented the first compiler, the A-0 system.");

    // reflect never needs a server-side model: it either answers or hands the memories to the agent
    const reflected = await call("reflect", { query: "who invented the compiler?" });
    expect(reflected.isError).toBe(false);
    if (reflected.data.mode === "recall_only") {
      expect(reflected.data.answer).toBeNull();
      expect(reflected.data.memories.length).toBeGreaterThan(0);
      expect(reflected.data.instructions).toContain("you answer");
    } else {
      expect(typeof reflected.data.answer).toBe("string");
    }

    // update: the old words stop matching, the new ones start
    const updated = await call("memory_update", { memory_id: memoryId, text: "Popularised machine-independent programming languages.", type: "experience" });
    expect(updated.isError).toBe(false);
    expect(updated.data.version).toBe(2);
    expect((await call("recall", { query: "compiler" })).data.results).toHaveLength(0);
    expect((await call("recall", { query: "machine-independent" })).data.results).toHaveLength(1);
    expect((await call("memory_update", { memory_id: memoryId, type: "observation" })).isError).toBe(true);

    // an observation is revised in place, not duplicated
    await call("memory_write", { subject_name: "Grace Hopper", subject_kind: "person", text: "Pioneer.", type: "observation" });
    const revised = await call("memory_write", { subject_name: "Grace Hopper", subject_kind: "person", text: "Pioneer of compilers.", type: "observation" });
    expect(revised.data.memory.version).toBe(2);

    const renamed = await call("entity_update", { entity_id: entityId, aliases: ["Amazing Grace"], summary: "Rear admiral" });
    expect(renamed.data.aliases).toEqual(["Amazing Grace"]);
    const detail = await call("entities_get", { id: entityId });
    expect(detail.data.memory).toHaveLength(2); // the fact + the one observation

    // delete
    expect((await call("memory_delete", { memory_id: memoryId })).data.deleted).toBe(true);
    expect((await call("recall", { query: "machine-independent" })).data.results).toHaveLength(0);
    expect((await call("entity_delete", { entity_id: entityId })).data.deleted).toBe(true);
  });

  test("full-sentence questions find memories and records (not just exact keywords)", async ({ request }) => {
    const token = await newToken(request);
    let n = 0;
    const call = async (name: string, args: object) =>
      JSON.parse(
        (await (await rpc(request, token, { jsonrpc: "2.0", id: ++n, method: "tools/call", params: { name, arguments: args } })).json())
          .result.content[0].text
      );

    await call("memory_write", { subject_name: "Homelab", subject_kind: "location", text: "Updates run in a separate updater container triggered by a marker file." });
    await call("memory_write", { subject_name: "Ada", subject_kind: "person", text: "Ada prefers async updates over meetings." });
    await call("memory_write", { subject_name: "Grace", subject_kind: "person", text: "Grace invented the first compiler." });

    // every word of the question used to be required, so this returned nothing
    const texts = (await call("recall", { query: "How do updates work with the updater container?" })).results.map((r: { text: string }) => r.text);
    expect(texts[0]).toContain("updater container"); // matches the most words, so it ranks first
    expect(texts).toContain("Ada prefers async updates over meetings.");
    expect(texts).not.toContain("Grace invented the first compiler.");
    expect((await call("recall", { query: "what does Grace do?" })).results.map((r: { text: string }) => r.text)).toContain("Grace invented the first compiler.");

    // the same for synced records via search
    expect((await request.post("/api/sources/demo/sync")).ok()).toBeTruthy();
    const listed = await call("list", { limit: 1 });
    const word = (listed.results[0].title as string).split(/\W+/).find((w: string) => w.length > 3) ?? listed.results[0].title;
    const found = await call("search", { query: `can you show me anything about ${word} please?` });
    expect(found.results.map((r: { id: string }) => r.id)).toContain(listed.results[0].id);
  });

  test("a huge memory can't crowd everything out of a token-budgeted recall (the hook's case)", async ({ request }) => {
    const token = await newToken(request);
    let n = 0;
    const call = async (name: string, args: object) =>
      JSON.parse(
        (await (await rpc(request, token, { jsonrpc: "2.0", id: ++n, method: "tools/call", params: { name, arguments: args } })).json())
          .result.content[0].text
      );
    await call("memory_write", { subject_name: "Archive", subject_kind: "organisation", text: "Transcript about the updater container. " + "filler words ".repeat(3000) });
    await call("memory_write", { subject_name: "Homelab", subject_kind: "location", text: "The updater container applies updates." });
    const results = (await call("recall", { query: "how does the updater container work?", limit: 6, max_tokens: 600 })).results;
    expect(results.length).toBe(2); // used to be 0: the oversized first hit ended the list
    const texts = results.map((r: { text: string }) => r.text);
    expect(texts).toContain("The updater container applies updates.");
    const trimmed = texts.find((t: string) => t.startsWith("Transcript"));
    expect(trimmed.endsWith("…")).toBe(true);
    expect(trimmed.length).toBeLessThanOrEqual(800);
  });

  test("vaults keep separate areas apart and can be used by name", async ({ request }) => {
    const token = await newToken(request);
    let n = 0;
    const call = async (name: string, args: object) =>
      JSON.parse(
        (await (await rpc(request, token, { jsonrpc: "2.0", id: ++n, method: "tools/call", params: { name, arguments: args } })).json())
          .result.content[0].text
      );
    expect((await call("vault_create", { name: "Garden Project" })).name).toBe("Garden Project");
    await call("vault_create", { name: "Book Club" });

    // write and read by name -- no vault ids needed
    expect((await call("memory_write", { subject_name: "Tomatoes", subject_kind: "organisation", text: "Tomatoes need staking by June.", vault_id: "garden project" })).memory).toBeTruthy();
    await call("memory_write", { subject_name: "Dune", subject_kind: "organisation", text: "The club reads Dune in June.", vault_id: "Book Club" });
    const garden = (await call("recall", { query: "what happens in June?", vault_id: "Garden Project" })).results.map((r: { text: string }) => r.text);
    expect(garden).toEqual(["Tomatoes need staking by June."]); // nothing from the other vault
    const personalId = (await call("vault_list", {})).results.find((v: { kind: string }) => v.kind === "personal").id;
    expect((await call("recall", { query: "what happens in June?", vault_id: personalId })).results).toEqual([]); // nor in the personal vault
    // no vault given: every org vault, each hit labelled with its vault
    const everywhere = (await call("recall", { query: "what happens in June?" })).results.map((r: { text: string; vault_name: string }) => [r.vault_name, r.text]);
    expect(everywhere).toEqual(expect.arrayContaining([["Garden Project", "Tomatoes need staking by June."], ["Book Club", "The club reads Dune in June."]]));

    expect((await call("recall", { query: "June", vault_id: "Nope" })).error).toContain("no vault named");
    expect((await call("vault_rename", { vault_id: "Book Club", name: "Reading Group" })).name).toBe("Reading Group");
  });

  test("initialize tells the agent it is the model", async ({ request }) => {
    const token = await newToken(request);
    const init = await (
      await rpc(request, token, { jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: "2025-06-18" } })
    ).json();
    expect(init.result.instructions).toContain("You are the model");
  });

  test("protocol errors and auth", async ({ request }) => {
    const token = await newToken(request);

    expect((await rpc(request, null, { jsonrpc: "2.0", id: 1, method: "tools/list" })).status()).toBe(401);
    expect((await rpc(request, "not-a-real-token", { jsonrpc: "2.0", id: 1, method: "tools/list" })).status()).toBe(401);
    expect((await request.get("/mcp")).status()).toBe(405);

    const unknownTool = await (
      await rpc(request, token, { jsonrpc: "2.0", id: 1, method: "tools/call", params: { name: "nope", arguments: {} } })
    ).json();
    expect(unknownTool.error.code).toBe(-32602);

    const unknownMethod = await (await rpc(request, token, { jsonrpc: "2.0", id: 2, method: "resources/list" })).json();
    expect(unknownMethod.error.code).toBe(-32601);

    // A tool that fails is a result the model sees, not a protocol error.
    const badArgs = await (
      await rpc(request, token, { jsonrpc: "2.0", id: 3, method: "tools/call", params: { name: "get", arguments: {} } })
    ).json();
    expect(badArgs.result.isError).toBe(true);
  });
});

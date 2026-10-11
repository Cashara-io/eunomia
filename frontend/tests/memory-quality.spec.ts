import http from "node:http";
import type { AddressInfo } from "node:net";
import { test, expect, request as pwRequest, type APIRequestContext } from "@playwright/test";
import { uniqueEmail, MOCK_BIND, mockBase } from "./helpers";

// Retrieval quality from real agent use: outdated facts drop out of recall,
// every hit carries its vault, date and a 0-1 score, a handle only ever
// written in a fact finds its entity, and decorated or near-duplicate names
// don't silently create duplicates. A stand-in model server answers the
// supersede check: "no access" is outdated once a later fact grants it.
type Json = any; // eslint-disable-line @typescript-eslint/no-explicit-any

function startModel(): Promise<{ base: string; close: () => void }> {
  const server = http.createServer((req, res) => {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      res.setHeader("content-type", "application/json");
      const send = (content: unknown) => res.end(JSON.stringify({ choices: [{ message: { content: JSON.stringify(content) } }] }));
      if (req.url?.endsWith("/models")) {
        res.statusCode = 404;
        return res.end("{}");
      }
      if (req.url?.endsWith("/embeddings")) {
        const input = JSON.parse(body).input as string[];
        return res.end(JSON.stringify({ data: input.map((_, index) => ({ index, embedding: Array(1536).fill(0.01) })) }));
      }
      const prompt: string = JSON.parse(body).messages.at(-1).content;
      if (prompt.includes('"superseded"')) {
        const lines = [...prompt.matchAll(/^\[(\d+)\] \S+ \S+(?: \(new\))?: (.*)$/gm)].map((m) => ({ n: Number(m[1]), text: m[2] }));
        const granted = lines.find((l) => l.text.includes("was granted access"));
        return send({ superseded: granted ? lines.filter((l) => l.n < granted.n && l.text.includes("no access")).map((l) => l.n) : [] });
      }
      if (prompt.includes("Extract entities")) return send({});
      send({ belief: "consolidated" });
    });
  });
  return new Promise((resolve) =>
    server.listen(0, MOCK_BIND, () => resolve({ base: mockBase((server.address() as AddressInfo).port), close: () => server.close() }))
  );
}

let ctx: APIRequestContext;
let model: { base: string; close: () => void };
const ok = async (name: string, args: object = {}) => {
  const res = await ctx.post(`/api/tools/${name}`, { data: args });
  const data = await res.json();
  expect(res.ok() && !data.error, `${name}: ${JSON.stringify(data)}`).toBeTruthy();
  return data;
};

test.describe.serial("memory quality", () => {
  test.beforeAll(async ({}, testInfo) => {
    ctx = await pwRequest.newContext({ baseURL: testInfo.project.use.baseURL as string });
    const email = uniqueEmail("quality");
    expect((await ctx.post("/api/auth/register", { data: { email, password: "correct-horse-battery-staple" } })).ok()).toBeTruthy();
    model = await startModel();
    expect((await ctx.patch("/api/settings", { data: { openai_base_url: model.base, openai_api_key: "sk-test" } })).ok()).toBeTruthy();
  });

  test.afterAll(async () => {
    model?.close();
    await ctx?.dispose();
  });

  test("a fact a later one contradicts is superseded and leaves recall", async () => {
    const old = await ok("memory_write", { subject_name: "Riley Chen", subject_kind: "person", text: "Riley has no access to the build server" });
    const fresh = await ok("memory_write", { subject_name: "Riley Chen", subject_kind: "person", text: "Riley was granted access to the build server" });
    // the check runs in a background job after the write: wait for it to mark the old fact
    const statusOf = async (id: string) => (await ok("entities_get", { id: fresh.entity.id })).memory.find((m: Json) => m.id === id).status ?? null;
    await expect.poll(() => statusOf(old.memory.id), { timeout: 30_000 }).toBe("superseded");
    expect(await statusOf(fresh.memory.id)).not.toBe("superseded");

    const recalled = await ok("recall", { query: "Riley Chen build server access" });
    const texts = recalled.results.map((i: Json) => i.text);
    expect(texts).toContain("Riley was granted access to the build server");
    expect(texts).not.toContain("Riley has no access to the build server");
    expect(recalled.results[0].score).toBe(1);
    expect(recalled.results.every((i: Json) => i.occurred_at && i.vault && i.vault_name)).toBeTruthy();

    const entity = await ok("entities_get", { id: fresh.entity.id });
    expect(entity.memory.find((m: Json) => m.id === old.memory.id).status).toBe("superseded");

    // Editing it makes it current again.
    await ok("memory_update", { memory_id: old.memory.id, text: "Riley had no access to the build server until October" });
    const edited = await ok("entities_get", { id: fresh.entity.id });
    expect(edited.memory.find((m: Json) => m.id === old.memory.id).status ?? null).toBeNull();
  });

  test("a decorated name resolves to the existing entity, adding aliases", async () => {
    const riley = await ok("entities_search", { query: "Riley Chen" });
    const w = await ok("memory_write", { subject_name: "Riley Chen (Acme/rc)", subject_kind: "person", text: "Riley leads the build team" });
    expect(w.entity.id).toBe(riley.results[0].id);
    expect(w.entity.aliases).toEqual(expect.arrayContaining(["Acme", "rc"]));
  });

  test("a handle written only in a fact finds its entity", async () => {
    const sam = await ok("memory_write", { subject_name: "Sam Okafor", subject_kind: "person", text: "Sam's chat handle is quietfox" });
    const found = await ok("entities_search", { query: "quietfox" });
    expect(found.results.map((e: Json) => e.id)).toContain(sam.entity.id);
  });

  test("a new entity sharing a name word with one that exists is flagged", async () => {
    const sam = (await ok("entities_search", { query: "Sam Okafor" })).results[0];
    const w = await ok("memory_write", { subject_name: "Sam", subject_kind: "person", text: "Sam is presenting on Friday" });
    expect(w.entity.id).not.toBe(sam.id);
    expect(w.possible_duplicates.map((e: Json) => e.id)).toContain(sam.id);
    // writing about an entity that already exists flags nothing
    expect((await ok("memory_write", { subject_name: "Sam", subject_kind: "person", text: "Sam likes tea" })).possible_duplicates).toBeUndefined();
  });
});

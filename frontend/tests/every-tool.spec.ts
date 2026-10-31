import { test, expect, request as pwRequest, type APIRequestContext } from "@playwright/test";
import { uniqueEmail } from "./helpers";

// Calls every MCP tool at least once over the real /mcp endpoint, checks each
// result, and fails if a tool exists that this test never called -- so a new
// tool can't ship untested.
async function account(request: APIRequestContext, prefix: string) {
  const email = uniqueEmail(prefix);
  expect((await request.post("/api/auth/register", { data: { email, password: "correct-horse-battery-staple" } })).ok()).toBeTruthy();
  const token = (await (await request.post("/api/auth/tokens", { data: { name: prefix } })).json()).token as string;
  return { email, token };
}

test("every MCP tool works end to end", async ({ request, baseURL }) => {
  test.setTimeout(120_000);
  const me = await account(request, "tools");
  const called = new Set<string>();
  let id = 0;
  const call = async (name: string, args: object = {}, token = me.token) => {
    called.add(name);
    const res = await request.post("/mcp", {
      headers: { Authorization: `Bearer ${token}`, Accept: "application/json, text/event-stream" },
      data: { jsonrpc: "2.0", id: ++id, method: "tools/call", params: { name, arguments: args } },
    });
    expect(res.status(), name).toBe(200);
    const body = await res.json();
    expect(body.error, `${name}: protocol error ${JSON.stringify(body.error)}`).toBeUndefined();
    return { isError: body.result.isError as boolean, data: JSON.parse(body.result.content[0].text) };
  };
  const ok = async (name: string, args: object = {}, token?: string) => {
    const r = await call(name, args, token);
    expect(r.isError, `${name} failed: ${JSON.stringify(r.data)}`).toBe(false);
    return r.data;
  };

  // docs
  expect((await ok("docs")).docs.length).toBeGreaterThanOrEqual(6); // quickstart ... connectors
  expect((await ok("docs", { topic: "concepts" })).markdown).toContain("# Concepts");
  expect((await call("docs", { topic: "nope" })).isError).toBe(true);

  // synced records (demo source), then the record tools
  expect((await request.post("/api/sources/demo/sync")).ok()).toBeTruthy();
  const listed = await ok("list", { limit: 5 });
  expect(listed.results.length).toBeGreaterThan(0);
  const recordId: string = listed.results[0].id;
  expect((await ok("get", { id: recordId })).id).toBe(recordId);
  expect(Array.isArray((await ok("links", { id: recordId })).links)).toBe(true);
  const title: string = listed.results[0].title;
  expect((await ok("search", { query: title.split(" ")[0] })).results.length).toBeGreaterThan(0);

  // entities + memory CRUD
  const ada = await ok("memory_write", { subject_name: "Ada Lovelace", subject_kind: "person", text: "Wrote the first published algorithm." });
  const ada2 = await ok("memory_write", { subject_name: "A. Lovelace", subject_kind: "person", text: "Countess of Lovelace." });
  const babbage = await ok("memory_write", { subject_name: "Charles Babbage", subject_kind: "person", text: "Designed the Analytical Engine." });
  expect((await ok("memory_update", { memory_id: ada.memory.id, text: "Published the first algorithm in 1843." })).version).toBe(2);
  expect((await ok("entity_update", { entity_id: ada.entity.id, summary: "Mathematician" })).summary).toBe("Mathematician");
  expect((await ok("entities_search", { query: "Lovelace" })).results.length).toBeGreaterThanOrEqual(2);
  await ok("code_relate", { from_id: ada.entity.id, to_id: babbage.entity.id, label: "collaborated_with" });
  const merged = await ok("entity_merge", { winner_id: ada.entity.id, loser_id: ada2.entity.id });
  expect(merged.aliases).toContain("A. Lovelace");
  const detail = await ok("entities_get", { id: ada.entity.id });
  expect(detail.memory.length).toBe(2);
  expect(detail.relations.length).toBe(1);
  expect((await ok("entities_graph")).edges.length).toBeGreaterThanOrEqual(1);
  await ok("memory_write", { subject_name: "orders_db", subject_kind: "db", text: "Runs Postgres 16." });
  expect((await ok("entity_types")).custom).toEqual([{ kind: "db", entities: 1 }]);
  const repo = await ok("code_entity_upsert", { kind: "repository", name: "eunomia", summary: "memory system" });
  const file = await ok("code_entity_upsert", { kind: "file", name: "auth.rs", parent_id: repo.id });
  expect(file.id).toBeTruthy();
  const consolidated = await ok("consolidate_observations", { subject_id: ada.entity.id });
  expect(consolidated.note ?? consolidated.consolidated).toBeTruthy();
  expect((await ok("recall", { query: "algorithm" })).results.length).toBeGreaterThan(0);
  const reflected = await ok("reflect", { query: "What did Ada publish?" });
  expect(reflected.answer ?? reflected.memories).toBeTruthy();
  expect((await ok("memory_delete", { memory_id: babbage.memory.id })).deleted).toBe(true);
  expect((await ok("entity_delete", { entity_id: babbage.entity.id })).deleted).toBe(true);

  // vaults: create, clone, merge, rename, share, leave, delete
  const team = await ok("vault_create", { name: "Team" });
  await ok("memory_write", { subject_name: "Grace Hopper", subject_kind: "person", text: "Built the first compiler.", vault_id: team.id });
  const personal = (await ok("vault_list")).results.find((v: { kind: string }) => v.kind === "personal");
  const copy = await ok("vault_clone", { vault_id: team.id, name: "Team copy" });
  expect(copy.entities_copied).toBe(1);
  const both = await ok("vault_merge", { vault_id_a: personal.id, vault_id_b: team.id });
  expect(both.entities).toBeGreaterThanOrEqual(3);
  expect((await ok("vault_rename", { vault_id: team.id, name: "Team (renamed)" })).name).toBe("Team (renamed)");

  const other = await pwRequest.newContext({ baseURL });
  const friend = await account(other, "friend");
  await ok("vault_invite", { vault_id: team.id, email: friend.email });
  expect((await other.post(`/api/vaults/${encodeURIComponent(team.id)}/invitations/accept`)).ok()).toBeTruthy();
  expect((await ok("vault_members", { vault_id: team.id })).results.length).toBe(2);
  expect((await ok("vault_leave", { vault_id: team.id }, friend.token)).left).toBe(true);
  await ok("vault_invite", { vault_id: team.id, email: friend.email });
  expect((await ok("vault_remove_member", { vault_id: team.id, email: friend.email })).removed).toBe(true);
  await other.dispose();
  for (const v of [team.id, copy.id, both.id]) expect((await ok("vault_delete", { vault_id: v })).deleted).toBe(true);

  // documents: upload, wait for indexing, read, download route, export, delete
  const doc = await ok("document_upload", { filename: "notes.md", text: "# Notes\n\nThe kestrel survey starts in May." });
  await expect.poll(async () => (await ok("document_get", { id: doc.id })).document.status, { timeout: 30_000 }).toBe("ready");
  expect((await ok("document_list")).results.map((d: { id: string }) => d.id)).toContain(doc.id);
  expect((await ok("document_download", { id: doc.id })).download.path).toBe(`/api/documents/${doc.id}/download`);
  expect((await ok("document_export", { id: doc.id })).chunks.length).toBe(1);
  expect((await call("document_upload", { filename: "x.exe", text: "MZ" })).isError).toBe(true);
  expect((await ok("document_delete", { id: doc.id })).deleted).toBe(true);

  // agent skills: save, replace by name, find, read, delete, and a bad name is a validation error
  const skill = await ok("skill_save", { name: "deploy-rollback", description: "How to roll back a bad deploy", body: "# Roll back\n\n1. Revert." });
  expect((await ok("skill_save", { name: "deploy-rollback", description: "How to roll back a bad deploy", body: "# Roll back v2" })).id).toBe(skill.id);
  expect((await ok("skill_find", { query: "roll back a deploy" })).skills.map((s: { id: string }) => s.id)).toContain(skill.id);
  expect((await ok("skill_get", { id: skill.id })).body).toBe("# Roll back v2");
  const listed = await ok("skill_list", { limit: 1 });
  expect(listed.skills.map((s: { name: string }) => s.name)).toEqual(["deploy-rollback"]);
  expect(listed.next).toBeNull();
  expect((await call("skill_save", { name: "Bad Name", description: "d", body: "b" })).isError).toBe(true);
  expect((await ok("skill_delete", { id: skill.id })).deleted).toBe(skill.id);

  // the change log has the skill's save, save again and delete, in order, and none of its content
  const log = await ok("changes_since", { limit: 1000 });
  const rows: { seq: number; op: string; kind: string; id: string }[] = log.changes;
  expect(rows.filter((c) => c.id === skill.id).map((c) => `${c.op} ${c.kind}`)).toEqual(["created skill", "updated skill", "deleted skill"]);
  expect(rows.map((c) => c.seq)).toEqual(rows.map((_, i) => i + 1));
  expect(log.next).toBe(rows.length);
  expect(JSON.stringify(log)).not.toContain("Roll back");

  // bulk writes: one result per fact, a bad one fails alone
  const batch = await ok("memory_write_batch", { items: [
    { subject_name: "Import page 1", subject_kind: "file", text: "Page one of the import." },
    { subject_name: "Import page 2", subject_kind: "file" },
  ] });
  expect([batch.written, batch.failed]).toEqual([1, 1]);
  expect(batch.results[1].code).toBe("validation.invalid");

  // coverage: every tool the server lists was exercised above
  const list = await request.post("/mcp", {
    headers: { Authorization: `Bearer ${me.token}` },
    data: { jsonrpc: "2.0", id: ++id, method: "tools/list" },
  });
  const names: string[] = (await list.json()).result.tools.map((t: { name: string }) => t.name);
  expect(names.filter((n) => !called.has(n)), "tools never called by this test").toEqual([]);
});

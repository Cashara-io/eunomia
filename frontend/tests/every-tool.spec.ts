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
    // show_view's text is plain prose for text-only clients; its structure comes as structuredContent
    const data = body.result.structuredContent ?? JSON.parse(body.result.content[0].text);
    return { isError: body.result.isError as boolean, data, text: body.result.content[0].text as string };
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
  const link = await ok("entity_relate", { from_id: babbage.entity.id, label: "corresponded_with", to_id: ada.entity.id, reason: "They wrote letters about the engine." });
  expect(link.reason_memory_id).toBeTruthy();
  const merged = await ok("entity_merge", { winner_id: ada.entity.id, loser_id: ada2.entity.id });
  expect(merged.aliases).toContain("A. Lovelace");
  const detail = await ok("entities_get", { id: ada.entity.id });
  expect(detail.memory.length).toBe(2);
  expect(detail.relations.length).toBe(2);
  expect((await ok("entities_graph")).edges.length).toBeGreaterThanOrEqual(1);
  const hood = await ok("entities_neighbourhood", { entity_id: ada.entity.id });
  expect(hood.nodes.map((n: { name: string }) => n.name)).toContain("Charles Babbage");
  const shown = await call("show_view", {
    view: "impact",
    start_id: ada.entity.id,
    headline: "Ada changes plans.",
    affected: [{ entity_id: babbage.entity.id, path: [hood.edges[0].id], why: "They work together." }],
  });
  expect(shown.isError).toBe(false);
  expect(shown.data.affected[0].entity.name).toBe("Charles Babbage");
  expect(shown.text).toContain("Charles Babbage");
  expect((await call("show_view", { view: "impact", start_id: "person:invented", headline: "x", affected: [] })).isError).toBe(true);
  await ok("memory_write", { subject_name: "orders_db", subject_kind: "db", text: "Runs Postgres 16." });
  expect((await ok("entity_types")).custom).toEqual([{ kind: "db", entities: 1, status: "observed", spaces: ["Personal"] }]);
  await ok("ontology_propose", { kinds: [{ kind: "db", description: "A database" }] });
  expect((await ok("ontology_get")).kinds.find((k: { kind: string }) => k.kind === "db").status).toBe("proposed");
  expect((await ok("ontology_suggest")).mode).toBeTruthy();
  // a typed property takes a value only once an owner approved it in the browser
  const ontology = await ok("ontology_propose", { properties: [{ kind: "db", name: "size_gb", datatype: "number", unit: "GB" }] });
  expect(ontology.properties[0].status).toBe("proposed");
  const orders = (await ok("entities_search", { query: "orders_db" })).results[0].id as string;
  expect((await call("entity_set_property", { entity_id: orders, property: "size_gb", value: 120 })).isError).toBe(true);
  const vaultOf = (await ok("ontology_get")).vault as string;
  const decided = await request.post(`/api/vaults/${vaultOf}/ontology/properties/decide`, {
    data: { kind: "db", name: "size_gb", decision: { action: "approve" } },
  });
  expect(decided.ok()).toBeTruthy();
  expect((await ok("entity_set_property", { entity_id: orders, property: "size_gb", value: 120 })).value.value).toBe(120);
  expect((await ok("entities_get", { id: orders })).properties[0].property).toBe("size_gb");
  // a metric over it runs by name once an owner approved it, with the entities it came from
  await ok("ontology_propose", { metrics: [{ kind: "db", name: "total_size_gb", expr: { agg: "sum", property: "size_gb" } }] });
  expect((await call("metric_eval", { metric: "total_size_gb" })).isError).toBe(true);
  const approvedMetric = await request.post(`/api/vaults/${vaultOf}/ontology/metrics/decide`, {
    data: { kind: "db", name: "total_size_gb", decision: { action: "approve" } },
  });
  expect(approvedMetric.ok()).toBeTruthy();
  const total = await ok("metric_eval", { metric: "total_size_gb" });
  expect([total.value, total.unit, total.entity_ids]).toEqual([120, "GB", [orders]]);
  // coverage: the approved metric's name is covered, a term nobody approved is not
  const covered = await ok("ontology_coverage", { question: "What is the total size gb of our databases?" });
  expect(covered.matched.map((m: { name: string }) => m.name)).toContain("total_size_gb");
  const uncovered = await ok("ontology_coverage", { question: "What is the gross margin?" });
  expect([uncovered.coverage, uncovered.uncovered]).toEqual([0, ["gross margin"]]);
  const ui = await ok("show_view", { view:"ui", spec:{root:"a",elements:{a:{type:"Stat",props:{title:"Total",summary:"Server result",data:{$state:"/sources/size/metric/value"}}}}}, sources:{size:{type:"metric",metric:"total_size_gb",vault_id:vaultOf}} });
  expect(ui.data.sources.size.metric.value).toBe(120);
  expect(ui.text).toContain("120");
  // readiness: the db kind is still only proposed
  const ready = await ok("ontology_readiness");
  expect(ready.blockers).toContain("1 kind is proposed and needs an owner");
  // a what-if over it: the server's numbers before and after, and nothing saved
  const whatIf = { changes: [{ scale: { kind: "db", property: "size_gb", factor: 1.5 } }], metrics: ["total_size_gb"] };
  const scenario = await ok("scenario_run", whatIf);
  expect([scenario.baseline[0].value, scenario.scenario[0].value, scenario.scenario[0].delta]).toEqual([120, 180, 60]);
  expect((await ok("metric_eval", { metric: "total_size_gb" })).value).toBe(120);
  const scenarioView = await call("show_view", { view: "scenario", headline: "What if the databases grow by half?", scenario: whatIf });
  expect([scenarioView.isError, scenarioView.data.result.scenario[0].value]).toEqual([false, 180]);
  expect(scenarioView.text).toContain("Nothing was saved: this is a what-if.");
  // auto-mapping a CSV, then importing its rows once an owner approved the kind and a property
  const csv = await ok("document_upload", { filename: "menu.csv", text: "product,price\nLatte,4.50\nMocha,5.00\n" });
  const mapped = await ok("ontology_automap", { document_id: csv.id });
  expect(["model", "profile_only"]).toContain(mapped.mode);
  const importArgs = { document_id: csv.id, kind: "menu_item", name_column: "product" };
  expect((await call("document_import_rows", importArgs)).isError).toBe(true);
  await ok("ontology_propose", {
    kinds: [{ kind: "menu_item" }],
    properties: [{ kind: "menu_item", name: "price", datatype: "money", unit: "AUD", source_map: [{ source: "document", record_type: csv.id, path: "price" }] }],
  });
  for (const [path, data] of [
    ["kinds", { kind: "menu_item", decision: { action: "approve" } }],
    ["properties", { kind: "menu_item", name: "price", decision: { action: "approve" } }],
  ] as const) {
    expect((await request.post(`/api/vaults/${vaultOf}/ontology/${path}/decide`, { data })).ok()).toBeTruthy();
  }
  const imported = await ok("document_import_rows", importArgs);
  expect([imported.created, imported.values, imported.bad_count]).toEqual([2, 2, 0]);
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
  // roles and ownership: promote, demote, hand over and back (an owner always remains)
  expect((await ok("vault_get", { vault_id: team.id })).member_count).toBe(2);
  expect((await ok("vault_check", { vault_id: team.id })).ok).toBe(true);
  expect((await ok("vault_repair", { vault_id: team.id })).repaired).toBeTruthy();
  expect((await ok("vault_set_role", { vault_id: team.id, email: friend.email, role: "owner" })).role).toBe("owner");
  expect((await ok("vault_set_role", { vault_id: team.id, email: friend.email, role: "member" })).role).toBe("member");
  expect((await ok("vault_transfer_ownership", { vault_id: team.id, email: friend.email })).role).toBe("owner");
  expect((await ok("vault_transfer_ownership", { vault_id: team.id, email: me.email }, friend.token)).role).toBe("owner");
  expect((await ok("vault_leave", { vault_id: team.id }, friend.token)).left).toBe(true);
  // invitations an agent can answer itself: see it, decline it, be invited again, accept it
  await ok("vault_invite", { vault_id: team.id, email: friend.email });
  expect((await ok("vault_invitations", {}, friend.token)).results.map((i: { vault_id: string }) => i.vault_id)).toContain(team.id);
  expect((await ok("vault_decline_invitation", { vault_id: team.id }, friend.token)).declined).toBe(true);
  await ok("vault_invite", { vault_id: team.id, email: friend.email });
  expect((await ok("vault_accept_invitation", { vault_id: team.id }, friend.token)).role).toBe("member");
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
  const skillPage = await ok("skill_list", { limit: 1 });
  expect(skillPage.skills.map((s: { name: string }) => s.name)).toEqual(["deploy-rollback"]);
  expect(skillPage.next).toBeNull();
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

  // peer vaults (no second Eunomia here): a share code (an error when this install knows no address), the
  // vault's peers, and a bad code or an unknown link are errors
  const shared = await ok("vault_create", { name: `Peer share ${Date.now()}` });
  await call("vault_share_code", { vault_id: shared.id, role: "read" });
  expect((await ok("vault_peers", { vault_id: shared.id })).links).toEqual([]);
  expect((await call("vault_join_shared", { share_code: "eunomia1:nope" })).isError).toBe(true);
  expect((await call("vault_peer_sync", { link_id: "0".repeat(32) })).isError).toBe(true);
  // secret requests: the agent gets a request and a handle, never a value; nothing is filled here (only a
  // person in a browser can, see secret-requests.spec.ts), so the transform and the call are refused
  const asked = await ok("secret_request", { name: "every_tool_key", description: "coverage", target: { hosts: ["api.example.com"] } });
  expect(asked.status).toBe("pending");
  expect((await ok("secret_request_status", { request_id: asked.request_id })).status).toBe("pending");
  const handle = asked.handle.$secret as string;
  expect((await call("secret_transform", { op: "base64", inputs: { value: handle } })).data.code).toBe("secret.unavailable");
  expect((await call("http_request", { url: "https://api.example.com/", headers: { Authorization: { $secret: handle } } })).data.code).toBe("secret.unavailable");

  // coverage: every tool the server lists was exercised above
  const list = await request.post("/mcp", {
    headers: { Authorization: `Bearer ${me.token}` },
    data: { jsonrpc: "2.0", id: ++id, method: "tools/list" },
  });
  const names: string[] = (await list.json()).result.tools.map((t: { name: string }) => t.name);
  expect(names.filter((n) => !called.has(n)), "tools never called by this test").toEqual([]);
});

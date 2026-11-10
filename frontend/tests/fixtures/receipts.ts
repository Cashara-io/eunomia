import type { Page, WebSocketRoute } from "@playwright/test";
import fixture from "./ontology-ui.json";
import { profileSchema } from "../../src/lib/queries/objects";
import { mockOntology } from "./ontology-ui";
export function receiptFixture() {
  const profile = profileSchema.parse(structuredClone(fixture.profile));
  profile.sources[0].document!.note = "Current document. Original imported row number and revision were not retained; this row is matched by object name.";
  const base = profile.properties[0];
  profile.properties.push(
    { ...base, name: "discount", value: 0, source: "second" },
    { ...base, name: "stock", datatype: "integer", unit: null, state: "conflicting", value: 14, source: "third" },
    { ...base, name: "freight", state: "unavailable", value: null, source: "denied", source_state: "unavailable" },
  );
  profile.sources.push(
    { id: "second", state: "available", record: null, document: { id: "duplicate", filename: "products.csv", row: { line: 9, cells: { product: "House blend 1kg", discount: "0", marker: "second source only" } }, note: "Current document. Original imported row version was not retained." } },
    { id: "third", state: "available", record: { title: "Stocktake", source: "Warehouse", external_id: "stock-17", payload: { stock: 14, marker: "third source only" } } },
    { id: "denied", state: "unavailable", record: null, document: { id: "foreign", filename: "SECRET.csv", row: { line: 1, cells: { secret: "PROTECTED CROSS SPACE" } }, note: "must not show" } },
  );
  return profile;
}
export async function mockReceipts(page: Page) {
  const requests = await mockOntology(page);
  let socket: WebSocketRoute | undefined;
  await page.routeWebSocket("**/api/events/ws*", ws => { socket = ws; ws.send(JSON.stringify({ type: "hello", vaults: { business: 0 } })); });
  const state = { profile: receiptFixture(), profileReads: 0, mode: "ok", membership: "ok" };
  await page.route("**/api/vaults/business/members", async route => {
    if (state.membership === "pending") return;
    if (state.membership !== "ok") return route.fulfill({ status: state.membership === "denied" ? 403 : 503, json: { code: state.membership === "denied" ? "vault.forbidden" : "http.error", detail: "Unavailable" } });
    return route.fulfill({ json: { results: [{ user_id: "owner", name: "Alex", email: "owner@example.test", role: "owner" }] } });
  });
  await page.route("**/api/vaults/business/ontology/objects/*", route => {
    state.profileReads++;
    if (state.mode === "loading") return;
    if (state.mode === "error" || state.mode === "denied" || state.mode === "missing") return route.fulfill({ status: state.mode === "denied" ? 403 : state.mode === "missing" ? 404 : 503, json: { code: state.mode === "denied" ? "vault.forbidden" : state.mode === "missing" ? "entity.not_found" : "http.error", detail: "Unavailable" } });
    return route.fulfill({ json: state.profile });
  });
  return { state, requests, revoke: () => socket?.send(JSON.stringify({ type: "vault", vault: "business", op: "member_removed" })) };
}

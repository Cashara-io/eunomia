import type { Page } from "@playwright/test";
import fixture from "./ontology-ui.json";

export type State =
  | "populated"
  | "loading"
  | "empty"
  | "filtered"
  | "error"
  | "denied";
export async function mockOntology(
  page: Page,
  state: State = "populated",
  failPath = "/ontology",
) {
  const model = structuredClone(fixture.ontology);
  if (state === "empty") {
    model.kinds = [];
    model.links = [];
  }
  const requests: string[] = [];
  await page.routeWebSocket("**/api/events/ws*", (ws) =>
    ws.send(
      JSON.stringify({
        type: "hello",
        vaults: { business: 0 },
        ontology: { business: 100 },
      }),
    ),
  );
  await page.route("**/api/**", async (route) => {
    const u = new URL(route.request().url());
    requests.push(u.pathname + u.search);
    const p = u.pathname;
    const json = (body: unknown) => route.fulfill({ json: body });
    if (p === "/api/auth/me")
      return json({
        id: "owner",
        name: "Alex",
        email: "owner@example.test",
        onboarded: true,
        is_admin: true,
      });
    if (p === "/api/auth/bootstrap") return json({ has_users: true });
    if (p === "/api/vaults")
      return json({
        results: [
          {
            id: "business",
            name: "Banksia Coffee",
            kind: "org",
            role: "owner",
            created_at: "2026-11-10T03:32:00Z",
          },
        ],
      });
    if (p.includes(failPath) && !p.endsWith("/readiness")) {
      if (state === "loading") return;
      if (state === "error" || state === "denied")
        return route.fulfill({
          status: state === "error" ? 503 : 403,
          json: {
            detail:
              state === "denied"
                ? "You do not have access to this information in this space."
                : "Fixture read unavailable.",
            code: `http.${state === "error" ? 503 : 403}`,
          },
        });
    }
    if (p.endsWith("/ontology")) return json(model);
    if (p.endsWith("/readiness")) {
      const counts = (items: { status: string }[]) =>
        Object.fromEntries(
          ["approved", "proposed", "observed", "rejected"].map((status) => [
            status,
            items.filter((i) => i.status === status).length,
          ]),
        );
      return json({
        vault: "business",
        ready: false,
        strict: true,
        kinds: counts(model.kinds),
        properties: counts(model.kinds.flatMap((k) => k.properties)),
        links: counts(model.links),
        metrics: counts(model.kinds.flatMap((k) => k.metrics)),
        blockers:
          state === "empty"
            ? ["No approved metrics yet."]
            : ["7 definitions are waiting for review."],
      });
    }
    if (p.endsWith("/activity"))
      return json({
        events:
          state === "empty" || state === "filtered"
            ? []
            : fixture.events.slice(
                0,
                Number(u.searchParams.get("limit") ?? 30),
              ),
        next: 70,
        more: state !== "empty" && state !== "filtered",
      });
    if (p.endsWith("/objects")) {
      if (!u.searchParams.has("kind"))
        return json({
          kinds: model.kinds.map((k) => ({
            kind: k.kind,
            entities: k.entities,
          })),
        });
      return json({
        rows:
          state === "empty" || state === "filtered"
            ? []
            : [
                {
                  id: "house",
                  name: "House blend 1kg",
                  values: { price: { value: 20, unit: "AUD" } },
                  source_count: 2,
                },
              ],
        columns: [{ name: "price", datatype: "money", unit: "AUD" }],
        total: state === "empty" || state === "filtered" ? 0 : 1,
        offset: 0,
        limit: 25,
        kind_approved: true,
      });
    }
    if (p.includes("/objects/")) {
      const profile = structuredClone(fixture.profile);
      if (state === "empty") {
        profile.properties = [];
        profile.numbers = [];
        profile.links = [];
        profile.link_counts = [];
        profile.history = [];
        profile.sources = [];
        profile.memories = [];
        profile.summary = "";
        profile.warnings = [];
        profile.history_total = 0;
      }
      return json(profile);
    }
    if (p.includes("invitations")) return json({ results: [] });
    if (p.includes("secrets") || p === "/api/sources") return json([]);
    return json({ results: [] });
  });
  return requests;
}

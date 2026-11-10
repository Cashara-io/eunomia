import type { Page, WebSocketRoute } from "@playwright/test";
import type { OntologyOut } from "../../src/lib/gen";

export const at = "2026-11-10T03:32:00Z";
export const selected = JSON.stringify(["property", "stock_item", "unit_cost"]);
export const reviewUrl = `/ontology/review?space=business&selected=${encodeURIComponent(selected)}`;
const base = {
  status: "approved",
  origin: "agent",
  revision: 1,
  synonyms: [],
  merge_into: null,
  reason: "Matched the stock_item.csv unit_cost column to the cost definition.",
} satisfies Partial<OntologyOut["kinds"][number]>;
export function reviewFixture(): OntologyOut {
  return {
    vault: "business",
    strict: true,
    kinds: [
      {
        ...base,
        kind: "stock_item",
        built_in: false,
        entities: 6000,
        samples: ["House blend 1kg", "Decaf 1kg"],
        description: "A stocked item sold by Banksia Coffee.",
        properties: [
          {
            ...base,
            name: "unit_cost",
            status: "proposed",
            description: "Cost of one unit, excluding delivery.",
            datatype: "money",
            unit: "AUD",
            enum_values: null,
            values: 6000,
            samples: ["AUD 12.00", "AUD 13.20"],
            source_map: [
              {
                source: "stock_item.csv",
                record_type: "csv",
                path: "unit_cost",
              },
            ],
          },
          {
            ...base,
            name: "price",
            description: "Selling price of one unit, including GST.",
            datatype: "money",
            unit: "AUD",
            enum_values: null,
            values: 6000,
            samples: ["AUD 20.00", "AUD 24.50"],
            source_map: [
              { source: "stock_item.csv", record_type: "csv", path: "price" },
            ],
          },
          {
            ...base,
            name: "quantity",
            description: "Available stock units.",
            datatype: "integer",
            unit: null,
            enum_values: null,
            values: 6000,
            samples: ["100", "75"],
            source_map: [],
          },
        ],
        metrics: [
          {
            ...base,
            name: "gross_profit",
            status: "proposed",
            description: "Gross profit across stock items.",
            unit: "AUD",
            expr: { agg: "sum", property: "price" },
            expr_words: "Sum of price",
          },
        ],
      },
      ...["warehouse", "delivery_route"].map((kind) => ({
        ...base,
        kind,
        status: "proposed" as const,
        built_in: false,
        entities: 0,
        description: `A ${kind.replaceAll("_", " ")} in the business records.`,
        samples: [],
        properties: [],
        metrics: [],
      })),
    ],
    links: [
      {
        ...base,
        from_kind: "stock_item",
        to_kind: "warehouse",
        label: "stored_at",
        status: "proposed",
        description: "The warehouse holding an item.",
        count: 120,
        samples: [{ from: "House blend 1kg", to: "Fitzroy warehouse" }],
        source_map: [],
      },
    ],
  };
}
export async function mockReview(
  page: Page,
  options: {
    member?: boolean;
    omitRedirect?: boolean;
    afterDecision?: (count: number) => Promise<void>;
    mode?:
      | "success"
      | "network"
      | "stale"
      | "saving"
      | "empty"
      | "loading"
      | "error";
  } = {},
) {
  const model = reviewFixture();
  if (options.mode === "empty") {
    model.kinds = [];
    model.links = [];
  }
  let mode = options.mode;
  let member = options.member;
  let socket: WebSocketRoute | undefined;
  const mutations: Record<string, unknown>[] = [];
  const events = [
    {
      action: "proposed",
      revision: 1,
      actor_name: "Claude Code",
      at,
      reason: "Matched source columns",
    },
  ];
  await page.routeWebSocket("**/api/events/ws*", (ws) => {
    socket = ws;
    ws.send(
      JSON.stringify({
        type: "hello",
        vaults: { business: 0 },
        ontology: { business: 0 },
      }),
    );
  });
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const json = (body: unknown) => route.fulfill({ json: body });
    if (path === "/api/auth/me")
      return json({
        id: "owner",
        email: "owner@example.test",
        name: "Alex Morgan",
        onboarded: true,
        is_admin: true,
      });
    if (path === "/api/auth/bootstrap") return json({ has_users: true });
    if (path === "/api/vaults")
      return json({
        results: [
          {
            id: "business",
            name: "Banksia Coffee",
            kind: "org",
            role: member ? "member" : "owner",
            created_at: at,
          },
        ],
      });
    if (path.endsWith("/decide")) {
      const body = request.postDataJSON();
      mutations.push(body);
      if (mode === "saving") return;
      if (mode === "network") {
        mode = "success";
        return route.abort("failed");
      }
      const kind = model.kinds.find(
        (k) => k.kind === (body.kind ?? body.from_kind),
      );
      const item = path.includes("/properties/")
        ? kind?.properties.find((p) => p.name === body.name)
        : path.includes("/metrics/")
          ? kind?.metrics.find((m) => m.name === body.name)
          : path.includes("/links/")
            ? model.links.find((l) => l.label === body.label)
            : kind;
      if (!item) throw new Error("Missing fixture definition");
      if (mode === "stale") {
        item.description = "Cost per kilogram, excluding delivery.";
        item.revision = 2;
        if ("datatype" in item) item.unit = "AUD/kg";
        mode = "success";
        return route.fulfill({
          status: 409,
          json: {
            code: "ontology.stale_revision",
            detail: "This definition changed.",
          },
        });
      }
      if (body.decision.action === "describe")
        item.description = body.decision.description;
      else
        item.status =
          body.decision.action === "approve" ? "approved" : "rejected";
      if ("merge_into" in item)
        item.merge_into = options.omitRedirect ? null : body.decision.merge_into ?? null;
      item.revision += 1;
      events.unshift({
        action:
          body.decision.action === "describe"
            ? "described"
            : body.decision.merge_into
              ? "merged"
              : item.status,
        revision: item.revision,
        actor_name: "Alex Morgan",
        at,
        reason: body.reason ?? null,
      });
      await options.afterDecision?.(mutations.length);
      return json(model);
    }
    if (path.endsWith("/ontology")) {
      if (mode === "loading") return;
      if (mode === "error")
        return route.fulfill({
          status: 503,
          json: {
            code: "fixture.unavailable",
            detail: "Could not load the ontology. Use Refresh to try again.",
          },
        });
      return json(model);
    }
    if (path.endsWith("/activity"))
      return json({
        events: events.map((event, i) => ({
          ...event,
          seq: events.length - i,
          actor: "fixture-actor",
          actor_type: event.action === "proposed" ? "token" : "user",
          definition_type: "property",
          kind: "stock_item",
          name: "unit_cost",
          target: "",
        })),
        next: 1,
        more: false,
      });
    if (path.endsWith("/readiness"))
      return json({
        vault: "business",
        ready: false,
        strict: true,
        blockers: [],
        kinds: {},
        properties: {},
        links: {},
        metrics: {},
      });
    if (path.includes("invitations")) return json({ results: [] });
    return json([]);
  });
  return {
    mutations,
    notifyOntologyChange() {
      socket?.send(JSON.stringify({ type: "ontology", vault: "business", event: { seq: 1 } }));
    },
    async setMember(value: boolean) {
      member = value;
      socket?.send(JSON.stringify({ type: "vault", vault: "business", op: "role_changed" }));
    },
    get model() {
      return model;
    },
    setMode(value: typeof mode) {
      mode = value;
    },
  };
}

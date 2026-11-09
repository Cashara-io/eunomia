import type { Spec } from "@json-render/core";
import { test, expect } from "@playwright/test";
import { registerAndOnboard, uniqueEmail } from "./helpers";
import { catalog, chartNames } from "../src/components/chat/ui/catalog";
import { readFileSync } from "node:fs";
import { execFileSync } from "node:child_process";

test("generated catalog copies match their definition and Docker contexts", () => {
  execFileSync("bun", ["scripts/chat-catalog.ts", "--check"]);
  expect(
    readFileSync("src/components/chat/ui/catalog.generated.json", "utf8"),
  ).toBe(readFileSync("../backend/src/views/catalog.generated.json", "utf8"));
});
const rows = [
  {
    name: "Alpha",
    value: 120.25,
    other: 70,
    entity_id: "entity:alpha",
    min: 0,
    max: 200,
    target: "Beta",
    date: "2026-01-01",
    group: "North",
  },
  {
    name: "Beta",
    value: 80.5,
    other: 35,
    entity_id: "entity:beta",
    min: 0,
    max: 200,
    target: "Gamma",
    date: "2026-02-01",
    group: "South",
  },
  {
    name: "Gamma",
    value: 150.75,
    other: 95,
    entity_id: "entity:gamma",
    min: 0,
    max: 200,
    target: "Alpha",
    date: "2026-03-01",
    group: "North",
  },
];
const entity = {
  id: "entity:alpha",
  name: "Alpha",
  kind: "sample",
  vault: "vault:test",
};
const metric = {
  metric: "total",
  kind: "sample",
  vault: "vault:test",
  expr_words: "Sum of value",
  value: 351.5,
  unit: "AUD",
  as_of: "2026-01-01",
  n_entities: 3,
  entity_ids: [entity.id],
  truncated: false,
  status: "approved",
  warnings: [],
};
const legacy: Record<string, object> = {
  Impact: {
    view: "impact",
    start: entity,
    headline: "A change",
    affected: [],
    linked: false,
    text: "No links",
  },
  Subgraph: {
    view: "subgraph",
    title: "Links",
    entities: [entity],
    relations: [],
    text: "Alpha",
  },
  Metric: { view: "metric", metric, entities: [entity], text: "351.5 AUD" },
  Scenario: {
    view: "scenario",
    headline: "A what-if",
    text: "No values saved",
    result: {
      vault: "vault:test",
      as_of: "2026-01-01",
      changes: [],
      baseline: [metric],
      scenario: [{ ...metric, value: 400, delta: 48.5, delta_pct: 13.797 }],
      changed: [],
      n_changed: 0,
      removed: [],
      reached: [],
      reached_truncated: false,
      warnings: [],
    },
  },
};
function fixture(kind: string, state = "full") {
  const source =
    kind === "Sankey"
      ? [
          { name: "Start", target: "Middle", value: 120.25 },
          { name: "Middle", target: "End", value: 80.5 },
        ]
      : kind === "Histogram"
        ? [
            { start: 80.5, end: 115.625, count: 1 },
            { start: 115.625, end: 150.75, count: 2 },
          ]
        : kind === "BoxPlot"
          ? [{ min: 80.5, q1: 100.375, median: 120.25, q3: 135.5, max: 150.75 }]
          : kind === "Violin"
            ? [
                { value: 80.5, count: 1 },
                { value: 120.25, count: 1 },
                { value: 150.75, count: 1 },
              ]
            : kind in legacy
              ? legacy[kind]
              : kind === "Stat"
                ? 351.5
                : kind === "Comparison"
                  ? { before: 120.25, after: 150.75 }
                  : kind === "Meter"
                    ? { value: 120.25, min: 0, max: 200 }
                    : state === "empty"
                      ? []
                      : state === "one"
                        ? rows.slice(0, 1)
                        : state === "null"
                          ? rows.map((r) => ({
                              ...r,
                              value: null,
                              other: null,
                            }))
                          : rows;
  const layout = [
    "Stack",
    "Grid",
    "Card",
    "Section",
    "Tabs",
    "Accordion",
  ].includes(kind);
  const copy = ["Heading", "Text", "Callout", "Badge", "Caption"].includes(
    kind,
  );
  const props = layout
    ? { title: kind }
    : copy
      ? { text: `${kind} words` }
      : kind === "Divider"
        ? {}
        : {
            title: kind,
            summary: "Fixture data for component coverage",
            data: { $state: "/sources/sample" },
            x:
              kind === "CalendarHeatmap"
                ? "date"
                : kind === "Scatter" || kind === "Bubble"
                  ? "other"
                  : "name",
            y: "value",
            format: "money",
            ...([
              "GroupedBar",
              "StackedBar",
              "StackedArea",
              "Combo",
              "SmallMultiples",
              "Line",
              "Area",
            ].includes(kind)
              ? { series: ["value", "other"] }
              : {}),
          };
  const resolved =
    state === "empty"
      ? []
      : state === "one" && Array.isArray(source)
        ? source.slice(0, 1)
        : state === "null" && Array.isArray(source)
          ? source.map((r) =>
              Object.fromEntries(
                Object.entries(r).map(([key, v]) => [
                  key,
                  typeof v === "number" ? null : v,
                ]),
              ),
            )
          : source;
  const spec: Spec = {
    root: "a",
    elements: {
      a: { type: kind, props, children: layout ? ["child"] : [] },
      ...(layout
        ? {
            child: {
              type: "Text",
              props: { text: "Child content" },
              children: [],
            },
          }
        : {}),
    },
  };
  return {
    view: "ui",
    text: "Source total 351.5 AUD",
    spec,
    data: { sources: { sample: resolved } },
  };
}
function history(views: object[]) {
  return views.flatMap((view, i) => [
    {
      role: "assistant",
      content: "",
      tool_calls: [
        {
          id: `call_${i}`,
          type: "function",
          function: { name: "show_view", arguments: "{}" },
        },
      ],
    },
    {
      role: "tool",
      tool_call_id: `call_${i}`,
      name: "show_view",
      content: JSON.stringify(view),
    },
  ]);
}
for (const theme of ["light", "dark"])
  for (const width of [1280, 390]) {
    test(`catalog and chart fixtures in ${theme} at ${width}px`, async ({
      page,
    }) => {
      test.setTimeout(120000);
      await page.setViewportSize({ width, height: 900 });
      await registerAndOnboard(page, uniqueEmail("gen-ui"));
      await page.addInitScript((t) => {
        localStorage.setItem("eunomia-theme", t);
      }, theme);
      const errors: string[] = [];
      page.on("pageerror", (e) => errors.push(e.message));
      const names = catalog.componentNames;
      const views = names.map((name) => fixture(name));
      const broken = fixture("Stat");
      broken.spec.elements.a.props = {
        ...broken.spec.elements.a.props,
        data: { $state: "/sources/missing" },
      };
      views.push(broken);
      const unknown = fixture("Stat");
      unknown.spec.elements.a.type = "Unknown";
      views.push(unknown);
      const badProp = fixture("Stat");
      badProp.spec.elements.a.props.value = 999;
      views.push(badProp);
      const thread = {
        id: "chat_thread:fixture",
        title: "UI catalog",
        created_at: new Date().toISOString(),
        updated_at: new Date().toISOString(),
      };
      await page.route("**/api/chat/threads", (route) =>
        route.fulfill({ json: [thread] }),
      );
      await page.route("**/api/chat/threads/*/history", (route) =>
        route.fulfill({ json: history(views) }),
      );
      await page.goto("/chat");
      await expect(page.locator(".generative-ui")).toHaveCount(names.length);
      for (const name of chartNames) {
        const svg = page
          .locator(
            `svg[data-chart="${name === "SmallMultiples" ? "Line" : name}"]`,
          )
          .first();
        await expect(svg).toBeVisible();
        expect((await svg.boundingBox())?.height).toBeGreaterThan(80);
        await expect(
          svg.locator("path,rect,circle,line").first(),
        ).toBeAttached();
      }
      const networkLabel = await page
        .locator('svg[data-chart="Network"] g[aria-label]')
        .first()
        .getAttribute("aria-label");
      expect(networkLabel).not.toMatch(/(?:^|; )[xy]:/);
      for (const name of names.filter(
        (n) => !chartNames.some((c) => c === n),
      )) {
        if (["Impact", "Subgraph", "Metric", "Scenario"].includes(name))
          continue;
        await expect(
          page.locator(".generative-ui").nth(names.indexOf(name)),
        ).toContainText(name === "Divider" ? "" : name);
      }
      await expect(
        page.getByText(
          "This display could not be drawn. The source answer is below.",
        ),
      ).toHaveCount(3);
      await expect(page.getByText("A$351.50", { exact: true })).toBeVisible();
      expect(errors).toEqual([]);
      expect(
        await page.evaluate(
          () => document.documentElement.scrollWidth <= window.innerWidth,
        ),
      ).toBeTruthy();
      if (process.env.E2E_CAPTURE_CATALOG === "1")
        for (const [i, name] of names.entries())
          await page
            .locator(".generative-ui")
            .nth(i)
            .screenshot({
              path: `test-results/catalog/${theme}-${width}-${name}.png`,
              animations: "disabled",
              timeout: 10000,
            });
      await page.screenshot({
        path: `test-results/catalog-${theme}-${width}.png`,
        fullPage: true,
      });
      for (const state of ["empty", "one", "null"]) {
        await page.route("**/api/chat/threads/*/history", (route) =>
          route.fulfill({
            json: history(chartNames.map((n) => fixture(n, state))),
          }),
        );
        await page.reload();
        await expect(page.locator(".generative-ui")).toHaveCount(
          chartNames.length,
        );
        await expect(
          page.getByText("This display could not be drawn.", { exact: false }),
        ).toHaveCount(0);
        expect(errors).toEqual([]);
      }
    });
  }

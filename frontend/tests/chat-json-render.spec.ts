import type { Spec } from "@json-render/core";
import { test, expect, type Locator } from "@playwright/test";
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
              : { series: ["value"] }),
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
async function hoverMark(mark: Locator) {
  const shape = mark.locator("circle,rect,path,line").first();
  const point = await shape.evaluate((el) => {
    if (!(el instanceof SVGGeometryElement))
      throw new Error("Expected a chart mark");
    const box = el.getBBox(),
      matrix = el.getScreenCTM(),
      style = getComputedStyle(el);
    for (const fy of [0.5, 0.2, 0.8, 0.1, 0.9, 0.3, 0.7, 0.4, 0.6])
      for (const fx of [0.5, 0.2, 0.8, 0.1, 0.9, 0.3, 0.7, 0.4, 0.6]) {
        const p = new DOMPoint(box.x + box.width * fx, box.y + box.height * fy);
        if (
          ((style.fill !== "none" && el.isPointInFill(p)) ||
            (style.stroke !== "none" && el.isPointInStroke(p))) &&
          matrix
        ) {
          const screen = p.matrixTransform(matrix);
          const svg = el.closest("svg");
          if (!svg) throw new Error("Expected a chart SVG");
          const bounds = svg.getBoundingClientRect();
          return { x: screen.x - bounds.left, y: screen.y - bounds.top };
        }
      }
    throw new Error("No hover target in chart mark");
  });
  // SVGs have an area even when a mark is a horizontal or vertical line.
  // Hover with an SVG-relative point so Playwright can wait for stable layout,
  // scroll it into view and hit-test it before moving the pointer.
  await mark.locator("xpath=ancestor::*[local-name()='svg']").hover({ position: point });
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
        expect((await svg.boundingBox())?.height).toBeGreaterThan(
          name === "Sparkline" ? 30 : 80,
        );
        if (name === "Sparkline") {
          expect((await svg.boundingBox())?.height).toBeLessThanOrEqual(48);
          await expect(svg.locator('[class*="visx-axis"]')).toHaveCount(0);
        }
        const mark = svg.locator("[data-chart-mark]").first();
        await mark.focus();
        const accessibleValue = await mark.getAttribute("aria-label");
        await expect(page.getByRole("tooltip")).toHaveText(
          accessibleValue || "",
        );
        await mark.blur();
        await hoverMark(mark);
        await expect(page.getByRole("tooltip")).toHaveText(
          accessibleValue || "",
        );
        await page.mouse.move(0, 0);
        await expect(
          svg.locator("path,rect,circle,line").first(),
        ).toBeAttached();
      }
      const focusedMark = page.locator('svg[data-chart="Bar"] [data-chart-mark]').first();
      const hoveredMark = page.locator('svg[data-chart="Line"] [data-chart-mark]').first();
      await focusedMark.focus();
      await expect(page.getByRole("tooltip")).toHaveCount(1);
      await hoverMark(hoveredMark);
      await expect(page.getByRole("tooltip")).toHaveCount(1);
      await expect(page.getByRole("tooltip")).toHaveText(await hoveredMark.getAttribute("aria-label") || "");
      // A late blur from the old owner must not close the new owner's tooltip.
      await focusedMark.blur();
      await expect(page.getByRole("tooltip")).toHaveCount(1);
      await page.mouse.move(0, 0);
      await expect(page.getByRole("tooltip")).toHaveCount(0);
      for (const name of ["Line", "Bar", "Pie", "Scatter", "Heatmap"]) {
        const mark = page
          .locator(`svg[data-chart="${name}"] [data-chart-mark]`)
          .first();
        await hoverMark(mark);
        await expect(page.getByRole("tooltip")).toContainText("A$120.25");
        await page.mouse.move(0, 0);
      }
      await expect(
        page.locator('[data-ui-component="Donut"] .ui-donut-total'),
      ).toHaveText("A$351.50");
      await expect(
        page.locator('svg[data-chart="Network"] circle'),
      ).toHaveCount(3);
      await expect(
        page.locator('[data-ui-component="Heatmap"] .ui-colour-scale'),
      ).toContainText("A$80.50");
      await expect(
        page.locator('svg[data-chart="CalendarHeatmap"]'),
      ).toContainText("Thu");
      await expect(
        page.locator('svg[data-chart="CalendarHeatmap"]'),
      ).toContainText("Jan");
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
      await expect(
        page
          .locator('[data-ui-component="Stat"]')
          .getByText("A$351.50", { exact: true }),
      ).toBeVisible();
      expect(errors).toEqual([]);
      expect(
        await page.evaluate(
          () => document.documentElement.scrollWidth <= window.innerWidth,
        ),
      ).toBeTruthy();
      if (process.env.E2E_CAPTURE_CATALOG === "1")
        for (const [i, name] of names.entries()) {
          const box = await page.locator(".generative-ui").nth(i).boundingBox();
          if (box && box.height > 600)
            await page.setViewportSize({
              width,
              height: Math.ceil(box.height + 500),
            });
          await page
            .locator(".generative-ui")
            .nth(i)
            .screenshot({
              path: `test-results/catalog/${theme}-${width}-${name}.png`,
              animations: "disabled",
              timeout: 10000,
            });
          await page.setViewportSize({ width, height: 900 });
        }
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

for (const width of [1280, 390])
  test(`dense networks and nested cards at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await registerAndOnboard(page, uniqueEmail("ui-layout"));
    const views = [10, 30].map((count) => {
      const view = fixture("Network");
      return {
        ...view,
        data: {
          sources: {
            sample: Array.from({ length: count }, (_, i) => ({
              name: `Supplier ${i + 1}`,
              target: `Supplier ${((i + 1) % count) + 1}`,
              value: 1250.5 + i,
            })),
          },
        },
      };
    });
    const nested = fixture("Line");
    nested.spec = {
      root: "card",
      elements: {
        card: {
          type: "Card",
          props: { title: "Monthly sales" },
          children: ["grid"],
        },
        grid: { type: "Grid", props: { columns: "two" }, children: ["a", "b"] },
        a: nested.spec.elements.a,
        b: {
          type: "Stat",
          props: {
            title: "Total sales",
            summary: "Bound total",
            data: { $state: "/sources/total" },
            format: "money",
          },
          children: [],
        },
      },
    };
    const nestedView = {
      ...nested,
      data: {
        sources: {
          sample: rows.map((r) => ({ ...r, value: r.value * 10 })),
          total: 3515,
        },
      },
    };
    await page.route("**/api/chat/threads", (route) =>
      route.fulfill({
        json: [
          {
            id: "chat_thread:layout",
            title: "Layout fixtures",
            created_at: new Date().toISOString(),
            updated_at: new Date().toISOString(),
          },
        ],
      }),
    );
    await page.route("**/api/chat/threads/*/history", (route) =>
      route.fulfill({ json: history([...views, nestedView]) }),
    );
    await page.goto("/chat");
    for (const [index, count] of [10, 30].entries()) {
      const chart = page.locator('svg[data-chart="Network"]').nth(index);
      await expect(chart.locator("circle")).toHaveCount(count);
      await expect(chart.locator("text")).toHaveCount(count);
      const boxes = await chart.locator("text").evaluateAll((elements) =>
        elements.map((el) => {
          const b = el.getBoundingClientRect();
          return { x: b.x, y: b.y, right: b.right, bottom: b.bottom };
        }),
      );
      const box = await chart.boundingBox();
      if (!box) throw new Error("Missing network");
      for (const [i, a] of boxes.entries()) {
        expect(a.x).toBeGreaterThanOrEqual(box.x);
        expect(a.right).toBeLessThanOrEqual(box.x + box.width);
        for (const b of boxes.slice(i + 1))
          expect(
            a.right <= b.x ||
              b.right <= a.x ||
              a.bottom <= b.y ||
              b.bottom <= a.y,
          ).toBeTruthy();
      }
      await chart.locator("[data-chart-mark]").last().focus();
      await expect(page.getByRole("tooltip")).toContainText(
        `Supplier ${count}`,
      );
      await chart.locator("[data-chart-mark]").last().blur();
      if (process.env.E2E_CAPTURE_CATALOG === "1") {
        const component = page.locator(".generative-ui").nth(index);
        const bounds = await component.boundingBox();
        await page.setViewportSize({
          width,
          height: Math.ceil((bounds?.height || 900) + 500),
        });
        await component.screenshot({
          path: `test-results/catalog/network-${count}-${width}.png`,
        });
        await page.setViewportSize({ width, height: 900 });
      }
    }
    const card = page.locator(".generative-ui").nth(2);
    await expect(card.locator('[data-ui-component="Line"]')).toBeVisible();
    expect(
      await card
        .locator('[data-ui-component="Line"]')
        .evaluate((el) => getComputedStyle(el).borderTopWidth),
    ).toBe("0px");
    const axis = card.locator(".visx-axis-left text");
    for (const text of await axis.all()) {
      const bounds = await text.boundingBox(),
        parent = await card.boundingBox();
      if (bounds && parent) expect(bounds.x).toBeGreaterThanOrEqual(parent.x);
    }
    if (process.env.E2E_CAPTURE_CATALOG === "1") {
      const bounds = await card.boundingBox();
      await page.setViewportSize({
        width,
        height: Math.ceil((bounds?.height || 900) + 500),
      });
      await card.screenshot({
        path: `test-results/catalog/nested-${width}.png`,
      });
      await page.setViewportSize({ width, height: 900 });
    }
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= window.innerWidth,
      ),
    ).toBeTruthy();
  });

test("typed money keeps its currency and missing values are unavailable", async ({ page }) => {
  await registerAndOnboard(page, uniqueEmail("typed-values"));
  const bad = fixture("Bar");
  bad.spec.elements.a.props.y = "name";
  const usd = fixture("Stat");
  usd.spec.elements.a.props.data = { $state: "/sources/sample/rows/0/value" };
  const usdView = { ...usd, data: { sources: { sample: { rows: [{ value: 120.25 }], columns: [{ key: "value", datatype: "money", unit: "USD" }] } } } };
  const count = structuredClone(usdView);
  count.spec.elements.a.props.data = { $state: "/sources/sample/totals/value/count" };
  const countView = { ...count, data: { sources: { sample: { ...usdView.data.sources.sample, totals: { value: { count: 1 } } } } } };
  const totalRows = structuredClone(countView);
  totalRows.spec.elements.a.props.data = { $state: "/sources/sample/total_rows" };
  const totalRowsView = { ...totalRows, data: { sources: { sample: { ...usdView.data.sources.sample, total_rows: 1 } } } };
  const table = fixture("Table");
  table.spec.elements.a.props.columns = [{key: "value", label: "Amount", total: "sum"}];
  const tableView = { ...table, data: { sources: { sample: { ...usdView.data.sources.sample, totals: {value: {sum: 120.25}} } } } };
  const gauge = fixture("Gauge");
  gauge.spec.elements.a.props.series = ["other"];
  const gaugeView = { ...gauge, data: { sources: { sample: [{name: "Supplier", value: null, other: 12, min: 0, max: 10}] } } };
  const meter = fixture("Meter");
  const meterView = { ...meter, data: { sources: { sample: {value: 5, min: null, max: 10} } } };
  const chart = fixture("Bar");
  chart.spec.elements.a.props.y = "value";
  chart.spec.elements.a.props.data = { $state: "/sources/sample/rows" };
  const chartView = { ...chart, data: { sources: { sample: { rows: [{ name: "Supplier", value: 120.25 }], columns: usdView.data.sources.sample.columns } } } };
  const multi = fixture("Line");
  multi.spec.elements.a.props.data = { $state: "/sources/sample/rows" };
  const multiView = { ...multi, data: { sources: { sample: { rows: [{name: "Supplier", value: 120.25, other: 70}], columns: [{key: "value", datatype: "money", unit: "USD"}, {key: "other", datatype: "money", unit: "EUR"}] } } } };
  const group = fixture("StatGroup");
  const groupView = { ...group, data: { sources: { sample: { rows, columns: [{ key: "value", datatype: "money", unit: "USD" }] } } } };
  await page.route("**/api/chat/threads", route => route.fulfill({ json: [{ id: "chat_thread:typed", title: "Typed values", created_at: new Date().toISOString(), updated_at: new Date().toISOString() }] }));
  await page.route("**/api/chat/threads/*/history", route => route.fulfill({ json: history([bad, usdView, groupView, countView, chartView, totalRowsView, tableView, gaugeView, meterView, multiView]) }));
  await page.goto("/chat");
  await expect(page.locator('[data-ui-component="Bar"]').first()).toContainText("No numeric values to show.");
  await expect(page.locator('[data-ui-component="Bar"]').first().locator("svg")).toHaveCount(0);
  await expect(page.locator('[data-ui-component="Stat"]').first()).toContainText("USD 120.25");
  await expect(page.locator('[data-ui-component="Stat"]').last()).toContainText("1");
  await expect(page.locator('[data-ui-component="Stat"]').last()).not.toContainText("USD");
  await expect(page.locator('[data-ui-component="Bar"]').last()).toContainText("USD");
  await expect(page.locator(".ui-stat-tile")).toHaveCount(3);
  await expect(page.locator('[data-ui-component="Table"] tbody')).toContainText("USD 120.25");
  await expect(page.locator('[data-ui-component="Table"] tfoot')).toContainText("USD 120.25");
  await expect(page.locator('[data-ui-component="Gauge"]')).toContainText("No numeric values to show.");
  await expect(page.locator('[data-ui-component="Gauge"] svg')).toHaveCount(0);
  await expect(page.locator('[data-ui-component="Meter"]')).toContainText("No range available.");
  const multiMark = page.locator('[data-ui-component="Line"] [data-chart-mark]').first();
  await multiMark.focus();
  await expect(page.getByRole("tooltip")).toContainText("USD 120.25");
  await expect(page.getByRole("tooltip")).toContainText("EUR 70.00");
  expect(await page.locator('[data-ui-component="Stat"] .text-3xl').first().evaluate(el => getComputedStyle(el).fontVariantNumeric)).toBe("tabular-nums");
});

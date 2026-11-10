import { test, expect } from "@playwright/test";
import { mkdir } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { mockOntology } from "./fixtures/ontology-ui";
import { diagramFixture } from "./fixtures/chat-diagrams";
const output =
  process.platform === "darwin"
    ? "/private/tmp/ontology-design/after/genui-diagrams"
    : join(tmpdir(), "ontology-design/after/genui-diagrams");

for (const theme of ["light", "dark"])
  for (const width of [1440, 390]) {
    for (const type of ["neighbourhood", "path", "impact", "kinds"] as const) {
      test(`${type} saved chat, ${theme}, ${width}`, async ({ page }) => {
        await page.setViewportSize({ width, height: 1100 });
        await mockOntology(page);
        await page.addInitScript(
          (t) => localStorage.setItem("eunomia-theme", t),
          theme,
        );
        const view = diagramFixture(type),
          thread = {
            id: "chat_thread:diagram",
            title: "How our business connects",
            created_at: "2026-11-10T00:00:00Z",
            updated_at: "2026-11-10T00:00:00Z",
          };
        const history = [
          { role: "user", content: "Show how these objects connect." },
          {
            role: "assistant",
            content: "",
            tool_calls: [
              {
                id: "diagram",
                type: "function",
                function: { name: "show_view", arguments: "{}" },
              },
            ],
          },
          {
            role: "tool",
            tool_call_id: "diagram",
            name: "show_view",
            content: JSON.stringify(view),
          },
          { role: "assistant", content: "The recorded links are shown above." },
        ];
        await page.route("**/api/chat/threads", (r) =>
          r.fulfill({ json: [thread] }),
        );
        await page.route("**/api/chat/threads/*/history", (r) =>
          r.fulfill({ json: history }),
        );
        const errors: string[] = [];
        page.on("pageerror", (e) => errors.push(e.message));
        await page.goto("/chat");
        const card = page.locator(`[data-diagram="${type}"]`);
        await expect(card).toBeVisible();
        await expect(card.getByRole("img")).toBeVisible();
        await expect(card.getByRole("img")).toHaveAccessibleDescription(
          view.text,
        );
        await expect(
          card.getByRole("link", { name: "Open in Ontology" }),
        ).toHaveAttribute("href", /^\/ontology/);
        expect(
          await page.evaluate(
            () => document.documentElement.scrollWidth <= innerWidth,
          ),
        ).toBe(true);
        const picture = card.getByRole("img");
        const geometry = await picture.evaluate((svg) => {
          const boxes = (selector: string) =>
            [...svg.querySelectorAll(selector)].map((el) => {
              const r = el.getBoundingClientRect();
              return { x: r.x, y: r.y, w: r.width, h: r.height };
            });
          const labels = boxes("[data-label-box]"),
            nodes = boxes("[data-solid-box], [data-node-box] .plate");
          const intersects = (
            a: (typeof labels)[number],
            b: (typeof labels)[number],
          ) =>
            a.x < b.x + b.w &&
            a.x + a.w > b.x &&
            a.y < b.y + b.h &&
            a.y + a.h > b.y;
          const texts = [...svg.querySelectorAll("foreignObject")].map((el) => {
            const text = el.querySelector("div");
            return text
              ? (parseFloat(getComputedStyle(text).fontSize) *
                  el.getBoundingClientRect().width) /
                  Number(el.getAttribute("width"))
              : 0;
          });
          return {
            collision: labels.some(
              (a, i) =>
                nodes.some((b) => intersects(a, b)) ||
                labels.slice(i + 1).some((b) => intersects(a, b)),
            ),
            minimum: Math.min(...texts),
          };
        });
        expect(geometry.collision).toBe(false);
        expect(geometry.minimum).toBeGreaterThanOrEqual(11.9);
        const marker = picture.locator("marker");
        expect(Number(await marker.getAttribute("markerWidth"))).toBeGreaterThanOrEqual(16);
        await card.getByRole("region").evaluate((el) => el.scrollIntoView({ block: "end" }));
        const frame = await card.getByRole("region").boundingBox();
        const composer = await page.getByRole("textbox", { name: "Message", exact: true }).boundingBox();
        if (width === 390) {
          await page.setViewportSize({ width, height: 700 });
          await card.getByRole("region").evaluate((el) => el.scrollIntoView({ block: "end" }));
          const shortFrame = await card.getByRole("region").boundingBox();
          const shortComposer = await page.getByRole("textbox", { name: "Message", exact: true }).boundingBox();
          expect(shortFrame?.height).toBeLessThanOrEqual(340);
          expect(shortFrame && shortComposer && shortFrame.y + shortFrame.height <= shortComposer.y).toBe(true);
          await page.setViewportSize({ width, height: 1100 });
        }
        expect(frame && composer && frame.y + frame.height <= composer.y).toBe(true);
        const layout = await card.locator("svg").first().innerHTML();
        await card.getByRole("button", { name: "Table", exact: true }).click();
        await expect(card.getByRole("table")).toHaveCount(2);
        await expect(card.getByRole("table").last()).toContainText(
          "delivered to",
        );
        await card
          .getByRole("button", { name: "Diagram", exact: true })
          .click();
        await page.reload();
        await expect(card).toBeVisible();
        expect(await card.locator("svg").first().innerHTML()).toBe(layout);
        await mkdir(output, { recursive: true });
        await card.evaluate((el) => el.scrollIntoView({ block: "end" }));
        await page.screenshot({
          path: `${output}/${type}-${width}-${theme}.png`,
          fullPage: true,
        });
        expect(errors).toEqual([]);
      });
    }
    test(`Overview reference, ${theme}, ${width}`, async ({ page }) => {
      await page.setViewportSize({ width, height: 1100 });
      await mockOntology(page);
      await page.addInitScript(
        (t) => localStorage.setItem("eunomia-theme", t),
        theme,
      );
      await page.goto("/ontology?space=business");
      await expect(page.locator("#kind-map-title")).toBeVisible();
      await mkdir(output, { recursive: true });
      await page.screenshot({
        path: `${output}/overview-${width}-${theme}.png`,
        fullPage: true,
      });
    });
  }

test("diagram catalogue source and bounded partial view keep text and values", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 1100 });
  await mockOntology(page);
  const view = diagramFixture("impact");
  view.diagram.nodes = view.diagram.nodes.filter((n) => n.distance !== 1);
  view.diagram.bands = [
    { distance: 0, shown: 1, omitted: 0, x: 150 },
    { distance: 1, shown: 0, omitted: 14, x: 410 },
    { distance: 2, shown: 1, omitted: 0, x: 670 },
    { distance: 3, shown: 1, omitted: 0, x: 930 },
  ];
  view.diagram.edges = view.diagram.edges.filter((e) => e.from !== "entity:item1" && e.to !== "entity:item1");
  view.diagram.edges[0].label =
    "has_a_particularly_long_relationship_with_this_delivery";
  view.diagram.partial = true;
  view.diagram.total_partial = true;
  view.diagram.total_nodes = 150;
  view.diagram.groups = [{ kind: "order", count: 146 }];
  view.diagram.omitted_edges = 200;
  view.diagram.nodes[0].metrics = [
    { metric: "order_count", value: 42, unit: "orders", n_entities: 42 },
  ];
  const shown = {
    view: "ui",
    text: view.text,
    spec: {
      root: "diagram",
      elements: {
        diagram: {
          type: "Diagram",
          props: {
            title: "Connections",
            summary: "Recorded links",
            data: { $state: "/sources/graph" },
          },
          children: [],
        },
      },
    },
    data: { sources: { graph: view } },
  };
  await page.route("**/api/chat/threads", (r) =>
    r.fulfill({
      json: [
        {
          id: "chat_thread:diagram",
          title: "Bounded diagram",
          created_at: "2026-11-10",
          updated_at: "2026-11-10",
        },
      ],
    }),
  );
  await page.route("**/api/chat/threads/*/history", (r) =>
    r.fulfill({
      json: [
        {
          role: "assistant",
          content: "",
          tool_calls: [
            {
              id: "diagram",
              type: "function",
              function: { name: "show_view", arguments: "{}" },
            },
          ],
        },
        {
          role: "tool",
          name: "show_view",
          tool_call_id: "diagram",
          content: JSON.stringify(shown),
        },
      ],
    }),
  );
  await page.goto("/chat");
  const card = page.locator('[data-diagram="impact"]');
  await expect(card).toBeVisible();
  expect(
    await card
      .getByRole("img")
      .locator("[data-label-box] foreignObject div")
      .evaluateAll((elements) =>
        elements.every((el) => {
          const range = document.createRange();
          range.selectNodeContents(el);
          return (
            range.getBoundingClientRect().height <=
            el.getBoundingClientRect().height
          );
        }),
      ),
  ).toBe(true);
  await expect(
    card.getByRole("heading", { name: "Connections", exact: true }),
  ).toBeVisible();
  await expect(card.getByRole("img")).toHaveAccessibleName("Connections");
  await expect(card.getByRole("img")).toContainText("1 hop: 14 not shown");
  await expect(card.getByRole("img")).toContainText("2 hops: 1 shown");
  await expect(card).toContainText("At least 150 objects");
  await expect(card).toContainText("146 more order");
  await card.getByRole("button", { name: "Table", exact: true }).click();
  await expect(card.getByRole("table").first()).toContainText("42 orders");
  await expect(card.getByRole("table").first()).toContainText("2 hops");
  await expect(card).toContainText("1 hop: 14 not shown");
});

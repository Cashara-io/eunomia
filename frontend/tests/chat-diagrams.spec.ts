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
        await card.scrollIntoViewIfNeeded();
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
  await expect(card).toContainText("At least 150 objects");
  await expect(card).toContainText("146 more order");
  await card.getByRole("button", { name: "Table", exact: true }).click();
  await expect(card.getByRole("table").first()).toContainText("42 orders");
});

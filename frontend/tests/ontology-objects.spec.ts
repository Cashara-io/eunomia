import { test, expect, type Page } from "@playwright/test";
import { registerAndOnboard, uniqueEmail } from "./helpers";
async function choose(page: Page, label: string, option: string) {
  await page.getByRole("combobox", { name: label, exact: true }).click();
  await page.getByRole("option", { name: option, exact: true }).click();
}

test("typed objects, server pages, sources, scoped numbers and related objects", async ({
  page,
}) => {
  test.setTimeout(180_000);
  await registerAndOnboard(page, uniqueEmail("objects"));
  const vault = await (
    await page.request.post("/api/vaults", {
      data: { name: "Banksia objects" },
    })
  ).json();
  const { token } = await (
    await page.request.post("/api/auth/tokens", { data: { name: "Fixture" } })
  ).json();
  async function tool(name: string, args: object) {
    const res = await page.request.post("/mcp", {
      headers: {
        Authorization: `Bearer ${token}`,
        Accept: "application/json, text/event-stream",
      },
      data: {
        jsonrpc: "2.0",
        id: 1,
        method: "tools/call",
        params: { name, arguments: args },
      },
    });
    expect(res.ok()).toBeTruthy();
    const body = await res.json();
    expect(body.result.isError).toBeFalsy();
    return JSON.parse(body.result.content[0].text);
  }
  await tool("ontology_propose", {
    vault_id: vault.id,
    kinds: [{ kind: "product" }],
    properties: [
      { kind: "product", name: "price", datatype: "money", unit: "AUD" },
      { kind: "product", name: "margin", datatype: "percent" },
    ],
    metrics: [
      {
        kind: "product",
        name: "total_price",
        expr: { agg: "sum", property: "price" },
      },
    ],
  });
  for (const [route, body] of [
    ["kinds", { kind: "product" }],
    ["properties", { kind: "product", name: "price" }],
    ["properties", { kind: "product", name: "margin" }],
    ["metrics", { kind: "product", name: "total_price" }],
  ] satisfies [string, object][]) {
    expect(
      (
        await page.request.post(
          `/api/vaults/${vault.id}/ontology/${route}/decide`,
          { data: { ...body, revision: 1, decision: { action: "approve" } } },
        )
      ).ok(),
    ).toBeTruthy();
  }
  const ids: string[] = [];
  for (let i = 0; i < 32; i++) {
    const entity = await (
      await page.request.post("/api/entities", {
        data: {
          kind: "product",
          name: `Basket ${String(i).padStart(2, "0")}`,
          vault_id: vault.id,
        },
      })
    ).json();
    ids.push(entity.id);
    if (i < 31)
      await tool("entity_set_property", {
        entity_id: entity.id,
        property: "price",
        value: i,
      });
  }
  await tool("entity_set_property", {
    entity_id: ids[0],
    property: "margin",
    value: 0.25,
  });
  await page.request.post(`/api/entities/${ids[0]}/relations`, {
    data: { to_id: ids[1], label: "supplied_by" },
  });
  await page.request.post(`/api/entities/${ids[0]}/memory`, {
    data: { text: "Review this product before the summer order." },
  });
  const csv = await tool("document_upload", {
    filename: "baskets.csv",
    text:
      "product,price,margin\n" +
      ids
        .map(
          (_, i) =>
            `Basket ${String(i).padStart(2, "0")},${i < 31 ? i : ""},${i === 0 ? 0.25 : ""}`,
        )
        .join("\n"),
    vault_id: vault.id,
  });
  await tool("document_import_rows", {
    document_id: csv.id,
    kind: "product",
    name_column: "product",
  });
  for (const [entity_id, property, value] of [
    [ids[0], "margin", 0.585], [ids[1], "margin", 0.4461], [ids[2], "price", 0.0049],
  ] satisfies [string, string, number][]) {
    await tool("entity_set_property", { entity_id, property, value });
  }
  const expensiveReads: string[] = [];
  page.on("request", (req) => {
    if (
      req.method() === "GET" &&
      /\/api\/vaults\/[^/]+\/ontology(?:\/readiness)?$/.test(
        new URL(req.url()).pathname,
      )
    )
      expensiveReads.push(req.url());
  });
  await page.goto(`/ontology/objects?space=${encodeURIComponent(vault.id)}`);
  await expect(page.getByRole("table").locator("tbody tr")).toHaveCount(25);
  await expect(page.getByRole("table")).toContainText("AUD 0.00");
  await expect(page.getByRole("table")).toContainText("58.50%");
  await expect(page.getByRole("table")).toContainText("44.61%");
  await expect(page.getByRole("table")).toContainText("AUD 0.0049");
  await page.getByRole("button", { name: "Columns", exact: true }).click();
  await page.getByRole("checkbox", { name: "Margin", exact: true }).uncheck();
  await expect(
    page.getByRole("columnheader", { name: "Margin", exact: true }),
  ).toHaveCount(0);
  await page.getByRole("checkbox", { name: "Margin", exact: true }).check();
  await page.getByRole("button", { name: "Columns", exact: true }).click();
  await page.getByRole("button", { name: "Next", exact: true }).click();
  await expect(page.getByRole("table").locator("tbody tr")).toHaveCount(7);
  await expect(
    page.getByRole("row").filter({ hasText: "Basket 31" }),
  ).toContainText("Missing");
  await page.locator("summary").filter({ hasText: "Filters" }).click();
  await choose(page, "Filter property", "Price");
  await choose(page, "Filter condition", "At least");
  await page.getByLabel("Value", { exact: true }).fill("29");
  await expect(page.getByRole("table").locator("tbody tr")).toHaveCount(2);
  await expect(page.getByRole("table")).toContainText("Basket 29");
  await page
    .getByRole("button", { name: "Clear filters", exact: true })
    .click();
  await expect(page.getByRole("table").locator("tbody tr")).toHaveCount(25);
  await page.locator("summary").filter({ hasText: "Filters" }).click();
  await page.getByRole("button", { name: "Next", exact: true }).click();
  await expect(page.getByRole("table").locator("tbody tr")).toHaveCount(7);
  await page.getByLabel("Search objects").fill("Basket 00");
  await expect(page.getByRole("table").locator("tbody tr")).toHaveCount(1);
  await page.getByLabel("Search objects").fill("");
  await expect(page.getByRole("table").locator("tbody tr")).toHaveCount(25);
  await page.getByRole("button", { name: "Price", exact: true }).click();
  await page.getByRole("button", { name: "Price", exact: true }).click();
  await expect(
    page.getByRole("table").locator("tbody tr").first(),
  ).toContainText("Basket 30");
  await page.getByLabel("Search objects").fill("Basket 00");
  await expect(page.getByRole("table").locator("tbody tr")).toHaveCount(1);
  await tool("entity_set_property", {
    entity_id: ids[0],
    property: "price",
    value: 99,
  });
  await page.getByRole("button", { name: "Refresh", exact: true }).click();
  await expect(page.getByRole("table")).toContainText("AUD 99.00");
  await tool("entity_set_property", {
    entity_id: ids[0],
    property: "price",
    value: 0,
  });
  await page.getByRole("button", { name: "Refresh", exact: true }).click();
  await expect(page.getByRole("table")).toContainText("AUD 0.00");
  await page.getByRole("link", { name: "Basket 00", exact: true }).click();
  await expect(
    page.getByText("Approval covers the kind definition, not every value.", {
      exact: true,
    }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Numbers", exact: true }).click();
  await expect(page.getByRole("region", { name: "Numbers" })).toContainText(
    "AUD 0.00",
  );
  await page.getByText("Inspect inputs", { exact: true }).click();
  await expect(page.getByRole("region", { name: "Numbers" })).toContainText(
    "AUD 0.00",
  );
  await page
    .getByRole("button", { name: "Related memories", exact: true })
    .click();
  await expect(
    page.getByText("Review this product before the summer order.", {
      exact: true,
    }),
  ).toBeVisible();
  await page
    .getByRole("link", { name: "Open in People & topics", exact: true })
    .click();
  await page
    .getByRole("link", { name: "Open in Ontology", exact: true })
    .click();
  await expect(page).toHaveURL(
    new RegExp(`space=${encodeURIComponent(vault.id)}`),
  );
  await page.getByRole("button", { name: "Links", exact: true }).click();
  await page.getByRole("link", { name: "Basket 01", exact: true }).click();
  await expect(page).toHaveURL(new RegExp(encodeURIComponent(ids[1])));
  await expect(page).toHaveURL(/space=/);
  await page.setViewportSize({ width: 390, height: 844 });
  await choose(page, "Object section", "Sources");
  expect(expensiveReads).toEqual([]);
  await expect(page.getByRole("region", { name: "Sources" })).toContainText(
    "baskets.csv",
  );
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBeTruthy();
  await page.setViewportSize({ width: 320, height: 844 });
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBeTruthy();
});

test("Banksia-sized visual evidence in both themes and widths", async ({
  page,
}) => {
  test.skip(
    !process.env.ONTOLOGY_OBJECTS_FIXTURE,
    "Run against the isolated fixture server for screenshot evidence",
  );
  test.setTimeout(180_000);
  const { readFile, mkdir, writeFile } = await import("node:fs/promises");
  const { z } = await import("zod");
  const fixture = z
    .object({
      space: z.string(),
      object: z.string(),
      entities: z.literal(18878),
      subcent_object: z.string(),
    })
    .parse(
      JSON.parse(
        await readFile(process.env.ONTOLOGY_OBJECTS_FIXTURE ?? "", "utf8"),
      ),
    );
  const output =
    process.env.ONTOLOGY_OBJECTS_SCREENSHOTS ??
    "/private/tmp/eunomia-ontology2/screenshots";
  await mkdir(output, { recursive: true });
  expect(
    (
      await page.request.post("/api/auth/login", {
        data: {
          email: "tester@example.com",
          password: "correct-horse-battery",
        },
      })
    ).ok(),
  ).toBeTruthy();
  await page.goto(
    `/ontology/objects?space=${encodeURIComponent(fixture.space)}&kind=product`,
  );
  const renderTimes: {
    view: string;
    theme: string;
    width: number;
    ready_ms: number;
  }[] = [];
  for (const theme of ["light", "dark"]) {
    await page.evaluate(
      (theme) => localStorage.setItem("eunomia-theme", theme),
      theme,
    );
    for (const width of [1440, 390]) {
      await page.setViewportSize({ width, height: width === 390 ? 844 : 900 });
      const paths = [
        [
          "objects",
          `/ontology/objects?space=${encodeURIComponent(fixture.space)}&kind=product${width === 1440 ? `&selected=${encodeURIComponent(fixture.object)}` : ""}`,
        ],
        [
          "profile",
          `/ontology/objects/${encodeURIComponent(fixture.subcent_object)}?space=${encodeURIComponent(fixture.space)}&section=Properties`,
        ],
        [
          "numbers",
          `/ontology/objects/${encodeURIComponent(fixture.object)}?space=${encodeURIComponent(fixture.space)}&section=Numbers`,
        ],
        [
          "sources",
          `/ontology/objects/${encodeURIComponent(fixture.object)}?space=${encodeURIComponent(fixture.space)}&section=Sources`,
        ],
      ];
      for (const [name, path] of paths) {
        const started = Date.now();
        await page.goto(path);
        if (name === "objects") {
          await expect(page.getByRole("table").locator("tbody tr")).toHaveCount(
            25,
          );
          if (width === 1440)
            await expect(
              page.getByRole("link", { name: "Open object page" }),
            ).toBeVisible();
        } else
          await expect(
            page.getByRole("region", {
              name:
                name === "profile"
                  ? "Properties"
                  : name === "numbers"
                    ? "Numbers"
                    : "Sources",
              exact: true,
            }),
          ).toBeVisible();
        await expect(
          page.getByRole("alert").filter({ hasText: "Could not" }),
        ).toHaveCount(0);
        expect(
          await page.evaluate(
            () => document.documentElement.scrollWidth <= window.innerWidth,
          ),
        ).toBeTruthy();
        renderTimes.push({
          view: name,
          theme,
          width,
          ready_ms: Date.now() - started,
        });
        await page.screenshot({
          path: `${output}/${name}-${theme}-${width}.png`,
          fullPage: true,
        });
      }
    }
  }
  await writeFile(
    `${output}/render-timings.json`,
    JSON.stringify(renderTimes, null, 2),
  );
});

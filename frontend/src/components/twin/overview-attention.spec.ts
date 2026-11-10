import { test, expect } from "@playwright/test";
import { mockOntology } from "../../../tests/fixtures/ontology-ui";
import fixture from "../../../tests/fixtures/ontology-ui.json";

for (const count of [16, 60, 120])
  test(`all owner issues remain reachable across ${count} kinds`, async ({
    page,
  }) => {
    await mockOntology(page);
    await page.setViewportSize({ width: 1440, height: 1000 });
    if (count === 60)
      await page.addInitScript(() =>
        localStorage.setItem("eunomia-theme", "dark"),
      );
    const model = structuredClone(fixture.ontology);
    model.kinds = Array.from({ length: count }, (_, i) => ({
      ...model.kinds[0],
      kind: `kind_${i}`,
      entities: count - i,
      status: "approved",
      properties: [],
      metrics: [],
    }));
    model.links = [];
    const affected = count === 16 ? 2 : 30;
    for (const kind of model.kinds.slice(-affected))
      kind.properties = [
        {
          ...fixture.ontology.kinds[0].properties[0],
          name: "needs_owner",
          status: "proposed",
        },
      ];
    const profile = structuredClone(fixture.profile);
    profile.kind = model.kinds.at(-1)!.kind;
    profile.properties = profile.properties
      .filter((p) => p.state === "stale")
      .flatMap((p) => [p, { ...p, name: "other_stale_value" }]);
    await page.route("**/api/vaults/business/ontology", (r) =>
      r.fulfill({ json: model }),
    );
    await page.route("**/api/vaults/business/ontology/objects/house?*", (r) =>
      r.fulfill({ json: profile }),
    );
    await page.goto("/ontology?space=business");
    const staleCount = profile.properties.length;
    await expect(
      page.getByRole("link", {
        name: `Review ${affected} suggestions`,
        exact: true,
      }),
    ).toBeVisible();
    await expect(
      page.getByRole("region", { name: "Owner attention", exact: true }),
    ).toContainText(`${affected + staleCount} issues`);
    const pinCounts = await page.locator("[data-pin-type]").allTextContents();
    let reachable = pinCounts.reduce((n, text) => n + Number(text.trim()), 0);
    if (count > 16) {
      const more = page.getByRole("button", { name: /more kinds.*issues/ });
      await expect(more).toBeVisible();
      reachable += Number(
        await more
          .locator("[data-issue-count]")
          .getAttribute("data-issue-count"),
      );
      await more.click();
      const list = page.getByRole("region", {
        name: "More kinds",
        exact: true,
      });
      await expect(
        list.getByRole("checkbox", { name: "Needs attention only" }),
      ).toBeChecked();
      await expect(list.locator("li")).toHaveCount(affected - 4);
      for (const row of await list.locator("li").all()) {
        await expect(row.locator("[data-issue-count]")).toBeVisible();
        await expect(row.getByRole("link", { name: /Review/ })).toHaveAttribute(
          "href",
          /selected=/,
        );
      }
      await list.getByRole("searchbox").fill(`Kind ${count - 1}`);
      await list
        .getByRole("button", { name: `Kind ${count - 1}`, exact: true })
        .click();
    } else await page.locator(`[data-kind="kind_${count - 1}"]`).click();
    expect(reachable).toBe(affected + staleCount);
    const inspector = page.getByRole("region", {
      name: `Kind ${count - 1} pending issues`,
      exact: true,
    });
    await expect(
      inspector.getByRole("link", { name: "Review Needs owner" }),
    ).toHaveAttribute("href", /selected=/);
    await expect(
      inspector.getByRole("link", { name: /Inspect/ }),
    ).toHaveAttribute("href", /section=Properties/);
    await expect(inspector).toContainText(`${1 + staleCount} issues`);
    if (count === 60 && process.env.ONTOLOGY_SCREENSHOTS) {
      await page.addStyleTag({
        content: "nextjs-portal{display:none!important}",
      });
      await page.evaluate(() => window.scrollTo(0, 0));
      await page.screenshot({
        path: `${process.env.ONTOLOGY_SCREENSHOTS}/overview-60-kinds-low-count-issues.png`,
        fullPage: true,
      });
    }
  });

test("mobile route caption counts the complete flat list and has no route toggle", async ({
  page,
}) => {
  await mockOntology(page);
  await page.setViewportSize({ width: 390, height: 900 });
  await page.goto("/ontology?space=business");
  await expect(
    page.locator('[data-testid="kind-landscape"] > svg'),
  ).toBeHidden();
  await expect(
    page.locator('[data-testid="kind-landscape"] ul button:visible'),
  ).toHaveCount(48);
  await expect(
    page.getByText("All 48 recorded relationships listed", { exact: false }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Show all routes", exact: true }),
  ).toBeHidden();
});

test("long inspector actions wrap within their column", async ({ page }) => {
  await mockOntology(page);
  const model = structuredClone(fixture.ontology);
  const name =
    "kind_with_a_very_long_name_for_an_owner_to_open_without_overlapping_links";
  model.kinds[0].kind = name;
  await page.route("**/api/vaults/business/ontology", (r) =>
    r.fulfill({ json: model }),
  );
  await page.goto("/ontology?space=business");
  await page.locator(`[data-kind="${name}"]`).click();
  const action = page.getByRole("link", { name: /Open Kind with a very long/ });
  const bounds = await action.evaluate((el) => ({
    action: el.getBoundingClientRect().right,
    column: el.parentElement!.getBoundingClientRect().right,
  }));
  expect(bounds.action).toBeLessThanOrEqual(bounds.column);
  await expect(action).toHaveCSS("white-space", "normal");
});

test("object chart keeps largest and issue kinds, expands the rest in count order", async ({
  page,
}) => {
  await mockOntology(page);
  const model = structuredClone(fixture.ontology);
  model.kinds = Array.from({ length: 60 }, (_, i) => ({
    ...model.kinds[0],
    kind: `kind_${i}`,
    entities: 60 - i,
    status: "approved",
    properties: [],
    metrics: [],
  }));
  model.kinds[59].properties = [
    {
      ...fixture.ontology.kinds[0].properties[0],
      name: "needs_owner",
      status: "proposed",
    },
  ];
  model.links = [];
  await page.route("**/api/vaults/business/ontology", (r) =>
    r.fulfill({ json: model }),
  );
  await page.goto("/ontology?space=business");
  const chart = page.getByRole("region", {
    name: "Objects per kind",
    exact: true,
  });
  await expect(chart.getByRole("button", { name: /objects$/ })).toHaveCount(13);
  await expect(
    chart.getByRole("button", { name: "Kind 59s: 1 objects", exact: true }),
  ).toBeVisible();
  await chart
    .getByRole("button", { name: "Show all 60 kinds", exact: true })
    .click();
  await expect(chart.getByRole("button", { name: /objects$/ })).toHaveCount(60);
  const counts = await chart
    .locator("button[data-objects]")
    .evaluateAll((rows) =>
      rows.map((el) => Number(el.getAttribute("data-objects"))),
    );
  expect(counts).toEqual(Array.from({ length: 60 }, (_, i) => 60 - i));
});

test("most-linked selection shows only its routes with clear labels in dark mode", async ({
  page,
}) => {
  await mockOntology(page);
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.addInitScript(() => localStorage.setItem("eunomia-theme", "dark"));
  const links = fixture.ontology.links;
  const mostLinked = fixture.ontology.kinds
    .map((k) => ({
      kind: k.kind,
      count: links.filter((l) => l.from_kind === k.kind || l.to_kind === k.kind)
        .length,
    }))
    .sort((a, b) => b.count - a.count || a.kind.localeCompare(b.kind))[0];
  await page.goto("/ontology?space=business");
  await page
    .getByRole("button", { name: "Show all routes", exact: true })
    .click();
  await page.locator(`[data-kind="${mostLinked.kind}"]`).click();
  await page.mouse.move(0, 0);
  await expect(page.locator("[data-route]:visible")).toHaveCount(
    mostLinked.count,
  );
  const neighbours = new Set([
    mostLinked.kind,
    ...links
      .filter(
        (l) => l.from_kind === mostLinked.kind || l.to_kind === mostLinked.kind,
      )
      .flatMap((l) => [l.from_kind, l.to_kind]),
  ]);
  const unrelatedIndex = fixture.ontology.kinds.findIndex(
    (k) => !neighbours.has(k.kind),
  );
  await expect(
    page.locator(`svg > g[data-form="k${unrelatedIndex}"]`),
  ).toHaveCSS("opacity", "0.12");
  const boxes = await page
    .locator("[data-route-label]:visible")
    .evaluateAll((els) =>
      els.map((el) => {
        const r = el.getBoundingClientRect();
        return { left: r.left, right: r.right, top: r.top, bottom: r.bottom };
      }),
    );
  expect(boxes.length).toBeGreaterThan(0);
  for (let i = 0; i < boxes.length; i++)
    for (const b of boxes.slice(i + 1)) {
      const a = boxes[i];
      expect(
        a.left < b.right &&
          a.right > b.left &&
          a.top < b.bottom &&
          a.bottom > b.top,
      ).toBe(false);
    }
  if (process.env.ONTOLOGY_SCREENSHOTS) {
    await page.addStyleTag({
      content: "nextjs-portal{display:none!important}",
    });
    await page.screenshot({
      path: `${process.env.ONTOLOGY_SCREENSHOTS}/overview-selected-most-linked-1440-dark.png`,
      fullPage: true,
    });
  }
});

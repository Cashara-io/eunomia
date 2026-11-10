import { test, expect } from "@playwright/test";
import { mockOntology } from "../../../tests/fixtures/ontology-ui";
import fixture from "../../../tests/fixtures/ontology-ui.json";

test("complete sentences keep the direction from both ends, routes and tooltips", async ({
  page,
}) => {
  await mockOntology(page);
  await page.goto("/ontology?space=business");
  for (const [kind, direction] of [
    ["location", "in"],
    ["order", "out"],
  ]) {
    await page.locator(`[data-kind="${kind}"]`).click();
    await expect(
      page.getByRole("region", { name: `Links ${direction}`, exact: true }),
    ).toContainText("Order delivered to Location");
    await expect(page.locator(`[data-kind="${kind}"]`)).toHaveAccessibleName(
      /Order delivered to Location/,
    );
  }
  await page
    .getByRole("button", { name: "Show all routes", exact: true })
    .click();
  for (const route of await page.locator("[data-route]").all()) {
    await expect(route).toHaveAttribute("marker-end", /arrow/);
    await expect(route).toHaveAttribute("data-from", /.+/);
    await expect(route).toHaveAttribute("data-to", /.+/);
  }
  for (const label of await page.locator("[data-route-label]").all()) {
    await expect(label).toHaveAttribute("title", /.+ .+ .+/);
    expect(
      Number.isFinite(Number(await label.getAttribute("data-arrow-angle"))),
    ).toBe(true);
  }
});
test("stale fixture explains the saved value on every entry", async ({
  page,
}) => {
  await mockOntology(page);
  await page.goto("/ontology?space=business");
  await expect(
    page.getByText(
      "The source changed after this value was saved. Check the value.",
      { exact: true },
    ),
  ).toHaveCount(1);
  await expect(
    page.getByRole("link", { name: /A place to start/ }),
  ).toContainText("Source changed after this value was saved");
  await expect(page.locator('[data-pin-type="stale"]')).toHaveAccessibleName(
    /source changed after this value was saved/i,
  );
  await expect(page.locator("main")).not.toContainText(
    "current source is out of date",
  );
});
test("grouped proposal pins expose every issue", async ({ page }) => {
  await mockOntology(page);
  await page.goto("/ontology?space=business");
  await page.route("**/api/vaults/business/ontology", (route) => {
    const model = structuredClone(fixture.ontology);
    model.kinds[0].properties[0].status = "proposed";
    model.kinds[0].properties[1].status = "proposed";
    return route.fulfill({ json: model });
  });
  await page.reload();
  await page
    .locator('[data-kind="product"]')
    .locator("..")
    .getByRole("button", { name: /proposals to review/ })
    .click();
  const list = page.getByRole("region", { name: "Product issues" });
  await expect(list).toBeFocused();
  await expect(list.getByRole("link")).toHaveCount(2);
  for (const link of await list.getByRole("link").all())
    await expect(link).toHaveAttribute("href", /space=business&selected=/);
});
for (const theme of ["light", "dark"])
  test(`idle routes meet contrast in ${theme} and reveal on interaction`, async ({
    page,
  }) => {
    await mockOntology(page);
    await page.addInitScript(
      (theme) => localStorage.setItem("eunomia-theme", theme),
      theme,
    );
    await page.goto("/ontology?space=business");
    const routes = page.locator("[data-route]:visible");
    await expect(routes).toHaveCount(5);
    await expect(
      page.getByText("5 of 48 routes shown", { exact: false }),
    ).toBeVisible();
    const ratios = await routes.evaluateAll((paths) =>
      paths.map((path) => {
        const ctx = document.createElement("canvas").getContext("2d")!;
        function rgb(value: string) {
          ctx.fillStyle = value;
          ctx.fillRect(0, 0, 1, 1);
          return [...ctx.getImageData(0, 0, 1, 1).data].slice(0, 3);
        }
        function lum(c: number[]) {
          const v = c.map((n) => {
            n /= 255;
            return n <= 0.04045 ? n / 12.92 : ((n + 0.055) / 1.055) ** 2.4;
          });
          return v[0] * 0.2126 + v[1] * 0.7152 + v[2] * 0.0722;
        }
        const css = getComputedStyle(path);
        const ground = rgb(
          getComputedStyle(
            document.querySelector('[data-testid="kind-landscape"]')!,
          ).backgroundColor,
        );
        const ink = rgb(css.stroke).map(
          (n, i) =>
            n * Number(css.opacity) + ground[i] * (1 - Number(css.opacity)),
        );
        const a = lum(ink),
          b = lum(ground);
        return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
      }),
    );
    expect(Math.min(...ratios)).toBeGreaterThanOrEqual(3);
    await page.locator('[data-kind="order"]').hover();
    expect(await routes.count()).toBeGreaterThan(5);
    await page
      .getByRole("button", { name: "Show all routes", exact: true })
      .click();
    await expect(routes).toHaveCount(48);
  });
for (const count of [1, 16, 60, 120])
  test(`bounded canvas with ${count} kinds and 300 links keeps geometry stable`, async ({
    page,
  }) => {
    await page.addInitScript(() => {
      const renders: Record<string, number> = {};
      const seenProps = new WeakSet<object>();
      Object.assign(window, {
        landscapeRenders: renders,
        __REACT_DEVTOOLS_GLOBAL_HOOK__: {
          supportsFiber: true,
          renderers: new Map(),
          inject: () => 1,
          onCommitFiberUnmount: () => {},
          onCommitFiberRoot: (_id: number, root: { current: unknown }) => {
            type Fiber = {
              type?: unknown;
              memoizedProps?: Record<string, unknown>;
              child?: Fiber;
              sibling?: Fiber;
            };
            function visit(node: Fiber | undefined) {
              if (!node) return;
              const props = node.memoizedProps;
              const name =
                node.type === "button" && props?.["data-kind"]
                  ? "LandscapeLandmark"
                  : node.type === "svg" &&
                      typeof props?.viewBox === "string" &&
                      props.viewBox.startsWith("0 0 1200 ")
                    ? "LandscapePlane"
                    : "";
              if (
                name &&
                node.memoizedProps &&
                !seenProps.has(node.memoizedProps)
              ) {
                seenProps.add(node.memoizedProps);
                renders[name] = (renders[name] ?? 0) + 1;
              }
              visit(node.child);
              visit(node.sibling);
            }
            visit(root.current as Fiber);
          },
        },
      });
    });
    await mockOntology(page);
    const model = structuredClone(fixture.ontology);
    model.kinds = Array.from({ length: count }, (_, i) => ({
      ...model.kinds[0],
      kind: `非常に長い名前${i}ABCDEFGHIJKLMNOPQRSTUVWXYZ`,
      entities: count - i,
      properties: [],
      metrics: [],
    }));
    model.links = Array.from({ length: 300 }, (_, i) => ({
      ...model.links[0],
      label: `route_${i}`,
      from_kind: model.kinds[i % count].kind,
      to_kind: model.kinds[(i + 1) % count].kind,
      count: 300 - i,
    }));
    await page.route("**/api/vaults/business/ontology", (route) =>
      route.fulfill({ json: model }),
    );
    if (count === 60)
      await page.addInitScript(() =>
        localStorage.setItem("eunomia-theme", "dark"),
      );
    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.goto("/ontology?space=business");
    await expect(page.locator("[data-kind]")).toHaveCount(Math.min(count, 16));
    const svg = page.locator('[data-testid="kind-landscape"] > svg');
    expect(
      Number((await svg.getAttribute("viewBox"))?.split(" ")[3]),
    ).toBeLessThanOrEqual(1200);
    const boxes = await page.locator("[data-kind]").evaluateAll((nodes) =>
      nodes.map((n) => {
        const r = n.getBoundingClientRect();
        return { x: r.x, y: r.y, w: r.width, h: r.height };
      }),
    );
    for (let i = 0; i < boxes.length; i++)
      for (let j = i + 1; j < boxes.length; j++) {
        const a = boxes[i],
          b = boxes[j];
        expect(
          a.x < b.x + b.w &&
            a.x + a.w > b.x &&
            a.y < b.y + b.h &&
            a.y + a.h > b.y,
        ).toBe(false);
      }
    await expect(
      page.locator("[data-kind]").first().locator("[title]"),
    ).toHaveAttribute("title", /非常/);
    await page.evaluate(() => {
      const changes: MutationRecord[] = [];
      new MutationObserver((records) => changes.push(...records)).observe(
        document.querySelector('[data-testid="kind-landscape"] > svg')!,
        { subtree: true, attributes: true, childList: true },
      );
      Object.assign(window, { routeChanges: changes });
    });
    await expect(
      page.getByRole("heading", { name: "House blend 1kg: Unit cost" }),
    ).toBeVisible();
    const rendersBefore = await page.evaluate(() => ({
      ...Reflect.get(window, "landscapeRenders"),
    }));
    expect(rendersBefore.LandscapePlane).toBeGreaterThan(0);
    expect(rendersBefore.LandscapeLandmark).toBeGreaterThan(0);
    for (const node of await page.locator("[data-kind]").all()) {
      await node.hover();
      const token = await node.locator("..").getAttribute("data-form");
      await expect(page.locator(`svg > g[data-form="${token}"]`)).toHaveCSS(
        "opacity",
        "1",
      );
    }
    expect(
      await page.evaluate(() => Reflect.get(window, "landscapeRenders")),
    ).toEqual(rendersBefore);
    await page.locator("[data-kind]").first().click();
    expect(
      await page.evaluate(() => Reflect.get(window, "routeChanges").length),
    ).toBe(0);
    if (count === 60 && process.env.ONTOLOGY_SCREENSHOTS)
      await page.screenshot({
        path: `${process.env.ONTOLOGY_SCREENSHOTS}/overview-60-kinds-1440-dark.png`,
        fullPage: true,
      });
    if (count > 16) {
      await page
        .getByRole("button", { name: `${count - 16} more kinds`, exact: true })
        .click();
      await page
        .getByRole("searchbox", { name: "Find a kind" })
        .fill(model.kinds.at(-1)!.kind);
      await expect(
        page
          .getByRole("region", { name: "More kinds" })
          .getByRole("button", { name: /非常/ }),
      ).toHaveCount(1);
    }
    await page.getByRole("button", { name: "Show table", exact: true }).click();
    await expect(
      page
        .getByRole("table", { name: "The same kinds as the landscape" })
        .locator("tbody tr"),
    ).toHaveCount(count);
    await expect(
      page
        .getByRole("table", { name: "The same connections as the diagram" })
        .locator("tbody tr"),
    ).toHaveCount(300);
  });

for (const [width, theme] of [
  [1440, "light"],
  [1440, "dark"],
  [390, "dark"],
] as const)
  test(`idle landscape capture ${width} ${theme}`, async ({ page }) => {
    await mockOntology(page);
    await page.setViewportSize({ width, height: 1000 });
    await page.addInitScript(
      (theme) => localStorage.setItem("eunomia-theme", theme),
      theme,
    );
    await page.goto("/ontology?space=business");
    await expect(
      page.getByRole("heading", { name: "House blend 1kg: Unit cost" }),
    ).toBeVisible();
    await expect(page.locator("[data-kind]")).toHaveCount(16);
    if (process.env.ONTOLOGY_SCREENSHOTS) {
      await page.addStyleTag({
        content: "nextjs-portal{display:none!important}",
      });
      await page.screenshot({
        path: `${process.env.ONTOLOGY_SCREENSHOTS}/overview-${width}-${theme}.png`,
        fullPage: true,
        animations: "disabled",
      });
    }
  });

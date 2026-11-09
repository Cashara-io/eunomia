import { test, expect, type Page } from "@playwright/test";
import { registerAndOnboard, uniqueEmail } from "./helpers";

async function choose(page: Page, label: string, option: string) {
  await page.getByRole("combobox", { name: label, exact: true }).click();
  await page.getByRole("option", { name: option, exact: true }).click();
  await expect(
    page.getByRole("combobox", { name: label, exact: true }),
  ).toContainText(option);
}

async function seed(page: Page) {
  await registerAndOnboard(page, uniqueEmail("twin"));
  const created = await page.request.post("/api/vaults", {
    data: { name: `Twin fixture ${Date.now()}` },
  });
  expect(created.ok()).toBeTruthy();
  const vault = await created.json();
  const { token } = await (
    await page.request.post("/api/auth/tokens", {
      data: { name: "Twin test fixture" },
    })
  ).json();
  const proposed = await page.request.post("/mcp", {
    headers: {
      Authorization: `Bearer ${token}`,
      Accept: "application/json, text/event-stream",
    },
    data: {
      jsonrpc: "2.0",
      id: 1,
      method: "tools/call",
      params: {
        name: "ontology_propose",
        arguments: {
          vault_id: vault.id,
          kinds: [
            {
              kind: "store",
              description: "A shop we operate",
              synonyms: ["shop"],
              reason: "From store records",
            },
            {
              kind: "shop",
              description: "An older name for store",
              merge_into: "store",
            },
            ...Array.from({ length: 14 }, (_, i) => ({
              kind: `fixture_${i}`,
              description: `Fixture kind ${i}`,
            })),
          ],
          properties: [
            {
              kind: "store",
              name: "revenue",
              datatype: "money",
              unit: "AUD",
              source_map: [
                {
                  source: "document",
                  record_type: "stores.csv",
                  path: "revenue",
                },
              ],
            },
          ],
          metrics: [
            {
              kind: "store",
              name: "total_revenue",
              expr: { agg: "sum", property: "revenue" },
              description: "Revenue across stores",
            },
          ],
          links: [
            {
              from_kind: "store",
              label: "supplies",
              to_kind: "store",
              reason: "Matched store codes",
            },
          ],
        },
      },
    },
  });
  expect(proposed.ok()).toBeTruthy();
  expect((await proposed.json()).result.isError).toBeFalsy();
  return { vault, token };
}

test("dedicated area, URL filters, kind details, Add data and old links", async ({
  page,
}) => {
  test.setTimeout(90_000);
  const { vault } = await seed(page);
  await page.goto("/twin");
  await expect(
    page.getByRole("heading", { name: "Choose a memory space" }),
  ).toBeVisible();
  await page.getByRole("link", { name: new RegExp(vault.name) }).click();
  await expect(
    page.getByRole("heading", { name: `${vault.name}: Overview` }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Add data", exact: true }).click();
  await expect(page.getByTestId("automap")).toBeVisible();
  await page
    .getByRole("link", { name: "Open Documents to upload or import" })
    .click();
  await expect(
    page.getByRole("combobox", { name: "Space", exact: true }),
  ).toContainText(vault.name);
  await page.goto(`/twin/schema?space=${encodeURIComponent(vault.id)}`);
  await expect(
    page
      .getByRole("table", { name: "Kinds and link types" })
      .locator("tbody tr"),
  ).toHaveCount(10);
  await page.getByRole("button", { name: "Next", exact: true }).click();
  await expect(page).toHaveURL(/page=2/);
  await page.goBack();
  await expect(page).not.toHaveURL(/page=2/);
  await page.getByRole("searchbox").fill("store");
  await choose(page, "Type", "Kinds");
  await expect(page.getByRole("table").locator("tbody tr")).toHaveCount(2);
  await choose(page, "Status", "Approved");
  await expect(page.getByRole("table")).toContainText("No definitions match");
  await choose(page, "Status", "Suggested");
  await page.getByRole("button", { name: "Refresh", exact: true }).click();
  await expect(page.getByRole("table").locator("tbody tr")).toHaveCount(2);
  await page.getByRole("link", { name: "store", exact: true }).click();
  await expect(page).toHaveURL(/\/twin\/kinds\/store/);
  await expect(page.getByText("shop", { exact: true })).toBeVisible();
  await choose(page, "Kind section", "Properties");
  await page.getByText("revenue (money, AUD)", { exact: true }).click();
  await expect(page.getByText("document: stores.csv, revenue")).toBeVisible();
  await choose(page, "Kind section", "Numbers");
  await page
    .locator("summary")
    .filter({ hasText: /^total revenue$/ })
    .click();
  await page.getByRole("button", { name: "Try it", exact: true }).click();
  await expect(page.getByRole("status")).toBeVisible();
  await choose(page, "Kind section", "Incoming and outgoing links");
  await expect(
    page.locator("summary").filter({ hasText: /store supplies store/ }),
  ).toBeVisible();
  await choose(page, "Kind section", "Review record");
  await expect(page.getByText(/does not provide who decided/)).toBeVisible();
  await page.goto("/vaults#kinds-and-links");
  await expect(page).toHaveURL(/\/twin\/schema\?space=/);
});

test("owner batch decisions, receipts, explicit merge and strict policy", async ({
  page,
}) => {
  test.setTimeout(90_000);
  const { vault, token } = await seed(page);
  const path = `/api/vaults/${vault.id}/ontology`;
  await page.goto(
    `/twin/review?space=${encodeURIComponent(vault.id)}&type=kind&q=store`,
  );
  await page
    .getByRole("checkbox", { name: "Select store", exact: true })
    .check();
  await expect(
    page.getByRole("button", { name: "Approve 1 selected" }),
  ).toBeDisabled();
  await page
    .getByRole("textbox", { name: "Reason for this batch (required)" })
    .fill("Matches fixture source records");
  await page.getByRole("button", { name: "Approve 1 selected" }).click();
  await expect(page.getByRole("dialog")).toContainText("store");
  await page
    .getByRole("button", { name: "Approve selected", exact: true })
    .click();
  await expect(page.getByRole("status")).toContainText("Kind store: Approved");
  const download = page.waitForEvent("download");
  await page.getByRole("button", { name: "Download receipt" }).click();
  expect((await download).suggestedFilename()).toBe(
    "business-twin-review-receipt.json",
  );
  await page.goto(
    `/twin/review?space=${encodeURIComponent(vault.id)}&type=kind&q=fixture_0`,
  );
  await page
    .getByRole("checkbox", { name: "Select fixture 0", exact: true })
    .check();
  await page
    .getByRole("textbox", { name: "Reason for this batch (required)" })
    .fill("Not part of this business");
  await page.getByRole("button", { name: "Reject 1 selected" }).click();
  await page
    .getByRole("button", { name: "Reject selected", exact: true })
    .click();
  await expect(page.getByRole("status")).toContainText(
    "Rejected without merging",
  );
  await page.goto(`/twin/review?space=${encodeURIComponent(vault.id)}&q=shop`);
  await page.getByRole("link", { name: /shop.*kind/ }).click();
  await page
    .getByRole("button", { name: "Reject and merge into store", exact: true })
    .click();
  await expect(
    page.getByRole("region", { name: "Selected suggestion" }),
  ).toContainText("New memories go to store.");
  await page.goto(`/twin/schema?space=${encodeURIComponent(vault.id)}`);
  await page
    .getByRole("switch", { name: /Only approved kinds and links/ })
    .click();
  await expect
    .poll(async () => (await (await page.request.get(path)).json()).strict)
    .toBe(true);
  const refused = await page.request.post(`${path}/kinds/decide`, {
    headers: { Authorization: `Bearer ${token}` },
    data: { kind: "fixture_1", decision: { action: "approve" } },
  });
  expect(refused.status()).toBe(403);
});

test("mobile navigation, keyboard filters and honest load failure", async ({
  page,
}) => {
  test.setTimeout(90_000);
  const { vault } = await seed(page);
  for (const width of [390, 320]) {
    await page.setViewportSize({ width, height: 844 });
    await page.goto(`/twin/schema?space=${encodeURIComponent(vault.id)}`);
    await choose(page, "View", "Review");
    await expect(page).toHaveURL(/\/twin\/review/);
    await page.getByRole("searchbox").fill("store");
    await page.getByRole("combobox", { name: "Type", exact: true }).focus();
    await page.keyboard.press("Enter");
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("Enter");
    await expect(
      page.getByRole("combobox", { name: "Type", exact: true }),
    ).toContainText("Kinds");
    await page
      .getByRole("link", { name: /store.*kind/ })
      .first()
      .click();
    await expect(
      page.getByRole("region", { name: "Selected suggestion" }),
    ).toBeVisible();
    await expect(
      page.getByRole("region", { name: "Review queue" }),
    ).toBeHidden();
    await page.getByRole("button", { name: "Back to queue" }).click();
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    ).toBe(true);
  }
  await page.route(/\/api\/vaults\/[^/]+\/ontology$/, (route) =>
    route.fulfill({ status: 503, json: { detail: "Fixture outage" } }),
  );
  await page.reload();
  await expect(
    page
      .getByRole("alert")
      .filter({ hasText: /Fixture outage|Could not load/ }),
  ).toBeVisible({ timeout: 15_000 });
});

test("members inspect evidence but cannot decide, and no-model replies stay honest", async ({
  page,
  browser,
}) => {
  test.setTimeout(90_000);
  const { vault } = await seed(page);
  const context = await browser.newContext();
  const member = await context.newPage();
  const email = uniqueEmail("twin-member");
  await registerAndOnboard(member, email);
  expect(
    (
      await page.request.post(`/api/vaults/${vault.id}/members`, {
        data: { email },
      })
    ).ok(),
  ).toBeTruthy();
  expect(
    (
      await member.request.post(`/api/vaults/${vault.id}/invitations/accept`)
    ).ok(),
  ).toBeTruthy();
  await member.goto(
    `/twin/review?space=${encodeURIComponent(vault.id)}&q=store&type=kind`,
  );
  await expect(
    member.getByText(/Only an owner of this space can decide/),
  ).toBeVisible();
  await member
    .getByRole("link", { name: /store.*kind/ })
    .first()
    .click();
  await expect(
    member.getByRole("button", { name: "Approve", exact: true }),
  ).toHaveCount(0);
  await expect(member.getByRole("checkbox")).toHaveCount(0);
  const refused = await member.request.post(
    `/api/vaults/${vault.id}/ontology/kinds/decide`,
    { data: { kind: "store", decision: { action: "approve" } } },
  );
  expect(refused.status()).toBe(403);
  await member.goto(`/twin/schema?space=${encodeURIComponent(vault.id)}`);
  await expect(member.getByRole("switch")).toBeDisabled();
  await context.close();
  const profile = await (
    await page.request.get(`/api/vaults/${vault.id}/ontology`)
  ).json();
  await page.route(/\/api\/vaults\/[^/]+\/ontology\/suggest$/, (route) =>
    route.fulfill({ json: { mode: "profile_only", profile, note: null } }),
  );
  await page.goto(`/twin/schema?space=${encodeURIComponent(vault.id)}`);
  await page.getByRole("button", { name: "Suggest kinds" }).click();
  await expect(page.getByRole("status")).toContainText(
    "No model is set up on this server",
  );
});

test("a batch approves dependencies in order and reports each saved result", async ({
  page,
}) => {
  test.setTimeout(60_000);
  const { vault } = await seed(page);
  await page.goto(
    `/twin/review?space=${encodeURIComponent(vault.id)}&kind=store`,
  );
  await page
    .getByRole("checkbox", { name: "Select suggestions on this page" })
    .check();
  await page
    .getByRole("textbox", { name: "Reason for this batch (required)" })
    .fill("Matches the source columns and store codes");
  await page.getByRole("button", { name: "Approve 4 selected" }).click();
  await page
    .getByRole("button", { name: "Approve selected", exact: true })
    .click();
  await expect(page.getByRole("status")).toContainText("Kind store: Approved");
  await expect(page.getByRole("status")).toContainText("Property revenue on store");
  await expect(page.getByRole("status")).toContainText(
    "Metric total revenue on store",
  );
  await expect(page.getByRole("status")).toContainText(
    "Link type store supplies store: Approved",
  );
  const current = await (
    await page.request.get(`/api/vaults/${vault.id}/ontology`)
  ).json();
  const store = current.kinds.find((k: { kind: string }) => k.kind === "store");
  expect([
    store.status,
    store.properties[0].status,
    store.metrics[0].status,
    current.links[0].status,
  ]).toEqual(["approved", "approved", "approved", "approved"]);
  await page.goto(`/twin/kinds/person?space=${encodeURIComponent(vault.id)}`);
  await expect(
    page.getByRole("button", { name: "Reject", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("combobox", { name: /Merge person/ }),
  ).toHaveCount(0);
});

test("one suggestion approves on its own, and closing it keeps the queue page", async ({
  page,
}) => {
  test.setTimeout(60_000);
  const { vault } = await seed(page);
  const space = encodeURIComponent(vault.id);
  await page.goto(`/twin/review?space=${space}&page=2`);
  await page.getByRole("link").filter({ hasText: /fixture/ }).first().click();
  await expect(page).toHaveURL(/selected=/);
  await page.getByRole("button", { name: "Back to queue" }).click();
  await expect(page).toHaveURL(/page=2/);
  await expect(page).not.toHaveURL(/selected=/);

  await page.goto(`/twin/review?space=${space}&type=kind&q=fixture_1`);
  await page.getByRole("link", { name: "fixture 1", exact: true }).click();
  await page.getByRole("button", { name: "Approve", exact: true }).click();
  await expect
    .poll(async () => {
      const current = await (
        await page.request.get(`/api/vaults/${vault.id}/ontology`)
      ).json();
      return current.kinds.find((k: { kind: string }) => k.kind === "fixture_1")
        ?.status;
    })
    .toBe("approved");
});

test("a kind page opened from Structure returns to the same Structure page", async ({
  page,
}) => {
  const { vault } = await seed(page);
  const space = encodeURIComponent(vault.id);
  await page.goto(`/twin/schema?space=${space}&page=2&status=all`);
  await page.getByRole("table").getByRole("link").first().click();
  await expect(page).toHaveURL(/\/twin\/kinds\//);
  await page.getByRole("link", { name: "Back to Structure" }).click();
  await expect(page).toHaveURL(/\/twin\/schema\?.*page=2/);
});

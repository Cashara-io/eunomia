import http from "node:http";
import type { AddressInfo } from "node:net";
import { test, expect } from "@playwright/test";
import { registerAndOnboard, uniqueEmail, MOCK_BIND, mockBase } from "./helpers";

// The whole connector flow against a stand-in for GitHub's REST API (the
// backend reaches it through the connector's `config.base_url`, which the
// backend only honours with EUNOMIA_ALLOW_CONNECTOR_BASE_URL=1 -- CI sets it): enter a
// token in the UI -> Test connection -> Sync now -> the issue is findable
// with the `search` tool -> a revoked token shows the provider's error.
function mockGitHub(): Promise<{ base: string; close: () => void }> {
  const issue = {
    number: 42,
    title: "Quarterly importer drops the last CSV row",
    state: "open",
    body: "Seen with the zanzibar export.",
    user: { login: "octocat" },
    labels: [{ name: "bug" }],
    assignees: [],
    comments: 0,
    repository: { full_name: "acme/widget" },
    html_url: "https://github.com/acme/widget/issues/42",
    created_at: "2024-01-01T00:00:00Z",
    updated_at: new Date().toISOString(),
  };
  const server = http.createServer((req, res) => {
    const ok = req.headers.authorization === "Bearer ghp_e2e_valid";
    res.setHeader("content-type", "application/json");
    if (!ok) {
      res.statusCode = 401;
      res.end(JSON.stringify({ message: "Bad credentials" }));
      return;
    }
    const path = (req.url ?? "").split("?")[0];
    if (path === "/user") return res.end(JSON.stringify({ login: "octocat" }));
    if (path === "/issues") return res.end(JSON.stringify([issue]));
    res.statusCode = 404;
    res.end("{}");
  });
  return new Promise((resolve) =>
    server.listen(0, MOCK_BIND, () => {
      const { port } = server.address() as AddressInfo;
      resolve({ base: mockBase(port), close: () => server.close() });
    })
  );
}

test("a connector syncs real API data that the search tool then finds", async ({ page }) => {
  const github = await mockGitHub();
  try {
    await registerAndOnboard(page, uniqueEmail("connector-sync"));
    await page.goto("/connectors");
    await page.getByRole("link", { name: /GitHub/ }).click();
    await expect(page.getByRole("heading", { name: "GitHub" })).toBeVisible();
    await expect(page.getByRole("link", { name: "fine-grained token" })).toBeVisible();

    await page.getByPlaceholder("github_pat_… or ghp_…").fill("ghp_e2e_valid");
    await page.getByRole("button", { name: "Save", exact: true }).click();
    await expect(page.getByRole("button", { name: "Saved" })).toBeVisible();
    // Point the connector at the mock instead of api.github.com (test-only hook).
    expect((await page.request.put("/api/connectors/github", { data: { config: { base_url: github.base } } })).ok()).toBeTruthy();
    // that change bypassed the page: reload so it shows the saved credentials (the button is disabled
    // with "Save credentials first" until it does, which raced the click above under CI load)
    await page.reload();
    await expect(page.getByRole("button", { name: "Test connection" })).toBeEnabled();

    await page.getByRole("button", { name: "Test connection" }).click();
    await expect(page.getByText("Connection works")).toBeVisible();

    await page.getByRole("button", { name: "Sync now" }).click();
    // The scheduler may sync a newly connected source before this click does: then the click finds the
    // issue already stored ("1 unchanged"). Either way it must be stored exactly once (checked below).
    await expect(page.getByRole("status")).toHaveText(/Synced: (1 new or changed|0 new or changed, 1 unchanged)/);

    const found = await (await page.request.post("/api/tools/search", { data: { query: "zanzibar importer" } })).json();
    expect(found.results.map((r: { id: string }) => r.id)).toContain("github:github.issue:acme/widget#42");

    await page.getByRole("link", { name: "View synced data" }).click();
    await expect(page.getByText("Quarterly importer drops the last CSV row")).toBeVisible();

    // A revoked token: the provider's 401 is shown, not a silent empty sync.
    await page.goto("/connectors/setup/github");
    await page.getByPlaceholder("github_pat_… or ghp_…").fill("ghp_revoked");
    await page.getByRole("button", { name: "Save", exact: true }).click();
    await page.getByRole("button", { name: "Sync now" }).click();
    await expect(page.getByRole("status")).toHaveText(/Sync failed: .*HTTP 401.*Bad credentials/);
    await page.getByRole("button", { name: "Test connection" }).click();
    await expect(page.getByText(/HTTP 401/).first()).toBeVisible();
  } finally {
    github.close();
  }
});

test("every connector offered opens a setup page with instructions", async ({ page }) => {
  await registerAndOnboard(page, uniqueEmail("connector-pages"));
  await page.goto("/connectors");
  const cards = page.locator('a[href^="/connectors/setup/"]');
  await expect(cards).toHaveCount(12);
  const hrefs = await cards.evaluateAll((els) => els.map((e) => e.getAttribute("href")));
  expect(hrefs).toHaveLength(12);
  expect(hrefs).not.toContain("/connectors/setup/open_connector");
  for (const href of hrefs) {
    await page.goto(href!);
    await expect(page.getByText("Unknown connector")).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Test connection" })).toBeVisible();
  }
});

import fs from "node:fs";
import { expect, test, type Page } from "@playwright/test";
import { registerAndOnboard, uniqueEmail } from "./helpers";

// Settings > Audit log > Integrity and Settings > Access reviews (compliance PR03, docs/audit-chain.md),
// against the real backend. Only the install's admin can use them: set E2E_ADMIN_EMAIL and
// E2E_ADMIN_PASSWORD to the first user's login (CI's scripts/ci/e2e-setup.sh does).
const admin = { email: process.env.E2E_ADMIN_EMAIL, password: process.env.E2E_ADMIN_PASSWORD };

async function signInAsAdmin(page: Page) {
  await page.goto("/login");
  await page.getByLabel("Email").fill(admin.email!);
  await page.getByLabel("Password", { exact: true }).fill(admin.password!);
  await page.getByRole("button", { name: "Sign in" }).click();
  await page.waitForURL((url) => url.pathname === "/");
}

async function keepEveryoneAndComplete(page: Page) {
  const groups = page.getByRole("group", { name: /^Decision for / });
  await expect(groups.first()).toBeVisible();
  const n = await groups.count();
  for (let i = 0; i < n; i++) {
    const keep = groups.nth(i).getByRole("button", { name: "Keep" });
    if ((await keep.getAttribute("aria-pressed")) !== "true") {
      await keep.click();
      await expect(keep).toHaveAttribute("aria-pressed", "true");
    }
  }
  const complete = page.getByRole("button", { name: "Complete review" });
  await expect(complete).toBeEnabled();
  await complete.click();
  await expect(complete).toBeHidden();
}

test.describe("security log integrity and access reviews", () => {
  test.skip(!admin.email || !admin.password, "set E2E_ADMIN_EMAIL and E2E_ADMIN_PASSWORD to the first user's login");

  test("an admin checks the chain and downloads a file without content", async ({ page }) => {
    await signInAsAdmin(page);
    await page.goto("/settings?tab=audit");
    await page.getByRole("button", { name: "Check integrity" }).click();
    const result = page.getByTestId("integrity-result");
    await expect(result).toContainText("This workspace: intact");
    await expect(result).not.toContainText("altered");

    const download = page.waitForEvent("download");
    await page.getByRole("button", { name: "Download for auditors" }).click();
    const file = await (await download).path();
    const doc = JSON.parse(fs.readFileSync(file, "utf-8"));
    expect(doc.format).toBe("eunomia-audit-export");
    expect(doc.ok).toBe(true);
    expect(doc.events.length).toBeGreaterThan(0);
    // the header states the file's own range, ending at the head it recorded
    expect(doc.range.first_seq).toBe(doc.events[0].seq);
    expect(doc.range.last_seq).toBe(doc.events[doc.events.length - 1].seq);
    expect(doc.range.head_seq).toBe(doc.head.seq);
    // metadata only: the free-text detail travels as its hash
    for (const e of doc.events) {
      expect(e.detail).toBeUndefined();
      expect(e.detail_digest).toMatch(/^[0-9a-f]{64}$/);
    }
  });

  test("an admin runs an access review to completion", async ({ page, browser }) => {
    // someone to review
    const member = uniqueEmail("review-member");
    const other = await browser.newPage();
    await registerAndOnboard(other, member);
    await other.close();

    await signInAsAdmin(page);
    await page.goto("/settings?tab=reviews");
    await expect(page.getByRole("heading", { name: "Access reviews" })).toBeVisible();

    const start = page.getByRole("button", { name: "Start a review" });
    const complete = page.getByRole("button", { name: "Complete review" });
    await expect(start.or(complete)).toBeVisible();
    // a review left open by an earlier run is finished first
    if (await complete.isVisible()) await keepEveryoneAndComplete(page);
    await start.click();
    await expect(page.getByTestId(`review-person-${member}`)).toContainText("Member");
    // your own row can't be decided by you
    await expect(page.getByTestId(`review-person-${admin.email}`)).toContainText("Another admin reviews your access");
    await keepEveryoneAndComplete(page);
    await expect(start).toBeVisible();

    const row = page.getByTestId("completed-review").first();
    await expect(row).toContainText("1 not reviewed");
    const download = page.waitForEvent("download");
    await row.getByRole("button", { name: "Download" }).click();
    const doc = JSON.parse(fs.readFileSync((await (await download).path())!, "utf-8"));
    expect(doc.result_hash_matches).toBe(true);
    expect(doc.snapshot.people.some((p: { email: string }) => p.email === member)).toBe(true);
    expect(JSON.stringify(doc)).not.toContain("token_hash");
  });
});

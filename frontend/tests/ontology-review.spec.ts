import { test, expect, type Page } from "@playwright/test";
import { mockReview, reviewUrl, selected } from "./fixtures/ontology-review";

const reasonLabel = "Your reason, saved with the decision (optional)";
const detail = (page: Page) =>
  page.getByRole("region", { name: "Selected suggestion", exact: true });
async function chooseMerge(page: Page, target: string) {
  await expect(detail(page)).toContainText("unit cost");
  const options = page.getByRole("group", { name: "Other definition options" });
  if (
    (await options.count()) &&
    !(await options.evaluate((e) => e.hasAttribute("open")))
  )
    await options.locator("summary").first().click();
  const picker = page.getByRole("combobox", {
    name: "Merge unit cost into",
    exact: true,
  });
  if (await picker.evaluate((e) => e.tagName === "SELECT"))
    await picker.selectOption({ label: target });
  else {
    await picker.click();
    await page.getByRole("option", { name: target, exact: true }).click();
  }
}

test("merge selection never saves, preview precedes one final mutation", async ({
  page,
}) => {
  const fixture = await mockReview(page);
  await page.goto(reviewUrl);
  await chooseMerge(page, "price");
  expect(fixture.mutations).toHaveLength(0);
  await expect(
    page.getByRole("region", { name: "Merge preview" }),
  ).toContainText("unit cost → price");
  await expect(
    page.getByRole("region", { name: "Merge preview" }),
  ).toContainText("6,000 recorded values");
  await expect(page.getByText("Conflicts to consider")).toBeVisible();
  await page.getByLabel(reasonLabel).fill("The catalogue uses selling price");
  await page
    .getByRole("button", { name: "Confirm merge", exact: true })
    .dblclick();
  await expect(
    page.getByRole("region", { name: "Saved decision" }),
  ).toContainText("Saved.");
  expect(fixture.mutations).toHaveLength(1);
  expect(fixture.mutations[0]).toMatchObject({
    revision: 1,
    reason: "The catalogue uses selling price",
    decision: { action: "reject", merge_into: "price" },
  });
});

test("evidence and consequence come before reason and decisions", async ({
  page,
}) => {
  await mockReview(page);
  await page.goto(reviewUrl);
  await expect(page.getByLabel(reasonLabel)).toBeVisible();
  expect(
    await detail(page).evaluate((e) => {
      const nodes = [
        e.querySelector('[aria-label="What changes"]'),
        e.querySelector('[aria-label="Source evidence"]'),
        e.querySelector('[aria-label="Consequence of approving"]'),
        e.querySelector("textarea"),
        e.querySelector("footer"),
      ];
      return nodes.every(
        (n, i) =>
          n &&
          (i === 0 ||
            Boolean(
              (nodes[i - 1]?.compareDocumentPosition(n) ?? 0) &
              Node.DOCUMENT_POSITION_FOLLOWING,
            )),
      );
    }),
  ).toBe(true);
  await expect(detail(page)).toContainText("Samples do not verify all values");
  await expect(detail(page)).toContainText("Proposed by Claude Code");
});

test("keyboard queue, approval, rejection, Escape and focus", async ({
  page,
}) => {
  const fixture = await mockReview(page);
  await page.goto("/ontology/review?space=business");
  await expect(page.locator("a[data-definition]").first()).toBeVisible();
  await page.keyboard.press("j");
  await expect(
    page.getByRole("heading", { name: "unit cost", exact: true }),
  ).toBeFocused();
  await page.getByLabel(reasonLabel).fill("A reason with a r j k");
  expect(fixture.mutations).toHaveLength(0);
  await page.getByRole("heading", { name: "unit cost", exact: true }).focus();
  await page.keyboard.press("j");
  await expect(
    page.getByRole("heading", { name: "gross profit", exact: true }),
  ).toBeFocused();
  await page.keyboard.press("k");
  await expect(
    page.getByRole("heading", { name: "unit cost", exact: true }),
  ).toBeFocused();
  await chooseMerge(page, "price");
  await page.keyboard.press("Escape");
  await expect(page.getByRole("button", { name: "Confirm merge" })).toHaveCount(
    0,
  );
  await expect(
    page.getByRole("combobox", { name: "Merge unit cost into", exact: true }),
  ).toBeFocused();
  expect(fixture.mutations).toHaveLength(0);
  await page.keyboard.press("Escape");
  await expect(page).not.toHaveURL(/selected=/);
  await expect(page.locator("a[data-definition]").first()).toBeFocused();
  await page.keyboard.press("j");
  await expect(
    page.getByRole("heading", { name: "unit cost", exact: true }),
  ).toBeFocused();
  await page.keyboard.press("a");
  await expect(page.getByRole("heading", { name: /^Saved\./ })).toBeFocused();
  await page.getByRole("button", { name: "Next suggestion" }).click();
  await expect(
    page.getByRole("heading", { name: "gross profit", exact: true }),
  ).toBeFocused();
  await page.keyboard.press("r");
  await expect(
    page.getByRole("region", { name: "Saved decision" }),
  ).toContainText("rejected");
  expect(fixture.mutations).toHaveLength(2);
});

test("network failure keeps reason and evidence, local Retry confirms Saved", async ({
  page,
}) => {
  const fixture = await mockReview(page, { mode: "network" });
  await page.goto(reviewUrl);
  await page.getByLabel(reasonLabel).fill("Matches the source definition");
  await page
    .getByRole("button", { name: "Approve unit cost", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "Retry", exact: true }),
  ).toBeVisible();
  await expect(page.getByLabel(reasonLabel)).toHaveValue(
    "Matches the source definition",
  );
  await expect(
    page.getByRole("region", { name: "Source evidence" }),
  ).toContainText("AUD 12.00");
  await expect(
    page.getByRole("region", { name: "Saved decision" }),
  ).toHaveCount(0);
  await page.getByRole("button", { name: "Retry", exact: true }).click();
  await expect(
    page.getByRole("region", { name: "Saved decision" }),
  ).toContainText("Saved.");
  expect(fixture.mutations).toHaveLength(2);
});

test("stale revision shows changed fields and keeps the draft until re-review", async ({
  page,
}) => {
  const fixture = await mockReview(page, { mode: "stale" });
  await page.goto(reviewUrl);
  await page.getByLabel(reasonLabel).fill("Check the cost basis");
  await page
    .getByRole("button", { name: "Approve unit cost", exact: true })
    .click();
  await expect(detail(page).getByRole("alert")).toContainText("Now: AUD/kg");
  await expect(
    page.getByRole("button", { name: "Approve unit cost", exact: true }),
  ).toBeDisabled();
  await expect(page.getByLabel(reasonLabel)).toHaveValue(
    "Check the cost basis",
  );
  await page.getByRole("button", { name: "Review latest version" }).click();
  await expect(detail(page)).toContainText(
    "Cost per kilogram, excluding delivery.",
  );
  await expect(page.getByLabel(reasonLabel)).toHaveValue(
    "Check the cost basis",
  );
  await page
    .getByRole("button", { name: "Approve unit cost", exact: true })
    .click();
  await expect(
    page.getByRole("region", { name: "Saved decision" }),
  ).toBeVisible();
  expect(fixture.mutations.map((m) => m.revision)).toEqual([1, 2]);
});

test("members see evidence with no decision controls or keyboard writes", async ({
  page,
}) => {
  const fixture = await mockReview(page, { member: true });
  await page.goto(reviewUrl);
  await expect(detail(page)).toContainText("Owner-only decision");
  await expect(
    page.getByRole("region", { name: "Source evidence" }),
  ).toContainText("AUD 12.00");
  await expect(
    detail(page).getByRole("button", { name: /Approve|Reject|Confirm merge/ }),
  ).toHaveCount(0);
  await expect(detail(page).getByRole("textbox")).toHaveCount(0);
  await page.keyboard.press("a");
  await page.keyboard.press("r");
  expect(fixture.mutations).toHaveLength(0);
});

test("saving retains evidence and disables competing decisions", async ({
  page,
}) => {
  await mockReview(page, { mode: "saving" });
  await page.goto(reviewUrl);
  await page.getByLabel(reasonLabel).fill("Reviewed source");
  await page
    .getByRole("button", { name: "Approve unit cost", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "Approve unit cost", exact: true }),
  ).toBeDisabled();
  await expect(
    page.getByRole("button", { name: "Reject", exact: true }),
  ).toBeDisabled();
  await expect(
    page.getByRole("button", { name: "Back to queue", exact: true }),
  ).toBeDisabled();
  await expect(page.getByLabel(reasonLabel)).toHaveValue("Reviewed source");
  await expect(
    page.getByRole("region", { name: "Source evidence" }),
  ).toContainText("AUD 12.00");
  await expect(
    page.getByRole("region", { name: "Saved decision" }),
  ).toHaveCount(0);
});

const figures = [
  ["loading", "review-loading"],
  ["error", "review-error"],
  ["queue", "review-populated"],
  ["filters-open", "review-populated"],
  ["detail", "review-detail"],
  ["merge-preview", "review-detail"],
  ["merge-conflict", "review-detail"],
  ["merge-cancelled", "review-detail"],
  ["merge-saved", "review-merge"],
  ["saving", "review-detail"],
  ["approved", "review-approved"],
  ["receipt-evidence", "review-approved"],
  ["rejected", "review-reject"],
  ["save-error", "review-save-error"],
  ["retry-saved", "review-approved"],
  ["stale-conflict", "review-stale-conflict"],
  ["latest-version", "review-stale-conflict"],
  ["member", "review-member"],
  ["empty", "review-empty"],
  ["filtered-empty", "review-populated"],
  ["edit-description", "review-detail"],
  ["metric", "review-detail"],
  ["link", "review-detail"],
  ["batch-confirm", "review-batch-confirm"],
] as const;
for (const width of [1440, 390])
  for (const theme of ["light", "dark"] as const)
    for (const [state, before] of figures) {
      test(`figure ${state} ${width} ${theme}`, async ({ page, browser }) => {
        test.skip(
          !process.env.ONTOLOGY_REVIEW_FIGURES,
          "Set ONTOLOGY_REVIEW_FIGURES to capture the paper figures.",
        );
        const { mkdir, writeFile } = await import("node:fs/promises");
        const { createHash } = await import("node:crypto");
        const folder = process.env.ONTOLOGY_REVIEW_FIGURES;
        if (!folder) return;
        await mkdir(folder, { recursive: true });
        await page.setViewportSize({
          width,
          height: width === 1440 ? 900 : 844,
        });
        await page.addInitScript(
          (theme) => localStorage.setItem("eunomia-theme", theme),
          theme,
        );
        await page.clock.setFixedTime(new Date("2026-11-10T03:32:00Z"));
        await mockReview(page, {
          member: state === "member",
          mode:
            state === "loading"
              ? "loading"
              : state === "error"
                ? "error"
                : state === "empty"
                  ? "empty"
                  : state === "saving"
                    ? "saving"
                    : ["save-error", "retry-saved"].includes(state)
                      ? "network"
                      : ["stale-conflict", "latest-version"].includes(state)
                        ? "stale"
                        : "success",
        });
        const queueState = [
          "queue",
          "filters-open",
          "loading",
          "error",
          "empty",
          "filtered-empty",
          "batch-confirm",
        ].includes(state);
        const id =
          state === "metric"
            ? JSON.stringify(["metric", "stock_item", "gross_profit"])
            : state === "link"
              ? JSON.stringify(["link", "stock_item", "stored_at", "warehouse"])
              : selected;
        await page.goto(
          queueState
            ? `/ontology/review?space=business${state === "filtered-empty" ? "&q=unmatched" : ""}`
            : `/ontology/review?space=business&selected=${encodeURIComponent(id)}`,
        );
        if (state === "loading")
          await expect(
            page.getByText("Loading the ontology", { exact: true }),
          ).toBeAttached();
        else if (state === "error")
          await expect(
            page
              .getByRole("alert")
              .filter({ hasText: "Could not load the ontology" }),
          ).toBeVisible({ timeout: 15000 });
        else
          await expect(
            page.getByRole("heading", {
              name: "Decide with the source in sight.",
            }),
          ).toBeVisible();
        if (!queueState) await expect(detail(page)).toBeVisible();
        if (state === "filters-open" && width === 390)
          await page
            .getByText("Search and filter suggestions", { exact: true })
            .click();
        if (
          [
            "merge-preview",
            "merge-conflict",
            "merge-cancelled",
            "merge-saved",
          ].includes(state)
        ) {
          await chooseMerge(
            page,
            state === "merge-conflict" ? "quantity" : "price",
          );
          if (state === "merge-cancelled") await page.keyboard.press("Escape");
          if (state === "merge-saved") {
            await page
              .getByLabel(reasonLabel)
              .fill("Uses the catalogue price definition");
            await page
              .getByRole("button", { name: "Confirm merge", exact: true })
              .click();
          }
        }
        if (
          [
            "saving",
            "approved",
            "receipt-evidence",
            "save-error",
            "retry-saved",
            "stale-conflict",
            "latest-version",
          ].includes(state)
        ) {
          await page
            .getByLabel(reasonLabel)
            .fill("Matches the source column and our cost definition.");
          await page
            .getByRole("button", { name: "Approve unit cost", exact: true })
            .click();
        }
        if (["save-error", "retry-saved"].includes(state))
          await expect(
            page.getByRole("button", { name: "Retry", exact: true }),
          ).toBeVisible();
        if (state === "retry-saved")
          await page
            .getByRole("button", { name: "Retry", exact: true })
            .click();
        if (["stale-conflict", "latest-version"].includes(state))
          await expect(
            page.getByRole("button", { name: "Review latest version" }),
          ).toBeVisible();
        if (state === "latest-version")
          await page
            .getByRole("button", { name: "Review latest version" })
            .click();
        if (state === "rejected")
          await page
            .getByRole("button", { name: "Reject", exact: true })
            .click();
        if (
          [
            "approved",
            "receipt-evidence",
            "retry-saved",
            "rejected",
            "merge-saved",
          ].includes(state)
        )
          await expect(
            page.getByRole("region", { name: "Saved decision" }),
          ).toBeVisible();
        if (state === "receipt-evidence")
          await page
            .getByText("Review the evidence again", { exact: true })
            .click();
        if (state === "edit-description") {
          await page
            .getByText("Merge or edit definition", { exact: true })
            .click();
          await page
            .getByRole("button", { name: "Edit description", exact: true })
            .click();
        }
        if (state === "batch-confirm") {
          await page.getByText("Review a batch", { exact: true }).click();
          await page.getByLabel("Select suggestions on this page").check();
          await page
            .getByRole("button", { name: /^Approve \d+ selected$/ })
            .click();
        }
        await page.evaluate(async () => {
          await document.fonts.ready;
          window.scrollTo(0, 0);
        });
        await page.addStyleTag({
          content: "nextjs-portal{display:none!important}",
        });
        expect(
          await page.evaluate(
            () => document.documentElement.scrollWidth <= innerWidth,
          ),
        ).toBe(true);
        const file = `${state}-${width}-${theme}.png`;
        const bytes = await page.screenshot({
          path: `${folder}/${file}`,
          fullPage: true,
          animations: "disabled",
        });
        await writeFile(
          `${folder}/${state}-${width}-${theme}.json`,
          JSON.stringify(
            {
              file,
              before: `../../before/${before}-${width}-${theme}.png`,
              state,
              width,
              height: width === 1440 ? 900 : 844,
              theme,
              dpr: 1,
              browser: browser.version(),
              fixture: "tests/fixtures/ontology-review.ts",
              fictional: true,
              fixtureTime: "2026-11-10T03:32:00Z",
              screenshotType: "full page",
              sha256: createHash("sha256").update(bytes).digest("hex"),
              capturedAt: new Date().toISOString(),
            },
            null,
            2,
          ),
        );
      });
    }

test("a live refresh reveals changed fields without replacing a drafted reason", async ({
  page,
}) => {
  const fixture = await mockReview(page);
  await page.goto(reviewUrl);
  await page.getByLabel(reasonLabel).fill("Compare with the supplier invoice");
  const property = fixture.model.kinds[0].properties[0];
  property.description = "Cost including freight";
  property.revision = 2;
  await page.getByRole("button", { name: "Refresh", exact: true }).click();
  await expect(detail(page).getByRole("alert")).toContainText(
    "Now: Cost including freight",
  );
  await expect(page.getByLabel(reasonLabel)).toHaveValue(
    "Compare with the supplier invoice",
  );
  await expect(
    page.getByRole("button", { name: "Approve unit cost", exact: true }),
  ).toBeDisabled();
  expect(fixture.mutations).toHaveLength(0);
});

test("reject never silently applies a suggested merge", async ({ page }) => {
  const fixture = await mockReview(page);
  fixture.model.kinds[0].properties[0].merge_into = "price";
  await page.goto(reviewUrl);
  await page.getByRole("button", { name: "Reject", exact: true }).click();
  await expect(
    page.getByRole("region", { name: "Saved decision" }),
  ).toContainText("Reject without merging");
  expect(fixture.mutations[0]).toMatchObject({
    decision: { action: "reject" },
  });
  expect(fixture.mutations[0].decision).not.toHaveProperty("merge_into");
});

test("description editing keeps revision preconditions and Escape cancels without writing", async ({
  page,
}) => {
  const fixture = await mockReview(page);
  await page.goto(reviewUrl);
  await page.getByText("Merge or edit definition", { exact: true }).click();
  await page
    .getByRole("button", { name: "Edit description", exact: true })
    .click();
  await page
    .getByLabel("Description", { exact: true })
    .fill("Cost including freight");
  await page.keyboard.press("Escape");
  expect(fixture.mutations).toHaveLength(0);
  await page
    .getByRole("button", { name: "Edit description", exact: true })
    .click();
  await page
    .getByLabel("Description", { exact: true })
    .fill("Cost including freight");
  await page
    .getByRole("button", { name: "Save description", exact: true })
    .click();
  await expect(
    page.getByRole("region", { name: "Saved decision" }),
  ).toContainText("description updated");
  expect(fixture.mutations[0]).toMatchObject({
    revision: 1,
    decision: { action: "describe", description: "Cost including freight" },
  });
});

for (const width of [320, 390, 1440])
  test(`reflow and reduced motion at ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ reducedMotion: "reduce" });
    await mockReview(page);
    await page.goto(reviewUrl);
    await chooseMerge(page, "quantity");
    await expect(
      page.getByRole("region", { name: "Merge preview" }),
    ).toContainText("Data types differ: money and integer.");
    await page.getByLabel(reasonLabel).fill("A long explanation ".repeat(20));
    await page
      .getByRole("button", { name: "Confirm merge", exact: true })
      .scrollIntoViewIfNeeded();
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    ).toBe(true);
    const animation = await page
      .locator('[aria-label="Merge preview"] [class*="reveal"]')
      .evaluate((e) => getComputedStyle(e).animationName);
    expect(animation).toContain("fade");
    await page.keyboard.press("Escape");
    expect(
      await page
        .getByRole("combobox", { name: "Merge unit cost into", exact: true })
        .inputValue(),
    ).toBe("");
  });

test("queue page, filters and focus survive closing a selected proposal", async ({
  page,
}) => {
  const fixture = await mockReview(page);
  for (let i = 0; i < 12; i++)
    fixture.model.kinds.push({ ...fixture.model.kinds[1], kind: `extra_${i}` });
  await page.goto("/ontology/review?space=business&type=kind&page=2");
  const first = page.locator("a[data-definition]").first();
  const id = await first.getAttribute("data-definition");
  await first.click();
  await page
    .getByRole("button", { name: "Back to queue", exact: true })
    .click();
  await expect(page).toHaveURL(/type=kind/);
  await expect(page).toHaveURL(/page=2/);
  await expect(first).toBeFocused();
  expect(
    await page.evaluate(() =>
      document.activeElement?.getAttribute("data-definition"),
    ),
  ).toBe(id);
});

test("200% text, forced colours and queue touch targets", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await mockReview(page);
  await page.goto("/ontology/review?space=business");
  const checkbox = page.getByRole("checkbox", {
    name: "Select unit cost",
    exact: true,
  });
  const target = await checkbox.locator("..").boundingBox();
  expect(target?.width).toBeGreaterThanOrEqual(44);
  expect(target?.height).toBeGreaterThanOrEqual(44);
  await page.keyboard.press("j");
  await expect(
    page.getByRole("heading", { name: "unit cost", exact: true }),
  ).toBeFocused();
  await page.emulateMedia({ forcedColors: "active" });
  await page.evaluate(() => {
    const sizes = [
      ...document.querySelectorAll<HTMLElement>(
        '[aria-label="Selected suggestion"] *',
      ),
    ].map((element) => ({
      element,
      size: parseFloat(getComputedStyle(element).fontSize),
      line: parseFloat(getComputedStyle(element).lineHeight),
    }));
    for (const { element, size, line } of sizes) {
      element.style.fontSize = `${size * 2}px`;
      if (Number.isFinite(line)) element.style.lineHeight = `${line * 2}px`;
    }
  });
  await page
    .getByRole("button", { name: "Approve unit cost", exact: true })
    .scrollIntoViewIfNeeded();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
  await expect(
    page.getByRole("button", { name: "Approve unit cost", exact: true }),
  ).toBeVisible();
});

test("phone filters fold away and the queue keeps its search after inspection", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await mockReview(page);
  await page.goto("/ontology/review?space=business");
  await expect(page.locator("a[data-definition]").first()).toBeVisible();
  await expect(page.getByRole("searchbox")).toHaveCount(0);
  const first = await page.locator("a[data-definition]").first().boundingBox();
  expect(first?.y).toBeLessThan(844);
  await page
    .getByText("Search and filter suggestions", { exact: true })
    .click();
  await page.getByRole("searchbox").fill("unit cost");
  await page.locator("a[data-definition]").first().click();
  await page
    .getByRole("button", { name: "Back to queue", exact: true })
    .click();
  await expect(page.getByRole("searchbox")).toHaveValue("unit cost");
  await expect(page).toHaveURL(/q=unit\+cost/);
});

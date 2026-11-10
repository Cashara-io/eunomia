import { test, expect, type Page } from "@playwright/test";
import { mockReview, reviewUrl } from "./fixtures/ontology-review";

const region = (page: Page) => page.getByRole("region", { name: "Selected suggestion", exact: true });
const heading = (page: Page) => region(page).getByRole("heading", { name: /unit cost/i, exact: true });
const approve = (page: Page) => page.getByRole("button", { name: /Approve unit cost/i, exact: true });
async function merge(page: Page) {
  await page.getByText("Merge or edit definition", { exact: true }).click();
  await page.getByRole("combobox", { name: /Merge unit cost into/i }).selectOption({ value: JSON.stringify(["property", "stock_item", "price"]) });
}

test("links never offer a merge even when another link exists", async ({ page }) => {
  const fixture = await mockReview(page);
  fixture.model.links.push({ ...fixture.model.links[0], label: "delivered_to", status: "approved" });
  const selected = JSON.stringify(["link", "stock_item", "stored_at", "warehouse"]);
  await page.goto(`/ontology/review?space=business&selected=${encodeURIComponent(selected)}`);
  await page.getByRole("group", { name: "Other definition options" }).locator("summary").click();
  await expect(region(page)).toContainText("Merging is not available for links.");
  await expect(region(page).getByRole("combobox", { name: /Merge/ })).toHaveCount(0);
  expect(fixture.mutations).toHaveLength(0);
});

test("merge receipt reports the returned rejection when the server did not redirect", async ({ page }) => {
  const fixture = await mockReview(page, { omitRedirect: true });
  await page.goto(reviewUrl);
  await merge(page);
  await page.getByRole("button", { name: "Confirm merge", exact: true }).click();
  const receipt = page.getByRole("region", { name: "Saved decision" });
  await expect(receipt).toContainText("rejected");
  await expect(receipt).toContainText("Reject without merging");
  await expect(receipt).not.toContainText("merged into");
  expect(fixture.mutations).toHaveLength(1);
});

for (const target of ["reason", "contenteditable", "plaintext-only", "textbox", "combobox", "searchbox", "outside", "modal", "dialog", "popover", "menu", "select"] as const) {
  test(`real A and R keys do not act in ${target}`, async ({ page }) => {
    const fixture = await mockReview(page);
    await page.goto(reviewUrl);
    await expect(heading(page)).toBeFocused();
    if (target === "reason") await page.getByLabel("Your reason, saved with the decision (optional)").focus();
    else if (target === "outside") {
      await page.evaluate(() => { const b = document.createElement("button"); b.textContent = "Outside review"; document.body.append(b); b.focus(); });
    } else {
      await region(page).evaluate((parent, target) => {
        if (["modal", "dialog", "popover", "menu", "select"].includes(target)) {
          const overlay = document.createElement(target === "dialog" ? "dialog" : "div");
          if (target === "modal") overlay.setAttribute("aria-modal", "true");
          if (target === "popover") { overlay.setAttribute("popover", "auto"); }
          if (target === "menu" || target === "select") overlay.setAttribute("role", target === "menu" ? "menu" : "listbox");
          document.body.append(overlay);
          if (overlay instanceof HTMLDialogElement) overlay.show();
          if (target === "popover") overlay.showPopover();
          // Deliberately keep background focus to catch document-level writes.
          return;
        }
        const editable = document.createElement("div");
        editable.tabIndex = 0;
        if (["contenteditable", "plaintext-only"].includes(target)) editable.setAttribute("contenteditable", target === "plaintext-only" ? "plaintext-only" : "");
        else editable.setAttribute("role", target);
        parent.append(editable);
        editable.focus();
      }, target);
    }
    await page.keyboard.type("aArR");
    await expect(approve(page)).not.toBeFocused();
    await expect(page.getByRole("button", { name: "Reject", exact: true })).not.toBeFocused();
    expect(fixture.mutations).toHaveLength(0);
  });
}

for (const modifier of ["Shift", "Control", "Alt", "Meta"]) {
  test(`${modifier} shortcuts do not focus or write`, async ({ page }) => {
    const fixture = await mockReview(page);
    await page.goto(reviewUrl);
    await expect(heading(page)).toBeFocused();
    await page.keyboard.press(`${modifier}+a`);
    await page.keyboard.press(`${modifier}+r`);
    await expect(heading(page)).toBeFocused();
    expect(fixture.mutations).toHaveLength(0);
  });
}

test("A and R only focus a decision, Enter writes once for the visible revision", async ({ page }) => {
  const fixture = await mockReview(page);
  await page.goto(reviewUrl);
  await expect(heading(page)).toBeFocused();
  await page.keyboard.press("a");
  await expect(approve(page)).toBeFocused();
  expect(fixture.mutations).toHaveLength(0);
  await page.keyboard.press("r");
  await expect(page.getByRole("button", { name: "Reject", exact: true })).toBeFocused();
  expect(fixture.mutations).toHaveLength(0);
  await page.keyboard.press("Enter");
  await expect(page.getByRole("region", { name: "Saved decision" })).toBeVisible();
  expect(fixture.mutations).toHaveLength(1);
  expect(fixture.mutations[0]).toMatchObject({ name: "unit_cost", revision: 1, decision: { action: "reject" } });
});

test("repeated keys do not act", async ({ page }) => {
  const fixture = await mockReview(page);
  await page.goto(reviewUrl);
  await expect(heading(page)).toBeFocused();
  await page.keyboard.down("a");
  await heading(page).focus();
  await page.keyboard.down("a");
  await page.keyboard.up("a");
  await expect(heading(page)).toBeFocused();
  expect(fixture.mutations).toHaveLength(0);
});

for (const action of ["single", "merge", "batch"] as const) {
  test(`losing owner role prevents ${action} writes`, async ({ page }) => {
    const fixture = await mockReview(page);
    await page.goto(action === "batch" ? "/ontology/review?space=business" : reviewUrl);
    if (action === "merge") await merge(page);
    if (action === "batch") {
      await page.getByText("Review a batch", { exact: true }).click();
      await page.getByLabel("Select suggestions on this page").check();
      await page.getByRole("button", { name: /^Approve \d+ selected$/ }).click();
    }
    await fixture.setMember(true);
    if (action === "batch") {
      await expect(page.getByText("Only an owner of this space can decide. You can inspect every suggestion and its evidence.")).toBeVisible();
      await page.getByRole("button", { name: "Approve selected", exact: true }).click();
      await expect(page.getByRole("alert").filter({ hasText: "Only an owner" })).toBeVisible();
    } else {
      await expect(region(page)).toContainText("Owner-only decision");
      await page.keyboard.press("a");
      await page.keyboard.press("Enter");
    }
    expect(fixture.mutations).toHaveLength(0);
  });
}

for (const theme of ["light", "dark"]) {
  test(`amber text clears AA on normal, selected and hover rows in ${theme}`, async ({ page }) => {
    await page.addInitScript(theme => localStorage.setItem("eunomia-theme", theme), theme);
    await mockReview(page);
    await page.goto(reviewUrl);
    await expect(heading(page)).toBeVisible();
    const rows = page.locator("li[data-selected]");
    for (const row of await rows.all()) {
      for (const hover of [false, true]) {
        if (hover) await row.hover();
        const ratio = await row.evaluate(row => {
          const risk = row.querySelector<HTMLElement>('[class*="risk"]');
          if (!risk) return 21;
          const rgb = (value: string) => (value.match(/[\d.]+/g) ?? []).slice(0, 3).map(Number);
          const luminance = (colour: number[]) => colour.map(v => { v /= 255; return v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; }).reduce((sum, v, i) => sum + v * [0.2126, 0.7152, 0.0722][i], 0);
          let bg: Element | null = row;
          while (bg && getComputedStyle(bg).backgroundColor === "rgba(0, 0, 0, 0)") bg = bg.parentElement;
          const fgL = luminance(rgb(getComputedStyle(risk).color));
          const bgL = luminance(rgb(bg ? getComputedStyle(bg).backgroundColor : "rgb(255,255,255)"));
          return (Math.max(fgL, bgL) + 0.05) / (Math.min(fgL, bgL) + 0.05);
        });
        expect(ratio).toBeGreaterThanOrEqual(4.5);
      }
    }
  });
}

test("human labels and relative receipt time retain exact machine values", async ({ page }) => {
  const fixture = await mockReview(page);
  await page.clock.setFixedTime(new Date("2026-11-10T03:37:00Z"));
  await page.goto(reviewUrl);
  await expect(region(page)).toContainText("Property on Stock item");
  await approve(page).click();
  const time = page.getByRole("region", { name: "Saved decision" }).locator("time");
  await expect(time).toHaveText("5 minutes ago");
  await expect(time).toHaveAttribute("datetime", "2026-11-10T03:32:00Z");
  await expect(time).toHaveAttribute("title", /2026/);
  expect(fixture.mutations[0]).toMatchObject({ kind: "stock_item", name: "unit_cost" });
});


test("an armed decision cannot write after the on-screen revision changes", async ({ page }) => {
  const fixture = await mockReview(page);
  await page.goto(reviewUrl);
  await expect(heading(page)).toBeFocused();
  await page.keyboard.press("a");
  expect(fixture.mutations).toHaveLength(0);
  fixture.model.kinds[0].properties[0].revision = 2;
  fixture.model.kinds[0].properties[0].description = "Changed while reviewing";
  fixture.notifyOntologyChange();
  await expect(region(page).getByRole("alert")).toContainText("Now: Changed while reviewing");
  await page.keyboard.press("Enter");
  expect(fixture.mutations).toHaveLength(0);
});

test("a batch rechecks the role between writes", async ({ page }) => {
  const fixture = await mockReview(page, {
    afterDecision: async count => {
      if (count !== 1) return;
      await fixture.setMember(true);
      await expect(page.getByText("Only an owner of this space can decide. You can inspect every suggestion and its evidence.")).toBeVisible();
    },
  });
  await page.goto("/ontology/review?space=business");
  await page.getByText("Review a batch", { exact: true }).click();
  await page.getByLabel("Select suggestions on this page").check();
  await page.getByRole("button", { name: /^Approve \d+ selected$/ }).click();
  await page.getByRole("button", { name: "Approve selected", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Batch results", exact: true })).toBeVisible();
  expect(fixture.mutations).toHaveLength(1);
  await expect(page.getByRole("status").filter({ hasText: "Batch results" })).toContainText("Only an owner");
});

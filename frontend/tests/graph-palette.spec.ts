import { test, expect, type Page } from "@playwright/test";
import { registerAndOnboard, uniqueEmail } from "./helpers";

async function tool(page: Page, name: string, args: object) {
  const res = await page.request.post(`/api/tools/${name}`, { data: args });
  expect(res.ok()).toBeTruthy();
  return res.json();
}

async function seedTwoRelatedPeople(page: Page) {
  const ada = await tool(page, "memory_write", {
    subject_name: "Ada Lovelace",
    subject_kind: "person",
    text: "Wrote the first published algorithm.",
  });
  const charles = await tool(page, "memory_write", {
    subject_name: "Charles Babbage",
    subject_kind: "person",
    text: "Designed the Analytical Engine.",
  });
  await tool(page, "code_relate", { from_id: ada.entity.id, to_id: charles.entity.id, label: "collaborated_with" });
}

test.describe("Entity graph", () => {
  test("draws relations that follow dragged nodes, selects on click, clears selection on vault switch", async ({ page }) => {
    await registerAndOnboard(page, uniqueEmail("graph"));
    await seedTwoRelatedPeople(page);
    await tool(page, "vault_create", { name: "Other vault" });

    await page.goto("/entities");
    const svg = page.getByRole("img", { name: "Entity relationship graph" });
    const link = svg.locator("line");
    await expect(link).toHaveCount(1);

    const adaNode = svg.locator("g").filter({ has: page.locator("title", { hasText: /^Ada Lovelace/ }) }).last();
    await expect(adaNode).toBeVisible();

    // Dragging a node moves the end of its link with it.
    await page.waitForTimeout(1500); // let the force layout settle
    const before = [await link.getAttribute("x1"), await link.getAttribute("x2")];
    const box = (await adaNode.boundingBox())!;
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.down();
    await page.mouse.move(box.x + box.width / 2 + 120, box.y + box.height / 2 + 90, { steps: 8 });
    await page.mouse.up();
    await expect
      .poll(async () => [await link.getAttribute("x1"), await link.getAttribute("x2")])
      .not.toEqual(before);

    // A click (no drag) selects the node; every node is also reachable by keyboard.
    await adaNode.click();
    await expect(page.getByRole("button", { name: "Edit", exact: true })).toBeVisible();
    await page.getByRole("button", { name: "Close" }).click();
    await page.getByRole("button", { name: "Charles Babbage" }).press("Enter");
    await expect(page.getByRole("button", { name: "Edit", exact: true })).toBeVisible();

    // Switching vault drops the selection from the old vault.
    await page.getByRole("combobox", { name: "Memory space" }).click();
    await page.getByRole("option", { name: "Other vault" }).click();
    await expect(page.getByRole("button", { name: "Edit", exact: true })).toHaveCount(0);
  });

  test("vector cloud layers vaults in one 3D space, including a merge of them", async ({ page }) => {
    await registerAndOnboard(page, uniqueEmail("cloud"));
    await seedTwoRelatedPeople(page);
    const team = await tool(page, "vault_create", { name: "Team" });
    await tool(page, "memory_write", { subject_name: "Grace Hopper", subject_kind: "person", text: "Built the first compiler.", vault_id: team.id });
    const personal = (await tool(page, "vault_list", {})).results.find((v: { kind: string }) => v.kind === "personal");
    const merged = await tool(page, "vault_merge", { vault_id_a: personal.id, vault_id_b: team.id, name: "Everything" });
    expect(merged.entities).toBe(3);

    // All three views share one memory space; start this one on Personal (else it opens on the newest shared space).
    await page.addInitScript(() => window.localStorage.setItem("eunomia.graph.space", ""));
    await page.goto("/entities");
    await page.getByRole("tab", { name: "Memory map" }).click();
    const cloud = page.getByRole("img", { name: "Memory map" });
    await expect(cloud.locator("canvas")).toBeVisible();

    const layers = page.getByRole("group", { name: "Memory space layers" });
    await layers.getByRole("button", { name: /Team/ }).click();
    await layers.getByRole("button", { name: /Everything/ }).click();
    await expect(layers.getByRole("button", { name: /Everything/ })).toContainText("3"); // one point per memory
    await expect(layers.getByRole("button", { name: /^Team/ })).toContainText("1");

    await page.getByRole("button", { name: "Built the first compiler." }).first().press("Enter");
    await expect(page.getByText(/· Fact$/).first()).toBeVisible();
  });
});

test.describe("Command palette", () => {
  test("opens with the shortcut, searches pages and entities, and starts fresh each time", async ({ page }) => {
    await registerAndOnboard(page, uniqueEmail("palette"));
    await seedTwoRelatedPeople(page);
    await page.goto("/");
    await expect(page.getByText("Connected sources", { exact: true })).toBeVisible(); // hydrated

    const input = page.getByPlaceholder("Jump to a page…");
    const palette = page.getByRole("dialog", { name: "Search", exact: true });
    await page.keyboard.press("ControlOrMeta+k");
    await expect(input).toBeFocused();

    await input.fill("Ada");
    await expect(palette.getByText("Ada Lovelace")).toBeVisible();

    // Below the search threshold, data hits are hidden again.
    await input.fill("A");
    await expect(palette.getByText("Ada Lovelace")).toHaveCount(0);

    await input.fill("sett");
    await page.keyboard.press("Enter");
    await expect(page).toHaveURL(/\/settings$/);
    await expect(input).toHaveCount(0);

    await page.keyboard.press("ControlOrMeta+k");
    await expect(input).toHaveValue("");
    await page.keyboard.press("Escape");
    await expect(input).toHaveCount(0);
  });
});

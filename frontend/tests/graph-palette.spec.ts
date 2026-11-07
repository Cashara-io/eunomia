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

    // List is the default tab; this test is about the Connections canvas
    await page.addInitScript(() => window.localStorage.setItem("eunomia.entities.view", "graph"));
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

  test("memory map lays several spaces over each other, keeps one, and follows the shared space", async ({ page }) => {
    await registerAndOnboard(page, uniqueEmail("cloud"));
    await seedTwoRelatedPeople(page);
    const team = await tool(page, "vault_create", { name: "Team" });
    await tool(page, "memory_write", { subject_name: "Grace Hopper", subject_kind: "person", text: "Built the first compiler.", vault_id: team.id });

    // start on Personal (else it opens on the newest shared space); only the first load, so the reload below keeps the pick
    await page.addInitScript(() => {
      if (window.localStorage.getItem("eunomia.graph.space") === null) window.localStorage.setItem("eunomia.graph.space", "");
    });
    await page.goto("/entities");
    await page.getByRole("tab", { name: "Memory map" }).click();
    const map = page.getByRole("img", { name: "Memory map" });
    await expect(map).toBeVisible();
    await expect(page.getByText(/^2 memories/)).toBeVisible();

    const layers = page.getByRole("group", { name: "Spaces on the map" });
    const personalChip = layers.getByRole("button", { name: /^Personal/ });
    const teamChip = layers.getByRole("button", { name: /^Team/ });
    await expect(personalChip).toHaveAttribute("aria-pressed", "true");
    await expect(teamChip).toHaveAttribute("aria-pressed", "false");

    // add Team: both spaces, each chip counting its own points
    await teamChip.click();
    await expect(page.getByText(/^3 memories/)).toBeVisible();
    await expect(teamChip).toHaveAttribute("aria-pressed", "true");
    await expect(teamChip).toContainText("1");
    await expect(personalChip).toContainText("2");

    // drop Personal: Team alone, and the other views follow to Team
    await personalChip.click();
    await expect(page.getByText(/^1 memory/)).toBeVisible();
    // the last space can't be turned off
    await teamChip.click();
    await expect(teamChip).toHaveAttribute("aria-pressed", "true");
    await expect(page.getByText(/^1 memory/)).toBeVisible();
    await page.getByRole("tab", { name: "Connections" }).click();
    await expect(page.getByRole("combobox", { name: "Memory space" })).toContainText("Team");

    // and a reload keeps the map's layers
    await page.reload();
    await page.getByRole("tab", { name: "Memory map" }).click();
    await expect(page.getByText(/^1 memory/)).toBeVisible();
    await map.getByRole("button", { name: "Built the first compiler." }).first().press("Enter");
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

    // Memory text matches too, under the entity it is about; Enter opens that entity.
    await input.fill("Analytical");
    const memory = palette.getByRole("button", { name: /Designed the Analytical Engine/ });
    await expect(memory).toBeVisible();
    await expect(memory).toContainText("Charles Babbage");
    await expect(palette.getByText(/Type 2 or more letters/)).toHaveCount(0);

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

    // Keyboard shortcuts: "g p" goes to People & topics, "?" lists them all.
    await page.keyboard.press("g");
    await page.keyboard.press("p");
    await expect(page).toHaveURL(/\/entities/);
    await page.keyboard.press("?");
    await expect(page.getByRole("dialog", { name: "Keyboard shortcuts" })).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog", { name: "Keyboard shortcuts" })).toHaveCount(0);
  });
});

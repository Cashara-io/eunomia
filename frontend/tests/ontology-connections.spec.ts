import { test, expect } from "@playwright/test";
import { registerAndOnboard, uniqueEmail } from "./helpers";
test("real authorised Connections route, directed links and space isolation", async ({
  page,
}) => {
  test.setTimeout(90000);
  await registerAndOnboard(page, uniqueEmail("connections"));
  const vault = await (
    await page.request.post("/api/vaults", {
      data: { name: "Connections fixture" },
    })
  ).json();
  const other = await (
    await page.request.post("/api/vaults", { data: { name: "Another space" } })
  ).json();
  const product = await (
    await page.request.post("/api/entities", {
      data: { kind: "product", name: "House blend 1kg", vault_id: vault.id },
    })
  ).json();
  const supplier = await (
    await page.request.post("/api/entities", {
      data: { kind: "supplier", name: "North Coast Beans", vault_id: vault.id },
    })
  ).json();
  expect(
    (
      await page.request.post(`/api/entities/${supplier.id}/relations`, {
        data: { to_id: product.id, label: "supplies" },
      })
    ).ok(),
  ).toBeTruthy();
  await page.goto(`/ontology?space=${encodeURIComponent(vault.id)}`);
  await page
    .getByRole("navigation", { name: "Ontology views" })
    .getByRole("link", { name: "Connections", exact: true })
    .click();
  await page
    .getByRole("searchbox", { name: "Find an object" })
    .fill("House blend");
  await page
    .getByRole("button", { name: "House blend 1kg Product", exact: true })
    .click();
  await expect(
    page.getByRole("region", { name: "Connections in words" }),
  ).toContainText("North Coast Beans supplies House blend 1kg.");
  await expect(
    page.getByRole("complementary", { name: "Selected object" }),
  ).toContainText("House blend 1kg");
  await page.getByRole("button", { name: "Show as table" }).click();
  await expect(
    page.getByRole("table", { name: "Directed links (1)" }),
  ).toContainText("North Coast Beans supplies House blend 1kg.");
  await page.goto(
    `/ontology/connections?space=${encodeURIComponent(other.id)}&focus=${encodeURIComponent(product.id)}`,
  );
  await expect(
    page.getByRole("heading", { name: "This object is unavailable here." }),
  ).toBeVisible();
  await expect(page.getByRole("table")).toHaveCount(0);
});

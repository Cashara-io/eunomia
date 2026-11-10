import { test, expect, type Page } from "@playwright/test";
import { mockReceipts } from "./fixtures/receipts";
const profile = "/ontology/objects/house?space=business&section=Properties";
async function cacheAction(page: Page, action: string): Promise<unknown[][]> {
  return page.locator("main").evaluate((el, action) => {
    const key = Object.keys(el).find(k => k.startsWith("__reactFiber$"))!;
    let fiber = Reflect.get(el, key);
    while (fiber) {
      const client = fiber.memoizedProps?.client;
      if (client?.getQueryCache) {
        if (action === "recheck") void client.invalidateQueries({ queryKey: ["receipt-access", "business"] });
        if (action === "seed") {
          client.setQueryData(["object-profile", "business", "other", 0], { marker: "OTHER CACHED OBJECT" });
          client.setQueryData(["objects", "personal"], { marker: "OTHER SPACE" });
        }
        return client.getQueryCache().getAll().map((q: {queryKey: unknown[]}) => q.queryKey);
      }
      fiber = fiber.return;
    }
    throw new Error("Query provider not found");
  }, action);
}
async function noProtectedText(page: Page) {
  await expect(page.locator("main")).not.toContainText("House blend 1kg");
  await expect(page.locator("main")).not.toContainText("products.csv");
  await expect(page.locator("main")).not.toContainText("AUD 12.00");
}
for (const change of ["replacement", "remaining name match after deletion"]) test(`${change} flags the differing current cell on receipt and source`, async ({ page }) => {
  const { state } = await mockReceipts(page);
  state.profile.properties.find(p => p.name === "unit_cost")!.state = "recorded";
  state.profile.sources[0].document!.row!.cells.unit_cost = "99.00";
  await page.goto(profile);
  const warning = "The file now says AUD 99.00. The recorded value is AUD 12.00. The file may have changed since this value was recorded.";
  await expect(page.getByRole("region", { name: "Properties", exact: true })).toContainText(warning);
  await expect(page.getByRole("button", {name: "Inspect source for Unit cost"})).toHaveText("products.csv, row 42 (matched by name)");
  await page.getByRole("button", {name: "Inspect source for Unit cost"}).click();
  const inspector = page.getByLabel("Source inspector", {exact: true});
  await expect(inspector.locator('[data-highlight="true"]')).toContainText(warning);
});
for (const reason of ["ambiguous", "deleted row", "replaced document"]) test(`${reason} never promises a source row`, async ({ page }) => {
  const { state } = await mockReceipts(page);
  state.profile.sources[0].document!.row = null;
  await page.goto(profile);
  await page.getByRole("button", {name: "Inspect source for Unit cost"}).click();
  const inspector = page.getByLabel("Source inspector", {exact:true});
  await expect(inspector).toContainText("No unique current row");
  await expect(inspector).toContainText("Several rows may share this name, or the row may have been removed or the document replaced.");
  await expect(inspector.locator("dl")).toHaveCount(0);
});
test("deleted document explains source unavailability", async ({ page }) => {
  const { state } = await mockReceipts(page);
  state.profile.sources[0].state = "unavailable"; delete state.profile.sources[0].document;
  await page.goto(profile);
  await page.getByRole("button", {name: "Inspect source for Unit cost"}).click();
  await expect(page.getByLabel("Source inspector", {exact:true})).toContainText("deleted");
});
for (const paused of [false, true]) test(`${paused ? "paused" : "offline"} membership recheck hides DOM text and keeps input`, async ({ page, context }) => {
  const {state} = await mockReceipts(page);
  await page.goto("/ontology/objects?space=business&kind=product&search=House&selected=house");
  await expect(page.getByRole("heading", {name:"House blend 1kg",exact:true})).toBeVisible();
  await page.clock.install();
  state.membership = "pending";
  if (paused) await page.evaluate(async () => {
    // A browser offline event puts TanStack reads into fetchStatus paused.
    window.dispatchEvent(new Event("offline"));
  });
  else await context.setOffline(true);
  await cacheAction(page, "recheck");
  await noProtectedText(page);
  await page.clock.fastForward(11000);
  state.membership = "ok";
  await context.setOffline(false);
  await page.evaluate(() => window.dispatchEvent(new Event("online")));
  await expect(page.getByLabel("Search objects")).toHaveValue("House");
});
test("hanging membership times out to unknown and retries", async ({ page }) => {
  const {state} = await mockReceipts(page); await page.goto(profile);
  await expect(page.getByRole("heading", {name:"House blend 1kg",exact:true})).toBeVisible();
  await page.clock.install(); state.membership="pending";
  await cacheAction(page,"recheck"); await noProtectedText(page);
  await page.clock.fastForward(11000);
  await expect(page.getByRole("button",{name:"Try again",exact:true})).toBeVisible();
  await noProtectedText(page);
});
test("denial purges space caches and rejects a late profile response", async ({ page }) => {
  const {state} = await mockReceipts(page); await page.goto(profile);
  await expect(page.getByRole("heading", {name:"House blend 1kg",exact:true})).toBeVisible();
  await cacheAction(page,"seed");
  let release = () => {};
  const held = new Promise<void>(resolve => { release=resolve; });
  let delivered = () => {};
  const completed = new Promise<void>(resolve => { delivered = resolve; });
  await page.route("**/api/vaults/business/ontology/objects/*", async r => { await held; await r.fulfill({json:state.profile}); delivered(); });
  await page.getByRole("button",{name:"Refresh ontology"}).click();
  state.membership="denied"; await cacheAction(page,"recheck");
  await expect(page.getByRole("alert").filter({hasText:"unavailable to you"})).toBeVisible();
  release(); await completed; await noProtectedText(page);
  await expect.poll(async () => (await cacheAction(page,"read")).filter(k => k.includes("business"))).toEqual([]);
  expect((await cacheAction(page,"read")).some(k => k.includes("personal"))).toBe(true);
});
test("deleted object clears only itself after membership confirmation", async ({ page }) => {
  const {state} = await mockReceipts(page); await page.goto(profile);
  await expect(page.getByRole("heading", {name:"House blend 1kg",exact:true})).toBeVisible();
  await cacheAction(page,"seed"); state.mode="missing";
  await page.getByRole("button",{name:"Refresh ontology"}).click();
  await expect(page.getByRole("region",{name:"Object unavailable"})).toBeVisible();
  const keys=await cacheAction(page,"read");
  expect(keys.some(k=>k[0]==="object-profile" && k[2]==="house")).toBe(false);
  expect(keys.some(k=>k[0]==="object-profile" && k[2]==="other")).toBe(true);
});
test("compact source keeps page two and pagination reachable", async ({ page }) => {
  await mockReceipts(page);
  await page.goto("/ontology/objects?space=business&kind=product&page=2&selected=house");
  await page.getByRole("button",{name:"Inspect source for Unit cost"}).click();
  await expect(page).toHaveURL(/page=2/);
  await expect(page.getByRole("navigation",{name:"Object pages"})).toBeVisible();
  await expect(page.getByRole("button",{name:"Previous",exact:true})).toBeEnabled();
});
test("import activity opens the imported kind Objects", async ({ page }) => {
  await mockReceipts(page);
  await page.route("**/api/vaults/business/ontology/activity?*",r=>r.fulfill({json:{events:[{seq:1,at:"2026-11-10T03:32:00Z",actor:"owner",actor_name:"Alex",actor_type:"user",action:"imported",definition_type:"import",kind:"product",name:"products.csv",target:"document:products",revision:1,reason:null}],next:1,more:false}}));
  await page.goto("/ontology/activity?space=business");
  await page.getByRole("link",{name:"Products.csv",exact:true}).click();
  await expect(page).toHaveURL(/ontology\/objects\?space=business&kind=product/);
  await expect(page.getByRole("table")).toBeVisible();
});
for (const theme of ["light", "dark"]) test(`mobile receipts keep values beside labels and emphasise mismatches in ${theme}`, async ({ page }) => {
  await page.setViewportSize({width:390,height:844});
  await page.addInitScript(theme => localStorage.setItem("eunomia-theme",theme),theme);
  const {state}=await mockReceipts(page);
  state.profile.sources[0].document!.row!.cells.unit_cost="99.00";
  await page.goto(profile);
  const price=page.getByRole("region",{name:"Properties",exact:true}).locator('dl > div').filter({has:page.locator('dt').getByText('Price',{exact:true})}).first();
  const geometry=await price.evaluate(el=>({label:el.querySelector('dt')!.getBoundingClientRect().top,value:el.querySelector('dd')!.getBoundingClientRect().top,padding:parseFloat(getComputedStyle(el).paddingBottom)}));
  expect(Math.abs(geometry.label-geometry.value)).toBeLessThan(5);
  expect(geometry.padding).toBeLessThanOrEqual(14);
  await expect(page.locator('main')).not.toContainText('not independently verified');
  const warning=page.getByRole('note').first();
  await expect(warning).toContainText('The file now says AUD 99.00.');
  expect(await warning.evaluate(el=>parseFloat(getComputedStyle(el).borderLeftWidth))).toBeGreaterThanOrEqual(4);
});

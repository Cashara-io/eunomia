import { test, expect, type Page } from "@playwright/test";
import { mockReceipts } from "./fixtures/receipts";
import { mockConnections } from "./fixtures/connections";
import { protectedValues } from "./fixtures/protected-values";
import fixture from "./fixtures/ontology-ui.json";

async function cacheAction(page: Page, action = "recheck") {
  return page.locator("main").evaluate((el, {action, fixture}) => {
    let fiber = Reflect.get(el, Object.keys(el).find(k => k.startsWith("__reactFiber$"))!);
    while (fiber) {
      const client = fiber.memoizedProps?.client;
      if (client?.getQueryCache) {
        if (action === "recheck") void client.invalidateQueries({queryKey:["receipt-access","business"]});
        if (action === "warm") {
          client.setQueryData(["ontology","business"], fixture.ontology);
          client.setQueryData(["connection-search","business","My own query","",0], {results:[{id:"house",name:"House blend 1kg",kind:"product"}]});
        }
        return;
      }
      fiber = fiber.return;
    }
    throw new Error("Query provider not found");
  }, {action, fixture});
}
async function mockBoth(page: Page) {
  await mockConnections(page);
  const result = await mockReceipts(page);
  // The broad Ontology fixture is installed last, so explicitly retain tool reads.
  await page.route("**/api/tools/entities_search", r => r.fulfill({json:{results:[{id:"house",name:"House blend 1kg",kind:"product"}],has_more:false}}));
  return result;
}
async function absent(page: Page) {
  await expect.poll(() => protectedValues(page, ["House blend 1kg", "North Coast Beans", "products.csv", "AUD 12.00", "Banksia Coffee"])).toEqual([]);
}
for (const uncertain of ["checking", "unknown", "denied"]) test(`Connections search then Objects and Back shares ${uncertain} access`, async ({page}) => {
  const {state} = await mockBoth(page);
  await page.goto("/ontology/connections?space=business&q=My+own+query");
  await expect(page.getByLabel("Object search results")).toContainText("House blend 1kg");
  await page.getByRole("link",{name:"Objects",exact:true}).click();
  await expect(page.getByLabel("Search objects")).toBeVisible();
  await page.clock.install();
  state.membership = uncertain === "denied" ? "denied" : "pending";
  await cacheAction(page);
  if (uncertain === "unknown") await page.clock.fastForward(11_000);
  const label = uncertain === "denied" ? "Retry space access" : uncertain === "unknown" ? "Try again" : null;
  if (label) await expect(page.getByRole("button",{name:label,exact:true})).toBeVisible();
  else await expect(page.getByText("Checking access",{exact:true})).toBeVisible();
  await page.goBack();
  await expect(page).toHaveURL(/connections.*q=My\+own\+query/);
  if (label) await expect(page.getByRole("button",{name:label,exact:true})).toBeVisible();
  else await expect(page.getByText("Checking access",{exact:true})).toBeVisible();
  await absent(page);
  if (uncertain !== "denied") await expect(page.getByRole("searchbox")).toHaveValue("My own query");
  await expect(page.getByLabel("Object search results")).toHaveCount(0);
  await page.goForward();
  await absent(page);
});
test("Back after denial may restore user input but no server data", async ({page}) => {
  const {state} = await mockReceipts(page);
  await page.goto("/ontology/objects?space=business&kind=product&search=My+own+query");
  await expect(page.getByLabel("Search objects")).toHaveValue("My own query");
  await page.evaluate(() => history.pushState(null,"","?space=business&kind=product&search=Other"));
  state.membership="denied";
  await cacheAction(page);
  await expect(page.getByRole("button",{name:"Retry space access"})).toBeVisible();
  await expect(page).toHaveURL(/objects\?space=business$/);
  await page.goBack();
  await expect(page).toHaveURL(/search=My\+own\+query/);
  await absent(page);
  await expect(page.getByRole("button",{name:"Retry space access"})).toBeVisible();
});
test("denial clears stored return context for this space only", async ({page}) => {
  const {state} = await mockReceipts(page);
  await page.goto("/ontology/objects?space=business&kind=product&search=House+blend+1kg");
  await page.getByRole("link",{name:"House blend 1kg",exact:true}).click();
  await expect(page.getByRole("heading",{name:"House blend 1kg",exact:true})).toBeVisible();
  await page.evaluate(() => {
    sessionStorage.setItem("object-position:/ontology/objects?space=personal&search=Other", "other space");
    sessionStorage.setItem("ontology-return:personal", "other space");
  });
  state.membership="denied";
  await cacheAction(page);
  await expect(page.getByRole("button",{name:"Retry space access"})).toBeVisible();
  expect(await page.evaluate(() => Object.keys(sessionStorage).filter(k => k.startsWith("object-position:") && new URL(k.slice("object-position:".length), location.origin).searchParams.get("space") === "business" || k === "ontology-return:business"))).toEqual([]);
  expect(await page.evaluate(() => sessionStorage.getItem("ontology-return:personal"))).toBe("other space");
});
for (const route of ["/ontology", "/ontology/connections", "/ontology/objects", "/ontology/objects/house", "/ontology/review", "/ontology/activity", "/ontology/schema", "/ontology/kinds/product"]) test(`shared unknown access gates ${route}`, async ({page}) => {
  const {state} = await mockReceipts(page);
  await page.goto(route+"?space=business");
  await expect(page.getByRole("navigation",{name:"Ontology views"})).toBeVisible();
  await page.clock.install();
  state.membership="pending";
  await cacheAction(page);
  await expect(page.getByText("Checking access",{exact:true})).toBeVisible();
  await absent(page);
  await page.clock.fastForward(11_000);
  await expect(page.getByRole("button",{name:"Try again",exact:true})).toBeVisible();
  await absent(page);
});
test("a fresh Ontology mount never reveals warm data before a successful authorised read", async ({page}) => {
  await mockBoth(page);
  await page.goto("/ontology/connections?space=business&q=My+own+query");
  await expect(page.getByLabel("Object search results")).toBeVisible();
  await page.getByRole("link",{name:"Memory spaces",exact:true}).click();
  await expect(page).toHaveURL(/\/vaults$/);
  await cacheAction(page,"warm");
  await page.route("**/api/vaults/business/**", r => r.fulfill({status:503,json:{code:"http.error"}}));
  await page.route("**/api/tools/**", r => r.fulfill({status:503,json:{code:"http.error"}}));
  await page.evaluate(() => {
    const leaks: string[] = [];
    Reflect.set(window, "__warmCacheLeaks", leaks);
    new MutationObserver(() => {
      const text = document.querySelector("main")?.textContent ?? "";
      if (text.includes("House blend 1kg")) leaks.push(text);
    }).observe(document.documentElement, {subtree:true, childList:true, characterData:true, attributes:true});
  });
  await page.goBack();
  await expect(page).toHaveURL(/connections/);
  await absent(page);
  expect(await page.evaluate(() => Reflect.get(window,"__warmCacheLeaks"))).toEqual([]);
});
test("attribute protection detects nested URL and HTML encoding", async ({page}) => {
  await mockReceipts(page);
  await page.goto("/ontology/objects?space=business");
  await expect(page.getByLabel("Search objects")).toBeVisible();
  await page.evaluate(() => {
    const el=document.createElement("div");
    el.setAttribute("data-nested", "&amp;#80;ROTECTED%252BATTRIBUTE");
    document.body.append(el);
  });
  expect((await protectedValues(page,["PROTECTED ATTRIBUTE"])).some(value => value === "PROTECTED ATTRIBUTE")).toBe(true);
});

for (const [recorded, current] of [["Paris", "paris"], ["SKU-AbC", "SKU-ABC"]]) test(`case-only ${recorded} receipt and source use a mild note`, async ({page}) => {
  const {state} = await mockReceipts(page);
  const property = state.profile.properties.find(p => p.name === "unit_cost")!;
  property.datatype="text"; property.unit=null; property.value=recorded;
  state.profile.sources[0].document!.row!.cells.unit_cost=current;
  await page.goto("/ontology/objects/house?space=business");
  const note=page.getByRole("note").filter({hasText:"differs only by capital letters"});
  await expect(note).toHaveText("The current cell differs only by capital letters.");
  expect(await note.evaluate(el => parseFloat(getComputedStyle(el).borderLeftWidth))).toBe(0);
  await expect(page.locator('[data-mismatch="true"]')).toHaveCount(0);
  await page.getByRole("button",{name:"Inspect source for Unit cost"}).click();
  await expect(page.getByLabel("Source inspector",{exact:true}).getByRole("note")).toHaveText("The current cell differs only by capital letters.");
});

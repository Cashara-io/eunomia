import { mkdir } from "node:fs/promises";
import {test,expect} from "@playwright/test";
import {mockConnections} from "./fixtures/connections";
import {receiptFixture} from "./fixtures/receipts";
for (const width of [1440, 390]) for (const theme of ["light", "dark"]) test(`Connections source summaries disclose name matches and disagreements at ${width} ${theme}`, async ({page})=>{
  await page.setViewportSize({width,height:width===390?844:900});
  await page.addInitScript(theme=>localStorage.setItem("eunomia-theme",theme),theme);
  await mockConnections(page);
  const data=receiptFixture();
  data.sources[0].document!.row!.cells.unit_cost="99.00";
  await page.route("**/api/vaults/business/ontology/objects/*",r=>r.fulfill({json:data}));
  await page.goto("/ontology/connections?space=business&focus=house&selected=house");
  const inspector=page.getByLabel("Selected object",{exact:true});
  await expect(inspector).toContainText("products.csv, row 42 (matched by name)");
  await expect(inspector).toContainText("The file now says AUD 99.00. The recorded value is AUD 12.00.");
  const output=process.env.ONTOLOGY_RECEIPT_SCREENSHOTS;
  if (output) { await mkdir(output,{recursive:true}); await page.screenshot({path:`${output}/connections-receipt-${width}-${theme}.png`,fullPage:true}); }
});

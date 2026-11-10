import { test, expect } from "@playwright/test";
import { mkdir, writeFile } from "node:fs/promises";
import { mockOntology, type State } from "./fixtures/ontology-ui";

const routes = [
  {
    name: "overview",
    path: "/ontology",
    failure: "/ontology",
    states: ["populated", "loading", "empty", "denied", "error"],
  },
  {
    name: "objects",
    path: "/ontology/objects",
    failure: "/ontology/objects",
    states: ["populated", "loading", "empty", "filtered", "denied", "error"],
  },
  {
    name: "structure",
    path: "/ontology/schema",
    failure: "/ontology",
    states: ["populated", "loading", "empty", "filtered", "denied", "error"],
  },
  {
    name: "review",
    path: "/ontology/review",
    failure: "/ontology",
    states: ["populated", "loading", "empty", "filtered", "denied", "error"],
  },
  {
    name: "activity",
    path: "/ontology/activity",
    failure: "/ontology/activity",
    states: ["populated", "loading", "empty", "filtered", "denied", "error"],
  },
  {
    name: "kind",
    path: "/ontology/kinds/product",
    failure: "/ontology",
    states: ["populated", "loading", "empty", "denied", "error"],
  },
  {
    name: "profile",
    path: "/ontology/objects/house",
    failure: "/ontology/objects/",
    states: ["populated", "loading", "empty", "denied", "error"],
  },
] satisfies { name: string; path: string; failure: string; states: State[] }[];

for (const route of routes)
  for (const state of route.states)
    test(`${route.name} ${state}`, async ({ browser }) => {
      test.setTimeout(90_000);
      const entries = [];
      for (const width of [1440, 390])
        for (const theme of ["light", "dark"]) {
          const context = await browser.newContext({
            viewport: { width, height: width === 1440 ? 900 : 844 },
            deviceScaleFactor: 1,
            locale: "en-AU",
            timezoneId: "Australia/Melbourne",
            reducedMotion: "reduce",
            hasTouch: width === 390,
            isMobile: width === 390,
          });
          const page = await context.newPage();
          await page.addInitScript(
            (theme) => localStorage.setItem("eunomia-theme", theme),
            theme,
          );
          await mockOntology(page, state, route.failure);
          const filter =
            state === "filtered"
              ? route.name === "objects"
                ? "&kind=product&search=nonexistent"
                : route.name === "activity"
                  ? ""
                  : "&q=nonexistent"
              : "";
          await page.goto(`${route.path}?space=business${filter}`);
          await expect(
            page.getByRole("navigation", { name: "Ontology views" }),
          ).toBeVisible();
          if (state === "error" || state === "denied")
            await expect(
              page.locator("main").getByRole("alert").first(),
            ).toBeVisible({ timeout: 10000 });
          else if (state === "loading")
            await expect(
              page.locator("main [role=status]").first(),
            ).toBeAttached();
          else if (route.name === "overview" && state === "populated")
            await expect(
              page.getByRole("heading", { name: "House blend 1kg: unit cost" }),
            ).toBeVisible();
          else await page.waitForTimeout(250);
          if (state === "filtered" && route.name === "activity") {
            await page
              .getByRole("combobox", { name: "Activity actor" })
              .selectOption("user");
          }
          await page.evaluate(() => document.fonts.ready);
          await page.addStyleTag({
            content: "nextjs-portal { display: none !important; }",
          });
          const pageWidth = await page.evaluate(
            () => document.documentElement.scrollWidth,
          );
          expect(
            pageWidth,
            `${route.name} ${state} ${width} ${theme} overflow`,
          ).toBeLessThanOrEqual(width);
          if (process.env.ONTOLOGY_SCREENSHOTS) {
            const dir = process.env.ONTOLOGY_SCREENSHOTS;
            await mkdir(dir, { recursive: true });
            const name = `${route.name}-${state}-${width}-${theme}.png`;
            await page.screenshot({
              path: `${dir}/${name}`,
              fullPage: true,
              animations: "disabled",
            });
            entries.push({
              name,
              route: page.url(),
              state,
              width,
              theme,
              pageWidth,
              dpr: 1,
              browser: browser.version(),
              captured: new Date().toISOString(),
            });
          }
          await context.close();
        }
      if (process.env.ONTOLOGY_SCREENSHOTS)
        await writeFile(
          `${process.env.ONTOLOGY_SCREENSHOTS}/${route.name}-${state}.json`,
          JSON.stringify(entries, null, 2),
        );
    });

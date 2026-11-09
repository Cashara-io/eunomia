import { execFile } from "node:child_process";
import { mkdtemp, readFile, readdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";
import { test, expect } from "@playwright/test";

let directory: string;
let script: string;
let css: string;

test.beforeAll(async () => {
  directory = await mkdtemp(join(tmpdir(), "eunomia-morph-test-"));
  await promisify(execFile)("bun", [
    "build", "tests/fixtures/morph-text.tsx", "--target=browser",
    `--outdir=${directory}`, '--define=process.env.NODE_ENV="production"',
  ]);
  script = await readFile(join(directory, "morph-text.js"), "utf8");
  const styles = (await readdir(directory)).filter((file) => file.endsWith(".css"));
  css = (await Promise.all(styles.map((file) => readFile(join(directory, file), "utf8")))).join("\n");
});

test.afterAll(async () => {
  if (directory) await rm(directory, { recursive: true, force: true });
});

for (const reduced of [false, true]) {
  test(`MorphText keeps one readable and selectable value, reduced motion ${reduced}`, async ({ page }) => {
    await page.emulateMedia({ reducedMotion: reduced ? "reduce" : "no-preference" });
    await page.route("http://morph.test/", (route) => route.fulfill({
      contentType: "text/html",
      body: '<html><head></head><body><div id="root"></div></body></html>',
    }));
    await page.goto("http://morph.test/");
    if (css) await page.addStyleTag({ content: css });
    await page.addScriptTag({ content: script, type: "module" });
    const host = page.getByTestId("value");
    const wrapper = host.locator("[data-text-morph]");
    const checkValue = async (value: string) => {
      await expect(page.getByText(value, { exact: true })).toHaveCount(1);
      await expect(host).toHaveAccessibleName(value);
      await expect(host).toHaveText(value);
      expect(await wrapper.evaluate((node) => {
        const range = document.createRange();
        range.selectNodeContents(node);
        const selection = window.getSelection();
        selection?.removeAllRanges();
        selection?.addRange(range);
        return selection?.toString();
      })).toBe(value);
    };
    await checkValue("Indexing");
    for (const value of ["Ready", "Indexing", "Ready"]) {
      await page.getByRole("button", { name: "Change value" }).click();
      await checkValue(value);
      await wrapper.evaluate(async (node) => {
        await Promise.all(node.getAnimations({ subtree: true }).map((animation) => animation.finished));
      });
      await checkValue(value);
      expect(await wrapper.evaluate((node) => node.childNodes.length)).toBe(1);
      await expect(wrapper.locator("*")).toHaveCount(0);
      if (reduced) {
        expect(await wrapper.evaluate((node) => node.getAnimations({ subtree: true }).length)).toBe(0);
      }
    }
  });
}

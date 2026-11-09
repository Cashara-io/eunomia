import { test, expect } from "@playwright/test";
import { registerAndOnboard, uniqueEmail } from "./helpers";

import fixture from "../../backend/tests/fixtures/chat_markdown.json";

const sample = fixture.cases.find(c => c.name === "e2e");
if (!sample) throw new Error("Missing checked chat e2e fixture");
const REPLY = sample;

test("assistant replies render as markdown; user text and raw HTML stay literal", async ({ page }) => {
  await registerAndOnboard(page, uniqueEmail("md"));
  const thread = { id: "chat_thread:md1", title: "Markdown", created_at: new Date().toISOString(), updated_at: new Date().toISOString() };
  await page.route("**/api/chat/threads", (route) =>
    route.request().method() === "GET" ? route.fulfill({ json: [thread] }) : route.continue()
  );
  await page.route("**/api/chat/threads/*/history", (route) =>
    route.fulfill({
      json: [
        { role: "user", content: "**not bold** please" },
        { role: "assistant", content: REPLY.visible_text, tree: REPLY.tree },
      ],
    })
  );

  await page.goto("/chat");
  const md = page.locator(".md");
  await expect(md.getByRole("heading", { name: "Plan" })).toBeVisible();
  await expect(md.locator("strong")).toHaveText("bold");
  await expect(md.locator("em")).toHaveText("italics");
  await expect(md.locator("p code")).toHaveText("inline code");
  await expect(md.getByRole("link", { name: "the docs" })).toHaveAttribute("href", "https://example.com/docs");
  await expect(md.getByRole("link", { name: "the docs" })).toHaveAttribute("target", "_blank");
  await expect(md.locator("li")).toHaveCount(2);
  await expect(md.locator("pre code")).toContainText('let answer = "42";');
  await expect(md.locator("table td").first()).toHaveText("1");

  // the user's own message is shown as typed, and a <script> in a reply is inert
  await expect(page.getByText("**not bold** please")).toBeVisible();
  expect(await page.evaluate(() => (window as unknown as { __pwned?: boolean }).__pwned)).toBeUndefined();
});


test("checked corpus text matches the browser DOM without another parser", async ({ page }) => {
  await registerAndOnboard(page, uniqueEmail("tree-parity"));
  const thread = { id: "chat_thread:parity", title: "Checked corpus", created_at: new Date().toISOString(), updated_at: new Date().toISOString() };
  await page.route("**/api/chat/threads", route => route.fulfill({ json: [thread] }));
  await page.route("**/api/chat/threads/*/history", route => route.fulfill({ json: fixture.cases.flatMap(sample => [
    {role: "user", content: sample.name},
    {role: "assistant", content: sample.visible_text, tree: sample.tree},
  ]) }));
  await page.goto("/chat");
  const replies = page.locator(".md");
  await expect(replies).toHaveCount(fixture.cases.length);
  for (const [i, sample] of fixture.cases.entries()) {
    expect(await replies.nth(i).textContent()).toBe(sample.visible_text);
    await expect(replies.nth(i).locator("img,script")).toHaveCount(0);
  }
});

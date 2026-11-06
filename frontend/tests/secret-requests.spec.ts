import { expect, test, type APIRequestContext, type Page } from "@playwright/test";
import { randomBytes } from "node:crypto";
import { registerAndOnboard, uniqueEmail } from "./helpers";

// Secret requests (docs/secret-requests.md): an agent asks for a secret with `secret_request` over the REST
// tool API and a personal access token, the person sees the masked pop-up in the web app, pastes the value
// and saves (or declines), and the agent only ever gets the handle. The browser talks to the real backend;
// the agent is `request` with the token, as an MCP or REST client would be.
const VALUE = `sk_e2e_${randomBytes(12).toString("hex")}`;

async function agentToken(page: Page): Promise<string> {
  const res = await page.request.post("/api/auth/tokens", { data: { name: "e2e agent" } });
  expect(res.ok()).toBeTruthy();
  return (await res.json()).token;
}

async function agent(request: APIRequestContext, token: string, tool: string, args: object) {
  const res = await request.post(`/api/tools/${tool}`, { data: args, headers: { authorization: `Bearer ${token}` } });
  const text = await res.text();
  expect(text).not.toContain(VALUE);
  return JSON.parse(text);
}

test.describe("secret requests", () => {
  test.describe.configure({ mode: "serial" });

  test("the pop-up masks the value, Save hands the agent only a handle, Decline tells it no", async ({ page, request }) => {
    await registerAndOnboard(page, uniqueEmail("secret-req"));
    const token = await agentToken(page);

    const asked = await agent(request, token, "secret_request", {
      name: "e2e_api_key",
      label: "Example API key",
      description: "To list this month's invoices",
      target: { hosts: ["api.example.com"] },
    });
    expect(asked.status).toBe("pending");
    expect(asked.fill_url).toContain("/secrets/requests/");
    expect(asked.handle.$secret).toMatch(/^eu:\/\/[A-Za-z0-9_]+\/e2e_api_key$/);

    // the pop-up appears on whatever page the person is on (polled while the tab is visible)
    await page.goto("/connectors");
    const popup = page.getByTestId("secret-request-popup");
    await expect(popup).toBeVisible({ timeout: 15_000 });
    await expect(popup.getByTestId("secret-request-label")).toHaveText("Example API key");
    await expect(popup).toContainText("To list this month's invoices");
    await expect(popup).toContainText("e2e agent");
    await expect(popup).toContainText("api.example.com");

    // masked, with a client-side Show toggle
    const field = popup.getByTestId("secret-value");
    await expect(field).toHaveAttribute("type", "password");
    await expect(field).toHaveAttribute("autocomplete", "off");
    await field.fill(VALUE);
    await popup.getByRole("button", { name: "Show the secret" }).click();
    await expect(field).toHaveAttribute("type", "text");
    await popup.getByRole("button", { name: "Hide the secret" }).click();
    await expect(field).toHaveAttribute("type", "password");
    await popup.getByRole("button", { name: "Save" }).click();
    await expect(popup).toBeHidden();
    await expect(page.getByRole("status").filter({ hasText: "Saved" })).toBeVisible();

    // the agent learns it is filled and gets the handle, never the value
    const status = await agent(request, token, "secret_request_status", { request_id: asked.request_id });
    expect(status.status).toBe("filled");
    expect(status.handle.$secret).toBe(asked.handle.$secret);
    // the value is nowhere the browser can read either
    await page.reload();
    await expect(page.getByTestId("held-secret-row").filter({ hasText: "e2e_api_key" })).toBeVisible();
    expect(await page.content()).not.toContain(VALUE);
    for (const path of ["/api/secrets/held", "/api/secrets/requests", `/api/secrets/requests/${asked.request_id}`]) {
      expect(await (await page.request.get(path)).text()).not.toContain(VALUE);
    }

    // a second request, declined
    const second = await agent(request, token, "secret_request", { name: "e2e_declined", label: "Another key", description: "Not needed", target: { hosts: ["api.example.com"] } });
    await expect(popup).toBeVisible({ timeout: 15_000 });
    await expect(popup.getByTestId("secret-request-label")).toHaveText("Another key");
    await popup.getByRole("button", { name: "Decline" }).click();
    await expect(popup).toBeHidden();
    expect((await agent(request, token, "secret_request_status", { request_id: second.request_id })).status).toBe("declined");
  });

  test("the standalone page fills a request, and nobody else's", async ({ page, request }) => {
    await registerAndOnboard(page, uniqueEmail("secret-page"));
    const token = await agentToken(page);
    const asked = await agent(request, token, "secret_request", { name: "e2e_page_key", label: "Page key", description: "From the link", target: { hosts: ["api.example.com"] } });
    const path = new URL(asked.fill_url).pathname;
    await page.goto(path);
    await expect(page.getByTestId("secret-request-popup")).toBeHidden(); // the page, not the modal
    const form = page.getByTestId("secret-request-form");
    await form.getByTestId("secret-value").fill(VALUE);
    await form.getByRole("button", { name: "Remove api.example.com" }).click();
    await form.getByLabel("Add a host").fill("api.example.org");
    await form.getByRole("button", { name: "Add", exact: true }).click();
    await form.getByRole("button", { name: "Save" }).click();
    await expect(page.getByRole("status").filter({ hasText: "Saved" })).toBeVisible();
    const held = await (await page.request.get("/api/secrets/held")).json();
    const row = held.find((s: { name: string }) => s.name === "e2e_page_key");
    expect(row.allowed_hosts).toEqual(["api.example.org"]);
    expect(JSON.stringify(held)).not.toContain(VALUE);

    // another person opening the same link sees no request
    await page.context().clearCookies();
    await registerAndOnboard(page, uniqueEmail("secret-other"));
    await page.goto(path);
    await expect(page.getByText("No such request for you")).toBeVisible();
  });
});

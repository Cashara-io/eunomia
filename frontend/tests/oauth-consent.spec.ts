import { expect, test } from "@playwright/test";
import { createHash } from "node:crypto";
import { z } from "zod";
import { registerAndOnboard, uniqueEmail } from "./helpers";

test("OAuth consent explains read and write access to every member space", async ({ page }) => {
  await registerAndOnboard(page, uniqueEmail("oauth-consent"));
  const origin = new URL(page.url()).origin;
  const redirect = "https://oauth-fixture.example.test/callback";
  const registered = await page.request.post("/oauth/register", {
    data: { client_name: "Consent fixture", redirect_uris: [redirect], token_endpoint_auth_method: "none" },
  });
  expect(registered.status()).toBe(201);
  const client = z.object({ client_id: z.string() }).parse(await registered.json());
  const params = new URLSearchParams({
    response_type: "code", client_id: client.client_id, redirect_uri: redirect,
    code_challenge: createHash("sha256").update("fixture-verifier-".repeat(4)).digest("base64url"),
    code_challenge_method: "S256", resource: `${origin}/mcp`, state: "fixture-consent",
  });
  await page.goto(`/consent?${params}`);
  await expect(page.getByRole("heading", { name: "Allow Consent fixture to use Eunomia?" })).toBeVisible();
  await expect(page.getByText("Read memories in every space you belong to", { exact: true })).toBeVisible();
  await expect(page.getByText("Save memories in every space you belong to", { exact: true })).toBeVisible();
  await expect(page.getByText("These permissions apply to every space you belong to, including your Personal space and team spaces.", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Allow", exact: true })).toBeVisible();
});

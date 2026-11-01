import { expect, test, type Page } from "@playwright/test";
import http from "node:http";
import type { AddressInfo } from "node:net";
import { createCipheriv, createHmac, hkdfSync, randomBytes } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { MOCK_BIND, mockBase, registerAndOnboard, uniqueEmail } from "./helpers";

// Settings, Connectors, Password managers (docs/password-managers.md) against stand-ins for a customer's
// 1Password Connect server and for Bitwarden's identity and API servers. The browser talks to the
// backend, the backend to the real secrets-helper container, and the sidecar to these mocks (in CI via
// E2E_MOCK_HOST=host.docker.internal; docker-compose.ci.yml gives the sidecar the host gateway).
// Setting up a password manager is for org owners and instance admins: the spec signs in as the install's
// first user (E2E_ADMIN_EMAIL / E2E_ADMIN_PASSWORD); a fresh user plays a member, who owns their personal
// vault and so manages that vault's references only (decided by Avi on 2026-11-01). The secrets helper
// runs under the `secrets` compose profile (CI sets it); E2E_UPDATE_STATUS_DIR, when set, lets the spec
// check the switch's request file the way the HTTPS spec plays the updater.
const admin = { email: process.env.E2E_ADMIN_EMAIL, password: process.env.E2E_ADMIN_PASSWORD };
const statusDir = process.env.E2E_UPDATE_STATUS_DIR;
const CONNECT_TOKEN = `connect-e2e-${randomBytes(6).toString("hex")}`;

async function listen(server: http.Server): Promise<{ base: string; close: () => void }> {
  return new Promise((resolve) =>
    server.listen(0, MOCK_BIND, () => resolve({ base: mockBase((server.address() as AddressInfo).port), close: () => server.close() })),
  );
}

function mockConnect() {
  return listen(
    http.createServer((req, res) => {
      res.setHeader("content-type", "application/json");
      if (req.headers.authorization !== `Bearer ${CONNECT_TOKEN}`) {
        res.statusCode = 401;
        return res.end(JSON.stringify({ status: 401, message: "Invalid token signature" }));
      }
      if (req.url?.startsWith("/v1/vaults")) return res.end(JSON.stringify([{ id: "vlt0ops", name: "Ops" }, { id: "vlt1dev", name: "Dev" }]));
      res.statusCode = 404;
      res.end("{}");
    }),
  );
}

// Bitwarden's client-side crypto, as the sidecar expects it (secrets-helper/src/providers/bitwarden_crypto.ts)
const BW_ID = "6b2f0a8e-1c3d-4e5f-8a9b-0c1d2e3f4a5b";
const BW_SECRET = "e2eClientSecretNotReal0123456789";
const BW_SEED = randomBytes(16);
const BW_TOKEN = `0.${BW_ID}.${BW_SECRET}:${BW_SEED.toString("base64")}`;
function encString(plain: string, key: Buffer) {
  const iv = randomBytes(16);
  const c = createCipheriv("aes-256-cbc", key.subarray(0, 32), iv);
  const data = Buffer.concat([c.update(plain), c.final()]);
  const mac = createHmac("sha256", key.subarray(32)).update(Buffer.concat([iv, data])).digest();
  return `2.${iv.toString("base64")}|${data.toString("base64")}|${mac.toString("base64")}`;
}
function mockBitwarden() {
  const tokenKey = Buffer.from(hkdfSync("sha256", BW_SEED, "bitwarden-accesstoken", "sm-access-token", 64));
  const orgKey = randomBytes(64);
  return listen(
    http.createServer((req, res) => {
      let body = "";
      req.on("data", (c) => (body += c));
      req.on("end", () => {
        res.setHeader("content-type", "application/json");
        const form = new URLSearchParams(body);
        if (req.method === "POST" && req.url === "/identity/connect/token" && form.get("client_id") === BW_ID && form.get("client_secret") === BW_SECRET) {
          return res.end(JSON.stringify({ access_token: "bearer-e2e", encrypted_payload: encString(JSON.stringify({ encryptionKey: orgKey.toString("base64") }), tokenKey) }));
        }
        res.statusCode = 400;
        res.end(JSON.stringify({ error: "invalid_client" }));
      });
    }),
  );
}

async function signInAsAdmin(page: Page) {
  await page.goto("/login");
  await page.getByLabel("Email").fill(admin.email!);
  await page.getByLabel("Password", { exact: true }).fill(admin.password!);
  await page.getByRole("button", { name: "Sign in" }).click();
  await page.waitForURL((url) => url.pathname === "/");
}

test("a member manages their own vault's references but cannot set up a provider or switch the helper", async ({ page }) => {
  await registerAndOnboard(page, uniqueEmail("pm-member"));
  await page.goto("/connectors");
  await expect(page.getByRole("heading", { name: "Password managers" })).toBeVisible();
  await expect(page.getByTestId("vault-owner-note")).toContainText("Only an owner of this organisation or an instance admin can set up a password manager.");
  // provider rows are shown, read-only
  const bw = page.getByTestId("pm-bitwarden_sm");
  await expect(bw).toBeVisible();
  await expect(bw.getByRole("button", { name: "Set up" })).toHaveCount(0);
  await expect(page.getByTestId("secrets-helper").getByRole("button")).toHaveCount(0);
  // their personal vault: add and remove a reference
  const ref = `op://Ops/Member-${Date.now()}/key`;
  await page.getByRole("button", { name: "Add reference" }).click();
  await page.getByLabel("Reference", { exact: true }).fill(ref);
  await page.getByLabel(/Allowed tools/).fill("*");
  await page.getByRole("button", { name: "Add reference" }).last().click();
  const row = page.getByTestId("secret-ref-row").filter({ hasText: ref });
  await expect(row).toBeVisible();
  await row.getByRole("button", { name: `Remove ${ref}` }).click();
  await row.getByRole("button", { name: "Remove" }).click();
  await expect(page.getByTestId("secret-ref-row").filter({ hasText: ref })).toHaveCount(0);
});

test("Bitwarden Secrets Manager is marked Preview, with its note; the 1Password kinds are not", async ({ page }) => {
  await registerAndOnboard(page, uniqueEmail("pm-preview"));
  await page.goto("/connectors");
  const bw = page.getByTestId("pm-bitwarden_sm");
  await expect(bw.getByTestId("preview-badge")).toHaveText("Preview");
  await expect(bw.getByTestId("preview-note")).toContainText("Preview: Eunomia's own Bitwarden client is tested against a mock");
  for (const k of ["onepassword_connect", "onepassword_service_account"]) {
    await expect(page.getByTestId(`pm-${k}`).getByTestId("preview-badge")).toHaveCount(0);
  }
  // the same from the API, the one source of truth
  const kinds = (await (await page.request.get("/api/secrets/kinds")).json()) as { kind: string; preview: boolean; stability: string }[];
  expect(kinds.find((k) => k.kind === "bitwarden_sm")).toMatchObject({ preview: true, stability: "preview" });
});

test.describe("as the instance admin", () => {
  test.skip(!admin.email || !admin.password, "set E2E_ADMIN_EMAIL and E2E_ADMIN_PASSWORD to the first user's login");
  test.describe.configure({ mode: "serial" });

  test("the secrets helper: running, and Disable asks the updater (an admin only)", async ({ page }) => {
    await signInAsAdmin(page);
    await page.goto("/connectors");
    const helper = page.getByTestId("secrets-helper");
    await expect(helper.getByRole("status")).toHaveText("The secrets helper is running.");
    test.skip(!statusDir, "set E2E_UPDATE_STATUS_DIR to check the request file");
    const req = path.join(statusDir!, "secrets.json");
    fs.rmSync(req, { force: true });
    await helper.getByRole("button", { name: "Disable password managers" }).click();
    await helper.getByRole("button", { name: "Disable", exact: true }).click();
    await expect(helper).toContainText("Switching off: waiting for the updater");
    expect(JSON.parse(fs.readFileSync(req, "utf8"))).toEqual({ enabled: false });
    // no updater in this stack: take the request back, and the helper reads as running again
    fs.rmSync(req, { force: true });
    await page.reload();
    await expect(helper.getByRole("status")).toHaveText("The secrets helper is running.");
  });

  test("1Password Connect: a write-only token, Test connection through the sidecar, Replace and Remove", async ({ page }) => {
    const connect = await mockConnect();
    try {
      await signInAsAdmin(page);
      await page.goto("/connectors");
      const row = page.getByTestId("pm-onepassword_connect");
      await expect(row).toContainText("Not set up");
      await row.getByRole("button", { name: "Set up" }).click();
      await row.getByLabel("Connect server URL").fill(connect.base);
      const token = row.getByLabel("Token");
      await expect(token).toHaveAttribute("type", "password");
      await token.fill(CONNECT_TOKEN);
      await row.getByRole("button", { name: "Save" }).click();
      await expect(row).toContainText("Configured");
      // the token is never read back: not on the page, not in the API's answer
      await page.reload();
      await expect(row).toContainText("Configured");
      expect(await page.content()).not.toContain(CONNECT_TOKEN);
      const listed = await page.request.get("/api/secrets/providers");
      expect(await listed.text()).not.toContain(CONNECT_TOKEN);

      await row.getByRole("button", { name: "Test connection" }).click();
      await expect(row.getByRole("status")).toHaveText("Connected: 2 vaults visible.");

      // Replace with a wrong token: the test now fails with the provider's refusal, and no token shows
      await row.getByRole("button", { name: "Replace" }).click();
      await row.getByLabel("New token (leave empty to keep the stored one)").fill("connect-e2e-wrong");
      await row.getByRole("button", { name: "Save" }).click();
      await row.getByRole("button", { name: "Test connection" }).click();
      await expect(row.getByText(/refused the token/)).toBeVisible();
      expect(await page.content()).not.toContain("connect-e2e-wrong");

      await row.getByRole("button", { name: "Remove 1Password Connect" }).click();
      await row.getByRole("button", { name: "Remove" }).click();
      await expect(row).toContainText("Not set up");
    } finally {
      connect.close();
    }
  });

  test("Bitwarden Secrets Manager: Test connection signs in through the sidecar", async ({ page }) => {
    const bw = await mockBitwarden();
    try {
      await signInAsAdmin(page);
      await page.goto("/connectors");
      const row = page.getByTestId("pm-bitwarden_sm");
      await row.getByRole("button", { name: "Set up" }).click();
      await row.getByLabel("Server URL (optional)").fill(bw.base);
      await row.getByLabel("Token").fill(BW_TOKEN);
      await row.getByRole("button", { name: "Save" }).click();
      await row.getByRole("button", { name: "Test connection" }).click();
      await expect(row.getByRole("status")).toHaveText("Connected: signed in as the machine account.");
      await row.getByRole("button", { name: "Remove Bitwarden Secrets Manager" }).click();
      await row.getByRole("button", { name: "Remove" }).click();
      await expect(row).toContainText("Not set up");
    } finally {
      bw.close();
    }
  });

  test("references: add, edit, refuse a malformed one, remove; the log starts empty", async ({ page }) => {
    await signInAsAdmin(page);
    await page.goto("/connectors");
    const ref = `op://Ops/Square-${Date.now()}/access_token`;
    await page.getByRole("button", { name: "Add reference" }).click();
    await page.getByLabel("Reference", { exact: true }).fill("op://only/two");
    await page.getByLabel(/Allowed tools/).fill("*");
    await page.getByRole("button", { name: "Add reference" }).last().click();
    await expect(page.getByText(/op:\/\/ takes vault\/item\/field/)).toBeVisible();

    await page.getByLabel("Reference", { exact: true }).fill(ref);
    await page.getByRole("button", { name: "Add reference" }).last().click();
    const row = page.getByTestId("secret-ref-row").filter({ hasText: ref });
    await expect(row).toBeVisible();
    await expect(row).toContainText("1Password Connect");

    await row.getByRole("button", { name: `Edit ${ref}` }).click();
    await page.getByLabel(/Allowed tools/).fill("square_charge, square_refund");
    await page.getByRole("button", { name: "Save reference" }).click();
    await expect(row).toContainText("square_charge, square_refund");

    await expect(page.getByRole("heading", { name: "Resolution log" })).toBeVisible();
    await row.getByRole("button", { name: `Remove ${ref}` }).click();
    await row.getByRole("button", { name: "Remove" }).click();
    await expect(page.getByTestId("secret-ref-row").filter({ hasText: ref })).toHaveCount(0);
  });
});

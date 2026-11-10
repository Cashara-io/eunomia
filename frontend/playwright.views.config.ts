import { defineConfig } from "@playwright/test";

// Fixed, isolated ports. Never reuse an existing stack for these mocked tests.
const baseURL = "http://127.0.0.1:19196";
export default defineConfig({
  testDir: "./tests",
  testMatch: ["views-scale.spec.ts", "scenario-card-format.spec.ts", "scenario-format.spec.ts"],
  fullyParallel: false,
  retries: 0,
  reporter: "list",
  use: { baseURL, channel: "chrome" },
  webServer: {
    command: "bun scripts/views-fixture.mjs",
    url: `${baseURL}/healthz`,
    reuseExistingServer: false,
    timeout: 60_000,
  },
});

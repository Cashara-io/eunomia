import { defineConfig } from "@playwright/test";

if (!process.env.E2E_BASE_URL)
  throw new Error("Set E2E_BASE_URL to your isolated frontend server.");
export default defineConfig({
  testDir: "./tests",
  testMatch: ["ontology-review.spec.ts", "ontology-review-regressions.spec.ts"],
  fullyParallel: false,
  workers: 1,
  retries: 0,
  reporter: "list",
  use: {
    baseURL: process.env.E2E_BASE_URL,
    channel: "chrome",
    trace: "retain-on-failure",
  },
});

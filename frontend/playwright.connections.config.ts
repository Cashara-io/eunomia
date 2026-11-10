import { defineConfig } from "@playwright/test";
if (!process.env.E2E_BASE_URL) throw new Error("Set an isolated E2E_BASE_URL");
export default defineConfig({
  testDir: "./tests",
  testMatch: [
    "ontology-connections-ui.spec.ts",
    "ontology-connections-review.spec.ts",
    "ontology-connections-access.spec.ts",
    "ontology-connections-performance.spec.ts",
  ],
  workers: 1,
  retries: 0,
  reporter: "list",
  use: {
    baseURL: process.env.E2E_BASE_URL,
    channel: "chrome",
    trace: "retain-on-failure",
  },
});

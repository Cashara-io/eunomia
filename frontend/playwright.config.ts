import { defineConfig } from "@playwright/test";

// E2E_BASE_URL points the suite at an already-running stack (e.g. the
// docker-compose one) instead of starting `bun run dev`.
const baseURL = process.env.E2E_BASE_URL || "http://localhost:3000";

// Uses the system-installed Chrome (no browser download needed).
// These tests exercise the real API, not a mock: the FastAPI backend
// (and SurrealDB) must already be running on :8001 before `bun run test` --
// `docker-compose up` the backend+surrealdb first. Playwright's `webServer`
// only starts the frontend dev server below; starting the whole compose
// stack from here would make failures much harder to diagnose (slow first
// boot, DB not ready yet) than just running it yourself once beforehand.
export default defineConfig({
  testDir: "./tests",
  // Run the mocked review regressions alongside the real API specs in CI.
  testMatch: "**/*.spec.ts",
  fullyParallel: false,
  retries: 0,
  reporter: "list",
  use: {
    baseURL,
    channel: "chrome",
    // CI uploads test-results/ on failure; a trace makes a failure debuggable
    trace: process.env.CI ? "retain-on-failure" : "off",
  },
  webServer: process.env.E2E_BASE_URL
    ? undefined
    : {
        command: "bun run dev",
        url: baseURL,
        reuseExistingServer: true,
        timeout: 30_000,
      },
});

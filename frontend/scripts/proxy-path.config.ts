import { defineConfig } from "@playwright/test";
import { resolve } from "node:path";

const port = process.env.PROXY_TEST_REAL_BACKEND_URL ? 28192 : 28190;

export default defineConfig({
  testDir: ".",
  testMatch: process.env.PROXY_TEST_REAL_BACKEND_URL ? "proxy-real.spec.ts" : "proxy-path.spec.ts",
  workers: 1,
  use: { baseURL: `http://127.0.0.1:${port}` },
  webServer: {
    cwd: resolve(__dirname, ".."),
    command: `bun run ${process.env.PROXY_TEST_PRODUCTION ? "start" : "dev"} --hostname 127.0.0.1 --port ${port}`,
    url: `http://127.0.0.1:${port}/login`,
    env: {
      // Only the optional real-backend fixture needs shortened timeout probes.
      // The isolated Next smoke tests use the production upload policy.
      ...(process.env.PROXY_TEST_REAL_BACKEND_URL ? { EUNOMIA_UPLOAD_GRACE_MS: "200", EUNOMIA_UPLOAD_WINDOW_MS: "200" } : {}),
      BACKEND_INTERNAL_URL: process.env.PROXY_TEST_REAL_BACKEND_URL || "http://127.0.0.1:28191",
    },
    timeout: 60_000,
  },
});

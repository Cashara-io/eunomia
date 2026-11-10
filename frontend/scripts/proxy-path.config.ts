import { defineConfig } from "@playwright/test";
import { resolve } from "node:path";

const port = process.env.PROXY_TEST_REAL_BACKEND_URL ? 18192 : 18190;

export default defineConfig({
  testDir: ".",
  testMatch: process.env.PROXY_TEST_REAL_BACKEND_URL ? "proxy-real.spec.ts" : "proxy-path.spec.ts",
  workers: 1,
  use: { baseURL: `http://127.0.0.1:${port}` },
  webServer: {
    cwd: resolve(__dirname, ".."),
    command: `bun run ${process.env.PROXY_TEST_PRODUCTION ? "start" : "dev"} --hostname 127.0.0.1 --port ${port}`,
    url: `http://127.0.0.1:${port}/login`,
    env: { EUNOMIA_UPLOAD_GRACE_MS: "200", EUNOMIA_UPLOAD_WINDOW_MS: "200", BACKEND_INTERNAL_URL: process.env.PROXY_TEST_REAL_BACKEND_URL || "http://127.0.0.1:18191" },
    timeout: 60_000,
  },
});

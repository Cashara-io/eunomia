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
      // Socket timeout probes use seconds of margin, without pacing the sender.
      EUNOMIA_UPLOAD_GRACE_MS: "3000",
      EUNOMIA_UPLOAD_WINDOW_MS: "5000",
      BACKEND_INTERNAL_URL: process.env.PROXY_TEST_REAL_BACKEND_URL || "http://127.0.0.1:28191",
    },
    timeout: 60_000,
  },
});

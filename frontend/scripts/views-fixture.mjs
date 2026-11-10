// A disposable UI-only fixture. Unknown API requests fail instead of reaching a database.
import { serve, spawn } from "bun";
const backend = serve({
  hostname: "127.0.0.1",
  port: 19197,
  fetch(request) {
    const health = new URL(request.url).pathname === "/healthz";
    return new Response(health ? "Views fixture" : "No backend in this fixture", {
      status: health ? 200 : 503,
      headers: { "X-Eunomia-Fixture": "views-196" },
    });
  },
});
const frontend = spawn(["bun", "run", "dev", "--hostname", "127.0.0.1", "--port", "19196"], {
  env: { ...process.env, BACKEND_INTERNAL_URL: "http://127.0.0.1:19197", NEXT_PUBLIC_API_URL: "" },
  stdout: "inherit",
  stderr: "inherit",
});
for (const signal of ["SIGTERM", "SIGINT"]) process.on(signal, () => {
  frontend.kill(signal);
  backend.stop(true);
});
const result = await frontend.exited;
backend.stop(true);
process.exit(result);

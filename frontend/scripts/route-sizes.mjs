// Run after next build. Includes the synchronous JS referenced by each route and root bootstrap.
import { readFileSync, statSync } from "node:fs";
import { runInNewContext } from "node:vm";
import { gzipSync } from "node:zlib";
const routes = [
  "",
  "entities",
  "vaults",
  "documents",
  "connectors",
  "ontology",
  "ontology/schema",
  "ontology/review",
  "chat",
  "settings",
  "login",
];
const result = {};
for (const route of routes) {
  const name =
    route === "login" ? "login/page" : `(app)/${route ? route + "/" : ""}page`;
  const ctx = { globalThis: {} };
  runInNewContext(
    readFileSync(
      `.next/server/app/${name}_client-reference-manifest.js`,
      "utf8",
    ),
    ctx,
  );
  const manifest = Object.values(ctx.globalThis.__RSC_MANIFEST)[0];
  const build = JSON.parse(
    readFileSync(`.next/server/app/${name}/build-manifest.json`, "utf8"),
  );
  const files = new Set(build.rootMainFiles);
  for (const mod of Object.values(manifest.clientModules)) {
    if (!mod.async)
      for (const chunk of mod.chunks)
        if (chunk.endsWith(".js")) files.add(chunk.replace(/^\/_next\//, ""));
  }
  result["/" + route] = {
    bytes: [...files].reduce((n, f) => n + statSync(".next/" + f).size, 0),
    gzip: [...files].reduce(
      (n, f) => n + gzipSync(readFileSync(".next/" + f)).length,
      0,
    ),
  };
}
console.log(JSON.stringify(result, null, 2));

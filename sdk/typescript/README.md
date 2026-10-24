# @eunomia/sdk (TypeScript)

Client for the Eunomia REST API. ESM, no runtime dependencies, uses the global `fetch` (Node 18+, Bun, Deno, browsers).

```ts
import { Eunomia } from "@eunomia/sdk";

const eu = new Eunomia("http://localhost:3000", { token: process.env.EUNOMIA_TOKEN! });
const a = await eu.createVault("Research");
const b = await eu.createVault("Notes");
await eu.memoryWrite({ subjectName: "Ada", subjectKind: "person", text: "Ada prefers tea", vaultId: a.id });
await eu.memoryWrite({ subjectName: "Ada", subjectKind: "person", text: "Ada lives in Perth", vaultId: b.id });
const merged = await eu.mergeVaults(a.id, b.id, { name: "Everything" });
console.log(await eu.recall("where does Ada live", { vaultId: merged.id }));
```

See [../README.md](../README.md) for the full method list.

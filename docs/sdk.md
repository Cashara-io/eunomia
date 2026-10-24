# Python and TypeScript SDKs

Use an SDK to manage vaults and memories from a script, with a personal API token. The same operations are in the app's Vaults page and the vault tools in [AI agents & MCP](agents.md#tools). The SDKs call the REST API behind them.

The code is in [`sdk/`](../sdk/README.md): `eunomia` for Python 3.9+ and `@eunomia/sdk` for TypeScript. Neither has runtime dependencies. They are not on PyPI or npm yet, so install from a checkout.

## Get a token

Create a token in Settings → API tokens, or sign in over REST and `POST /api/auth/tokens`. It is shown once. A token can be limited to one vault or to some scopes (`memory:read`, `memory:write`, `vaults:admin`).

## Python

```python
from eunomia import Eunomia

eu = Eunomia("http://localhost:3000", token="<token>")

a = eu.create_vault("Research")
b = eu.create_vault("Notes")
eu.memory_write("Ada", "person", "Ada prefers green tea", vault_id=a.id)
eu.memory_write("Grace", "person", "Grace wrote the first compiler", vault_id=b.id)

merged = eu.merge_vaults(a.id, b.id, name="Everything")
for hit in eu.recall("who wrote a compiler", vault_id=merged.id):
    print(hit["text"])
```

## TypeScript

```ts
import { Eunomia } from "@eunomia/sdk";

const eu = new Eunomia("http://localhost:3000", { token: process.env.EUNOMIA_TOKEN! });

const a = await eu.createVault("Research");
const b = await eu.createVault("Notes");
await eu.memoryWrite({ subjectName: "Ada", subjectKind: "person", text: "Ada prefers green tea", vaultId: a.id });
await eu.memoryWrite({ subjectName: "Grace", subjectKind: "person", text: "Grace wrote the first compiler", vaultId: b.id });

const merged = await eu.mergeVaults(a.id, b.id, { name: "Everything" });
for (const hit of await eu.recall("who wrote a compiler", { vaultId: merged.id })) console.log(hit.text);
```

Merging copies both vaults into a new one and leaves the originals alone. Matching entities (same kind and name) are combined. See [concepts](concepts.md#vaults).

## Importing many facts

```python
items = [{"subject_name": p.title, "subject_kind": "document", "text": p.summary} for p in pages]
results = eu.write_memories(items, vault_id=vault.id, concurrency=4)
failed = [r for r in results if not r.ok]
```

The server rate-limits each token and replies `429` with `Retry-After`. The SDK waits and retries, and all workers pause together. Items that still fail come back with their error instead of stopping the run.

## Errors

Failures raise `EunomiaError` with `status`, `code`, `detail` and `trace_id`, the fields of the API's problem+json body. See [errors](errors.md).

The full method list, with the endpoint each one calls, is in [`sdk/README.md`](../sdk/README.md).

# Eunomia SDKs

Two small clients for the Eunomia REST API. Use them to manage vaults and to read and write memories from a script. Neither has runtime dependencies.

| SDK | Package | Needs |
|---|---|---|
| [Python](python/) | `eunomia` | Python 3.9+ |
| [TypeScript](typescript/) | `@eunomia/sdk` | Node 18+ (or anything with `fetch`) |

They are not published to a registry yet. Install from a checkout: `pip install ./sdk/python`, or `npm install ./sdk/typescript` (run `npm run build` in it first).

## Authenticate

Create a personal API token in the app (Settings → API tokens), or over REST: log in, then `POST /api/auth/tokens {"name": "my-script"}`. The token is shown once. Pass it as the `token`; the SDK sends it as `Authorization: Bearer <token>`. Point the client at the server root, for example `http://localhost:3000`.

## Methods

Both SDKs have the same methods (Python `snake_case`, TypeScript `camelCase`).

| Method | Calls |
|---|---|
| `list_vaults` | `GET /api/vaults` |
| `get_vault(id_or_name)` | `GET /api/vaults`, filtered (there is no single-vault route) |
| `create_vault(name, kind="org")` | `POST /api/vaults` |
| `rename_vault(id, name)` | `PATCH /api/vaults/{id}` |
| `delete_vault(id)` | `DELETE /api/vaults/{id}` |
| `clone_vault(id, name?, kind)` | `POST /api/vaults/{id}/clone` |
| `merge_vaults(a, b, name?, kind)` | `POST /api/vaults/merge` |
| `list_members(id)` | `GET /api/vaults/{id}/members` |
| `invite_member(id, email, role="member")` | `POST /api/vaults/{id}/members` |
| `remove_member(id, email)` | `DELETE /api/vaults/{id}/members/{email}` |
| `leave_vault(id)` | `POST /api/vaults/{id}/leave` |
| `list_invitations` / `accept_invitation(id)` / `decline_invitation(id)` | `GET /api/vaults/invitations`, `POST /api/vaults/{id}/invitations/accept` and `/decline` |
| `memory_write(...)` | `POST /api/tools/memory_write` |
| `recall(query, vault_id?)` | `POST /api/tools/recall` |
| `entities_search(query, vault_id?)` | `POST /api/tools/entities_search` |
| `write_memories(items, concurrency=4)` | many `memory_write` calls, see below |
| `call_tool(name, args)` | `POST /api/tools/{name}` for any other tool |

`vault_id` accepts a vault id or its name. Merging and cloning make a new vault and never change the originals.

## Errors

Every failed call raises `EunomiaError` (Python) or throws `EunomiaError` (TypeScript), with the API's `status`, `code`, `detail` and `trace_id` (`traceId` in TypeScript). Codes are listed in [docs/errors.md](../docs/errors.md). Quote the trace id when you report a problem. A network failure has status `0` and code `client.transport`.

## Bulk writes and rate limits

The server limits requests per token and answers `429` with a `Retry-After` header. The clients wait that long and retry (5 times by default), and one request waiting pauses the rest of the client, so a pool of workers backs off together.

`write_memories` / `writeMemories` writes a list of facts with a small worker pool. A failed item does not stop the run: you get one result per item, in input order, each with `ok` and either the result or the error. Use it to import hundreds of pages.

## Tests

```sh
cd sdk/python && python -m unittest discover -s tests
cd sdk/typescript && npm install && npm test
```

Each runs against a local stub server. Set `EUNOMIA_URL` and `EUNOMIA_TOKEN` to also run a live test that creates two vaults, merges them, recalls from the result and deletes what it made.

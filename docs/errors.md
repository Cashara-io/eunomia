# Error codes

Every error the backend returns has a stable dotted `code`. On HTTP it is an RFC 9457 `application/problem+json` body:

```json
{ "type": "about:blank", "title": "Not Found", "status": 404, "detail": "Vault not found.", "code": "vault.not_found", "trace_id": "0af7651916cd43dd8448eb211c80319c" }
```

The same `trace_id` is in the `x-trace-id` response header on every response. Tool errors (REST `/api/tools/*` and MCP `tools/call`) are `{ "error", "code", "trace_id" }` values, and MCP protocol errors carry `{ "code", "trace_id" }` in the JSON-RPC error `data`. The browser sends a W3C `traceparent` header and the backend continues that trace.

Raw database or upstream text is never in a response. It is only on the request span (field `source`), found by `trace_id`.

The `ErrorCode` enum is in `backend/src/error.rs`. A unit test fails if a code is missing from this file, so add a row here when you add a variant.

| Code | HTTP | Meaning | Emitted by |
|------|------|---------|------------|
| `auth.unauthorized` | 401 | No valid session cookie or API token, or wrong email or password. | `auth.rs` (extractor), `routers/auth.rs`, `routers/mcp.rs` |
| `auth.forbidden` | 403 | Request rejected before auth, for example a disallowed browser `Origin` on `/mcp`, a non-admin reading `/api/debug/capsules/{trace_id}`, or a signup the server refuses (closed, not on the allow list, or an address reserved in `EUNOMIA_ADMIN_EMAILS`). | `routers/mcp.rs`, `routers/debug.rs`, `routers/auth.rs` |
| `auth.email_taken` | 409 | Signup with an email that already has an account. | `models_user.rs`, `routers/auth.rs` |
| `auth.not_found` | 404 | API token or session id does not exist or is not yours. | `routers/auth.rs` |
| `auth.scope` | 403 | The token lacks the scope this route or tool needs, or is restricted to another vault, or the route is account-level and the token is vault-restricted. | `gate.rs`, `authz.rs`, `tools/registry.rs` |
| `auth.token_expired` | 401 | The API token's `expires_at` has passed. Create a new token. | `gate.rs`, `auth.rs` |
| `auth.session_expired` | 401 | The session cookie is past its (sliding) expiry or its JWT `exp`. Log in again. | `gate.rs`, `auth.rs` |
| `auth.password_disabled` | 403 | `AUTH_MODE=oidc`: email and password sign-in and signup are turned off; users sign in through the OpenID Connect provider (`/api/auth/oidc/start`). | `routers/auth.rs` |
| `rate.limited` | 429 | Too many requests for this user, token or (for login and signup) client address. Also 10 failed logins for one email, or 30 failed token requests for one OAuth client, in 15 minutes, whatever the address (counted per process). Honour the `Retry-After` header. | `gate.rs`, `routers/auth.rs`, `oauth/server.rs` |
| `tenant.denied` | 403 | The database refused a query on permissions grounds, or a statement named a table of the other database (control vs org) and was refused before it was sent. Logged at error level; treat as a high-severity alert. | `error.rs` (DB error mapping), `store/mod.rs` (`CROSSING_MARK`) |
| `tenant.not_found` | 404 | The org has no ready database: its routing row is missing or the database is still being created or moved. | `pool.rs` |
| `tenant.schema_behind` | 503 | The org's database is more than one schema version behind this code (below N-1). The `migrate_tenant` job brings it up; retry shortly. | `pool.rs` |
| `tenant.provisioning_disabled` | 501 | Signup needs to create an org database, and this binary was built without the `provisioning` feature. | `models_user.rs` |
| `vault.not_found` | 404 | Vault id is malformed or unknown. | `routers/vaults.rs` |
| `vault.forbidden` | 403 | You are not a member, or not an owner, of the vault (or of both entities' vaults). Also the default for a bare 403. | `vaults/service.rs`, `entities/service.rs` |
| `vault.name_taken` | 409 | Creating or renaming a vault to a name you already have on another vault (compared case-insensitively, among the vaults you belong to). | `vaults/service.rs` |
| `entity.not_found` | 404 | Entity id unknown or not visible to you. | `routers/entities.rs`, `tools/registry.rs` |
| `memory.not_found` | 404 | Memory id unknown or not visible to you. | `routers/entities.rs`, `tools/registry.rs` |
| `chat.thread_not_found` | 404 | Chat thread id unknown or not yours. | `routers/chat.rs` |
| `connector.not_found` | 404 | Unknown connector kind. | `routers/connectors.rs` |
| `connector.not_connected` | 400 | The action needs a connector that has not been connected. | `routers/connectors.rs` |
| `source.not_found` | 404 | Unknown source key. | `routers/sources.rs`, `sources/registry.rs` |
| `extension.not_found` | 404 | No such extension of yours (a guessed id, another user's, or one already deleted). | `routers/extensions.rs` |
| `extension.disabled` | 403 | The delivery was authenticated, but the extension is switched off. | `routers/extensions.rs` |
| `extension.buffer_full` | 429 | The extension's inbox holds the maximum of waiting and leased events (10,000 by default); the sender should retry later. Not a rate limit: it clears as the consumer acks. | `routers/extensions.rs` |
| `tool.not_found` | 404 | Unknown tool name (REST, MCP and registry). | `routers/tools.rs`, `routers/mcp.rs`, `tools/registry.rs` |
| `resource.not_found` | 404 | Generic not found, also the default for a bare 404. | `error.rs`, `routers/entities.rs` (bad id), various |
| `secret.unavailable` | 503 | A `{"$secret": "<reference>"}` in the tool arguments could not be used, so the tool did not run. The message names the reference and why: denied (not registered, not allowed for this tool, credential or vault, malformed, or the tool takes no secrets), not found in the password manager, or a provider error (none configured, the secrets helper down or slow, the provider refused the token). Never a fallback to a stored value; every attempt is in the resolution log (`secret.resolve`). | `secrets/broker.rs`, `tools/registry/mod.rs` |
| `document.not_found` | 404 | No such document of yours (a guessed id, another user's or org's, or one already deleted). | `documents/` |
| `document.too_large` | 413 | The upload is larger than `EUNOMIA_DOCUMENTS_MAX_BYTES` (default 25 MiB), or than the transport allows. | `documents/` |
| `document.unsupported_type` | 415 | The upload is not one of the supported types (plain text, Markdown, JSON, CSV, text-based PDF). | `documents/` |
| `document.storage_unavailable` | 503 | Document storage is off (`EUNOMIA_DOCUMENTS_BACKEND` unset) or the database's file bucket cannot be reached (server without `--allow-experimental=files`, backend down). See [documents.md](documents.md). | `documents/` |
| `validation.invalid` | 400 | The request or tool arguments are invalid. Also the default for a bare 400, for MCP protocol errors and for tool errors with no more specific code. | most routers, `tools/registry.rs`, `routers/mcp.rs` |
| `db.conflict` | 409 | A transaction conflicted with another write. Retryable: repeat the request. Also the default for a bare 409. | `error.rs` (DB error mapping) |
| `db.duplicate` | 409 | A unique index rejected the write. Not retryable. | `error.rs` (DB error mapping), `routers/auth.rs` |
| `job.no_handler` | n/a | A job of a kind no handler is registered for was claimed. Dead-lettered, never sent over HTTP (stored in `job.last_error_code`). | `jobs/worker.rs` |
| `job.lease_expired` | n/a | A job was claimed `max_attempts` times and each lease expired (worker crash loop). Dead-lettered. Stored in `job.last_error_code`. | `jobs/leader.rs` |
| `job.panicked` | n/a | A job handler panicked. Retried with backoff, dead after `max_attempts`. Stored in `job.last_error_code`. | `jobs/worker.rs` |
| `internal` | 500 | Anything else. The detail is always the generic "Internal server error."; use the `trace_id` to find the cause. The one exception is `GET /readyz`, which answers 503 with this code and a plain detail ("The control database did not answer.", or the migration versions that differ), because a probe has no trace to follow. | everywhere via `AppError::internal` and unmapped DB errors; `lib.rs` (`readyz`) |

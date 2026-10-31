# Credential broker (lane L3): spec

Status: draft for Avi, written before the code (plan section 6, decision D5). PRs: B1 `feat/broker-core`,
B2 `feat/broker-sidecar`, B3 `feat/broker-ui`, stacked on R `feat/redact-208` (#111 and #115 carried
onto 2.0.8 and the Projects change log).

## Goal

Agents use secrets held in the customer's 1Password or Bitwarden Secrets Manager without Eunomia ever
storing the plaintext. Eunomia stores **references**; the broker resolves a reference at tool-call time,
hands the value to that one tool execution, scrubs it from everything the call leaves behind, and audits
that it did.

Providers in this release:

| Kind | What the admin enters | How the sidecar reaches it |
|---|---|---|
| `onepassword_connect` | Connect server URL and a Connect token | the Connect REST API on the customer's server |
| `onepassword_service_account` | a service account token (`ops_...`) | the official `@1password/sdk` (MIT) |
| `bitwarden_sm` | a machine account access token, optional server URL (default the US cloud) | Bitwarden's public identity and API endpoints, with our own small client (see Licence) |

## Non-goals

- Eunomia as a secret store. There is no "save a value" path, and no fallback to a stored value.
- Writing to a provider (create or rotate a secret).
- Resolving references inside connector settings at sync time. B1 parses `{"$secret": ...}` there with
  the same parser and rejects nothing that works today, but connector syncs run in jobs with no tool call
  and their own credential path (`connectors/crypto.rs`); wiring them is a follow-up PR (open question 3).
- A resolution cache. Off by default per the plan; B1 ships none (each call resolves), so there is no
  switch to get wrong. `SECRETS_CACHE_SECONDS` is reserved.
- Provider SDKs in the backend (D5). The Rust side only speaks the sidecar API below.

## References

A reference is a string. Two schemes:

- `op://<vault>/<item>/<field>` and `op://<vault>/<item>/<section>/<field>`: 1Password's own secret
  reference syntax ([1Password docs](https://developer.1password.com/docs/cli/secret-reference-syntax)).
  Names or ids; 1Password treats them case-insensitively. An optional `?attribute=<name>` query is
  accepted by the parser (1Password uses it for `otp`, `type` and so on) and passed through. Parts are
  non-empty, at most 256 characters each, with no control characters; `/` inside a name is not supported
  (1Password needs an id there). Anything else is malformed.
- `bws://<secret-id>`: our scheme for a Bitwarden Secrets Manager secret, addressed by its UUID
  (lowercase or uppercase hex, canonical 8-4-4-4-12 form). Nothing after the id.

In tool arguments a reference is written as an object with exactly one key:

```json
{"subject_name": "Square", "text": {"$secret": "op://Ops/Square/access_token"}}
```

Any value anywhere in the arguments (nested objects and arrays included) that is such an object is a
reference. An object with `$secret` and other keys, or a `$secret` that is not a string, is malformed.
At most 16 references per call.

## Tables (tenant migration `0025_credential_refs`)

### `credential_ref`

| Field | Type | Notes |
|---|---|---|
| `reference` | string | the `op://` or `bws://` string, as parsed and normalised (scheme lowercased) |
| `provider` | string | `onepassword_connect`, `onepassword_service_account` or `bitwarden_sm`; `bws://` only with `bitwarden_sm`, `op://` only with a 1Password kind |
| `vault` | record<vault> | the Eunomia vault the reference is bound to |
| `label` | string | a name for people (default: the reference) |
| `allowed_tools` | array<string> | tool names that may resolve it; `*` is any tool; empty is none |
| `allowed_actors` | array<string> | actor ids (`user:...`, `api_token:...`, `oauth_client:...`) that may resolve it; empty is any member of `vault` |
| `created_by`, `created_at`, `updated_at` | | |

Unique on `(vault, reference)`. **No value field, ever**; the schema is SCHEMAFULL so one cannot be
added by a write.

### `secret_provider`

One row per provider kind, id `secret_provider:<org key>_<kind>` (the org in the key keeps two orgs' rows apart even in one shared database, the isolation suite's second layer).

| Field | Type | Notes |
|---|---|---|
| `kind` | string | as above |
| `host` | option<string> | Connect: the server URL (required). Bitwarden: the server URL (optional; `https://vault.bitwarden.com` when absent; `https://vault.bitwarden.eu` or a self-hosted server). Service accounts: none |
| `token_enc` | string | the bootstrap token, `enc:v1:` AES-256-GCM under `ENCRYPTION_KEY` (`connectors/crypto.rs`), never plaintext; an ASSERT refuses anything not starting `enc:v1:` |
| `created_by`, `created_at`, `updated_by`, `updated_at` | | |

Both tables are in `store::TENANT_TABLES`, copied by `provisioning/legacy.rs` (empty on a legacy
install) and carry isolation canary rows.

## Resolution (in `registry::call`)

After `authorize()` (scope and restriction checks) and vault-name resolution, before the tool runs:

1. Walk the arguments for `$secret` objects. None: the call goes on unchanged (no extra query).
2. Parse each. Malformed: the call fails with `secret.unavailable` (outcome `denied`).
3. Look up `credential_ref` by reference among the vaults the caller belongs to (`vault IN $vaults`,
   the app-level isolation filter). The ref is allowed only when all hold:
   a row exists for it; the caller is an active member of `ref.vault`; a vault-restricted credential's
   vault equals `ref.vault`; the tool's name is in `allowed_tools` (or `*`); `allowed_actors` is empty or
   holds the caller's actor id. The first that fails: `secret.unavailable`, outcome `denied`. Several
   rows (the same reference bound to several vaults): the one whose vault is the call's `vault_id`, else
   the first the caller may use.
4. Load the org's `secret_provider` row for `ref.provider`. Absent: `provider_error`. Decrypt the token
   (`Zeroizing<String>`).
5. One `POST /v1/resolve` to the sidecar with every reference of the call for that provider.
6. Any failure (sidecar down or slow, provider error, a reference not found): the whole call fails with
   `secret.unavailable`; nothing runs. Never a fallback.
7. On success the tool runs with a **copy** of the arguments where each `$secret` object is replaced by
   its value. The original arguments (references only) are what the audit row, the capsule, the span and
   any log line see.
8. While the tool runs, the resolved values sit in a task-local scrub set. `redact::redact` (the #111
   write path for memories, skills and audit summaries) replaces any exact occurrence with
   `[redacted: secret]` before #111's shape detectors run, so a tool that writes memory in the same call
   cannot store the value. After the tool returns, the same exact-value scrub runs over every string in
   the result before it goes back to the agent.
9. The resolved copy of the arguments and the values are zeroised (`zeroize`) when the call ends. Best
   effort: a tool handler receives an owned `serde_json::Value` and its internal copies are dropped,
   not zeroised.

Read-only tools are resolved the same way (an `allowed_tools` entry is needed either way).

## Audit

Each reference resolution attempt writes one control `audit_event`:

| Field | Value |
|---|---|
| `action` | `secret.resolve` |
| `user`, `actor_kind`, `actor_id` | the caller (user session, PAT or OAuth client) |
| `target` | the org key (what scopes the resolution log to one org; `audit_event` has no org field) |
| `outcome` | `ok`, `denied`, `not_found` or `provider_error` |
| `trace_id` | the request's trace id |
| `detail` | compact JSON: `{"ref": "<reference>", "ref_id": "<credential_ref id or empty>", "tool": "...", "channel": "rest|mcp|chat", "session": "<MCP session or chat thread>", "vault": "<vault id>", "reason": "<short reason when not ok>"}` |

**Never the value.** References are not secrets (they name an item), so they are kept. The resolution
log in Settings reads these rows (`action = 'secret.resolve' AND target = <org>`), newest first.

## Error

`secret.unavailable` (HTTP 503 on REST, a tool error value on MCP): a `$secret` reference could not be
resolved for this call. The message says which reference and whether it was denied, not found or a
provider failure; never a value or a token. Added to `docs/errors.md`.

## Settings API (backend, B1)

Org owners and instance admins only (`membership.role = 'owner'` or `authz::is_admin`); anyone else
gets `auth.forbidden`. A vault-restricted token gets `auth.scope` (account-level routes).

| Route | What |
|---|---|
| `GET /api/secrets/providers` | each configured kind: `{kind, host, configured: true, updated_at, updated_by}`; the token is never returned |
| `PUT /api/secrets/providers/{kind}` | `{host?, token?}`; `token` is required when none is stored, otherwise omitted keeps it; validated shape (non-empty, at most 4 KiB, no control characters) |
| `DELETE /api/secrets/providers/{kind}` | removes the row (the ciphertext with it) |
| `POST /api/secrets/providers/{kind}/test` | `POST /v1/check` on the sidecar; `{ok, error?, message?}` |
| `GET /api/secrets/refs`, `POST /api/secrets/refs` | list, add `{reference, provider, vault_id, label?, allowed_tools, allowed_actors?}` |
| `PUT /api/secrets/refs/{id}`, `DELETE /api/secrets/refs/{id}` | edit, remove |
| `GET /api/secrets/log?limit=&before=` | the resolution log (paginated, newest first) |
| `GET /api/secrets/counts?hours=` | per reference: `ok`, `denied`, `not_found`, `provider_error` counts over the window |

## Sidecar internal API (`secrets-helper`, version 1)

A small JSON-over-HTTP API on the internal compose network only (no published port). All bodies are
JSON, at most 64 KiB. Every response carries `Cache-Control: no-store`.

### Auth between backend and sidecar

Two layers:

1. **Network.** The sidecar is not on the default compose network. It joins `secrets` (`internal: true`,
   no outside access), which only `backend` and `secrets-helper` join, and `secrets-egress`, a plain
   bridge only the sidecar joins, for its outbound calls to the providers. The frontend, backup, updater,
   database and Caddy cannot reach it.
2. **Shared key.** `SECRETS_HELPER_KEY` in `.env`, sent as `Authorization: Bearer <key>` and compared in
   constant time. The installer writes it and the updater adds it when missing, like the other keys.
   Under an OLD updater (the first update to this release) the key is absent: the sidecar then accepts
   calls on its internal network alone and logs once that the key is unset. That is the safe default:
   layer 1 already keeps every other container out.

Why both: the key alone would need the old updater to have written it (it cannot); the network alone
leaves a compromised backend-adjacent container on the shared network as a caller, which the dedicated
network removes, and the key makes a later network mistake fail closed.

### `GET /healthz`

No auth. `200 {"ok": true, "api": 1, "version": "<package version>"}`. Used by the compose healthcheck.

### `POST /v1/resolve`

```json
{
  "provider": {"kind": "onepassword_connect", "token": "<bootstrap token>", "host": "https://connect.example.com:8080"},
  "refs": ["op://Ops/Square/access_token", "op://Ops/Square/api/secret"]
}
```

`200`, one result per reference, in order:

```json
{"results": [
  {"ref": "op://Ops/Square/access_token", "ok": true, "value": "..."},
  {"ref": "op://Ops/Square/api/secret", "ok": false, "error": "not_found", "message": "no field \"secret\" in section \"api\""}
]}
```

Per-reference `error`: `not_found` (no such vault, item, section, field or secret), `denied` (the
provider refused this token for it), `invalid_ref` (malformed, or the wrong scheme for the kind),
`provider_error` (anything else, including a timeout).

### `POST /v1/check`

`{"provider": {...}}` -> `200 {"ok": true, "detail": "2 vaults visible"}` or
`200 {"ok": false, "error": "denied" | "provider_error" | "bad_config", "message": "..."}`.

### Request-level errors

`401 {"error": "unauthorized"}` (bad or missing key when one is set), `400 {"error": "bad_request",
"message": ...}` (not JSON, unknown kind, missing token or host, more than 16 refs), `413` (body too
large), `404` (unknown route).

### Rules the sidecar keeps

- Stateless: the token comes in every request and lives only for that request. No volume, no config
  file, no token in the environment, no cache.
- Never logs a request body, a token or a value. Every error message passes through a scrubber that
  replaces the request's token (and its URL-encoded form) with `[token]` and the resolved values with
  `[value]` before it is logged or returned.
- Timeouts: 8 s per provider call, 10 s per request. The backend waits at most 12 s (connect 2 s).
- Runs as a non-root user on a read-only root filesystem.

## Threat model

| Threat | Mitigation |
|---|---|
| Database dump or backup leaks the bootstrap token | stored only as `enc:v1:` ciphertext under `ENCRYPTION_KEY` (not in the database) |
| Settings API leaks the token | write-only: no response carries it or its ciphertext; a test asserts it |
| A resolved value lands in memory, a skill, `audit_log`, a capsule, `tool_usage`, a log line or the change log | the original arguments (references) are what is recorded; the task-local exact scrub covers memory and skill writes in the same call; the result is scrubbed; #111's detectors run under it; canary test |
| A value is echoed back to the agent | result scrub (exact values), then the agent sees `[redacted: secret]` |
| Org B resolves org A's reference | references live in the org's own database; the sidecar has no org state; isolation test |
| A vault-restricted token resolves a reference of another vault | the restriction check in step 3; test |
| An agent with write scope resolves anything it can name | only registered references, only for the listed tools and actors, only for members of the bound vault |
| Another container calls the sidecar | dedicated internal network plus the shared key |
| The sidecar logs a token in an error | scrubber on every message; test |
| Provider down | `secret.unavailable`, never a fallback; audited `provider_error` |
| Timing attack on the internal key | constant-time compare |
| A malicious tool argument smuggles a reference for another org | lookups are in the caller's org database only |

Residual risks: the value is in backend memory during the call, and copies inside a tool handler are
not zeroised; a tool that sends the value to an outside service is trusted to do so (that is the point);
the exact scrub does not catch a value the tool transformed (base64 of it, say), which #111's shape
detectors may or may not catch.

## Licence

| Package | Licence | Use |
|---|---|---|
| `zeroize` (Rust) | Apache-2.0 OR MIT | zeroising resolved values and the decrypted token |
| `@1password/sdk` (npm) | MIT | service accounts in the sidecar |
| Bitwarden `sdk-napi` / `sdk-sm` | Bitwarden SDK License Agreement (not OSI; limits what may be built with it) | **not used**: the sidecar has a small client of Bitwarden's public identity and API endpoints instead (open question 1) |

## Tests

- Reference parsing: good and malformed `op://` (two parts, empty parts, control characters, overlong,
  query), `bws://` (bad UUID, trailing path), `$secret` objects with extra keys or non-strings.
- Token stored only as `enc:v1:`, never returned by the API.
- Canary: a registered reference resolves (mock sidecar) to a canary value, a tool echoes it and writes
  it to memory; the canary is in none of `audit_log`, `audit_event`, `tool_usage`, `failure_capsule`,
  the change log, memory, skills, or the captured log output; the agent's result has the marker.
- Allowed tools, allowed actors, vault restriction, unregistered reference, provider not configured,
  sidecar down and sidecar slow: each `secret.unavailable`, each audited with the right outcome.
- Isolation: org A's reference does not resolve for org B; canary rows for both tables.
- Sidecar (B2): unit tests; a mock Connect and a mock Bitwarden through the real sidecar; no token on
  the sidecar's filesystem or in its environment after a call.
- Update path: the broker tables are new and empty after the update (listed as new), the sidecar starts
  healthy under the old updater's `docker compose up -d --remove-orphans`.

## Open questions for Avi

1. Bitwarden: the official SDK's licence is not OSI and restricts use; the sidecar talks to Bitwarden's
   public endpoints with its own small client (client credentials login, then decrypting the secret with
   the access token's key, as Bitwarden documents). It is tested against our mock, not against
   Bitwarden's cloud. Is a manual check against a real Bitwarden account before release acceptable, or
   should Bitwarden wait?
2. Admins: settings are for org owners and instance admins. Should vault owners manage references bound
   to their own vault?
3. Connector settings: resolving `{"$secret": ...}` inside connector credentials needs the sync jobs to
   call the broker. A follow-up PR, or in this release?
4. The resolution log reads `audit_event` without an index on `action` (a control migration would add
   one; this lane holds only tenant numbers). Fine at today's volumes; worth a control migration later?

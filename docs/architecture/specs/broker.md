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
| `bitwarden_sm` (**preview**) | a machine account access token, optional server URL (default the US cloud) | Bitwarden's public identity and API endpoints, with our own small client (see Licence) |

**Bitwarden Secrets Manager is a preview (decided by Avi on 2026-11-01).** Our own client is tested
against a mock of Bitwarden's API, not yet against Bitwarden's cloud. One source of truth,
`secrets::PROVIDERS` (backend), gives each kind a `stability` (`stable` or `preview`), a `preview` flag
and a one-line `note`; `GET /api/secrets/kinds` and every provider in `GET /api/secrets/providers` carry
them, and Settings shows a "Preview" badge and the note from there. It leaves preview once it has been
checked against a real Bitwarden account (open question 1).

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

`secret.unavailable` (a tool error value, on REST and MCP alike, as every tool error; HTTP 503 from the
settings routes): a `$secret` reference could not be resolved for this call. The message says which
reference and whether it was denied, not found or a provider failure; never a value or a token. A stable
`reason` comes with it (`reason` in the tool error value and in the problem body):

| `reason` | When |
|---|---|
| `helper_disabled` | the secrets helper is switched off (its `secrets` compose profile is not on). Answered at once: no provider token is decrypted and nothing goes on the network |
| `helper_unreachable` | the helper is on but did not answer properly: down, starting, slower than the timeouts, refusing the internal key, or answering nonsense |
| `denied`, `not_found`, `provider_error` | otherwise, the first refused reference's outcome |

The `secret.resolve` audit row carries the same `helper_disabled` or `helper_unreachable` in
`detail.code`. Added to `docs/errors.md`.

## Settings API (backend, B1)

Who may (vault owners decided by Avi on 2026-11-01):

- **Org owners and instance admins** (`membership.role = 'owner'` or `authz::is_admin`): everything, for
  every vault they belong to. Only they set up a provider: its token is org-wide. Saving, removing or
  testing a provider also needs a browser session.
- **Vault owners** (`vault_member.role = 'owner'`): create, edit and remove the references bound to a
  vault they own, and read that vault's resolution log and counts (entries whose `detail.vault` is a vault
  they own). They see which provider kinds are configured (no host, no `updated_by`), to pick one. They
  cannot bind a reference to, or move one into, a vault they do not own (`vault.forbidden`); another
  vault's reference is not found to them (404, as another org's is); every provider route and the helper
  switch are `auth.forbidden`. Every user owns their personal vault, so every member can register
  references for it, which only they can then use (see the threat model).
- **Instance admins only**: switching the secrets helper on or off (`POST /api/secrets/helper`), as for
  HTTPS: it changes the server's compose stack.
- **Anyone signed in**: `GET /api/secrets/kinds` and `GET /api/secrets/helper`.

A vault-restricted token gets `auth.scope` on every route here (account-level in `gate.rs`), **even for
the vault it is restricted to**: such a token is an agent's, and letting it register or widen references
would let an agent grant itself secrets. An unrestricted token acts as its user. Anyone else gets
`auth.forbidden`.

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
| `GET /api/secrets/kinds` | the provider kinds: `{kind, label, stability, preview, note?}` (Bitwarden is `preview`) |
| `GET /api/secrets/me` | what the caller may do here: `{manage_providers, manage_refs, toggle_helper, vaults: [{id, name}]}` |
| `GET /api/secrets/helper` | the helper's status (below) |
| `POST /api/secrets/helper` | `{enabled}`: ask the updater to switch the helper on or off (below) |

For a vault owner the log is read in batches of 200 from the org's `secret.resolve` rows and filtered on
`detail.vault`; a page stops after 2000 rows read and then gives `next_before` to carry on.

## The helper is behind a compose profile (decided by Avi on 2026-11-01)

`secrets-helper` runs only under the compose profile **`secrets`**, off by default, the way `caddy` sits
behind `https`. An install that never turns it on is exactly as before under any updater's plain
`docker compose up -d --remove-orphans` (the old updater's whole update step): no helper container, no
helper image pulled. The backend still joins the empty internal `secrets` network.

**How the backend knows.** Compose passes `EUNOMIA_COMPOSE_PROFILES=${COMPOSE_PROFILES:-}` to the
backend. The helper is on when that list names `secrets` (`Settings::secrets_helper_enabled`); when the
variable is absent (the backend not run by this compose file: `cargo run`, the test suite, another
orchestrator) it is on and `SECRETS_HELPER_URL` is simply tried. Off, every resolution and "Test
connection" fail at once with `secret.unavailable`, `helper_disabled`; on but not answering,
`helper_unreachable` within the timeouts below (connect 2 s; a stopped container's name does not resolve,
so that is immediate). Settings, references, the log and everything else work either way.

**Status** (`GET /api/secrets/helper`): `state` is

| `state` | Meaning |
|---|---|
| `off` | the profile is not on (`reason: helper_disabled`); `message` holds the updater's error if the last switch failed |
| `starting` | an enable request waits for the updater, or the updater wrote `starting` less than 5 minutes ago and the helper does not answer yet |
| `running` | `GET /healthz` answered within 2 s (`version` is the helper's) |
| `unreachable` | the profile is on but the helper does not answer (`reason: helper_unreachable`) |

plus `enabled` (the profile, as this backend sees it), `pending` (`enable` or `disable` while a request
waits), `can_request` (the updater's `update-status/` directory is there) and `profile` (`secrets`).

**Switching it on and off: the updater channel (plan 7.3, as Settings, HTTPS does).** "Enable password
managers" (instance admins, browser session) writes `update-status/secrets.json`
(`{"enabled": true}`, one key per line). The updater (`scripts/auto-update.sh`, `secrets_apply`, every
20 s) validates it, removes it, and:

- **enable**: writes `starting` to `update-status/secrets-status.json`, adds `secrets` to
  `COMPOSE_PROFILES` in `.env` (keeping any other profile, such as `https`), so every later plain
  `docker compose up -d` keeps the helper, then `docker compose pull secrets-helper` and
  `docker compose up -d backend secrets-helper` (the backend is recreated to see the new profile: the app
  is away for a few seconds). Outcome `running` or `error` with a message.
- **disable**: removes `secrets` from `COMPOSE_PROFILES` (the line goes when it was the only profile),
  `docker compose rm -sf secrets-helper`, `docker compose up -d backend`. Outcome `off` or `error`.

The status goes to its own `secrets-status.json` rather than `status.json`, which the update check
rewrites whole every 10 minutes (the same reason HTTPS has `https-status.json`). The HTTPS switch now
adds and removes only its own profile in `COMPOSE_PROFILES`, so the two do not undo each other.

**The first update to this release runs the OLD updater**, which knows nothing of `secrets.json`: the
install comes up with the profile off (the backend answers `helper_disabled`). The `updater` service runs
`scripts/auto-update.sh` from the checkout afresh every 20 s (`scripts/updater.sh`), so the pass after the
checkout is already this release's script, and a request is acted on from then on (one written earlier
waits in `update-status/`). A host cron job running the script does the same.

**By hand** (no updater service, or an admin who prefers it): add `secrets` to `COMPOSE_PROFILES` in
`.env` (`COMPOSE_PROFILES=secrets`, or `COMPOSE_PROFILES=https,secrets` with HTTPS on), then
`docker compose up -d`. Settings shows exactly that when `can_request` is false.

D5 holds either way: the helper is stateless and the token never rests in it.

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
   constant time. The updater adds it when missing, like the other keys: on an update, and when it
   switches the helper on (`secrets_apply`), so a helper started through "Enable password managers" always
   has one, even on an install whose last update ran an OLD updater. Switched on by hand without one, the
   sidecar accepts calls on its internal network alone and logs once that the key is unset. That is the
   safe default: layer 1 already keeps every other container out.

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
| A vault owner registers a reference to any item the org's provider token can read (every user owns their personal vault, and any member can create a vault) | **the provider token's own scope is the boundary**: give the service account, Connect token or machine account access only to the password-manager vaults whose items any vault owner in Eunomia may use (docs/password-managers.md says so). Every registration is audited (`secret.ref.create` with the actor) and every resolution is in the log the org owners see. A per-provider allow-list of reference prefixes for vault owners would close it but needs a tenant migration (open question 5) |
| A vault-restricted (agent) token grants itself a secret | `auth.scope` on every settings route, even for its own vault; test |
| The helper is off or down | `secret.unavailable` at once (`helper_disabled`) or within the timeouts (`helper_unreachable`); never a hang, never a fallback |
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
- Bitwarden is `preview` in `GET /api/secrets/kinds` and in a configured provider; the 1Password kinds
  are `stable`; the UI shows the badge and the note (Playwright).
- Helper switched off: a resolution and "Test connection" answer `secret.unavailable` / `helper_disabled`
  in well under 2 s with nothing sent to the sidecar; every settings route still answers 200; the status is
  `off`. Helper on but not answering: `helper_unreachable` within 4 s; status `unreachable`. Status
  `running` against the mock; the enable request file (admins, browser session; a token gets
  `auth.scope`, a member `auth.forbidden`); `starting` while it waits; the updater's error shown.
- Vault owners: own vault create, edit, remove, list, log and counts; another vault (where they are a
  plain member) `vault.forbidden` or 404, and the row untouched; providers and the helper switch
  `auth.forbidden`; a token restricted to their own vault `auth.scope` on every route; a member with only a
  personal vault sees nothing of others'; org B sees nothing; the org owner as before.
- The updater (`scripts/tests/auto-update.test.sh`): enable adds `secrets` to `COMPOSE_PROFILES` and keeps
  `https`; disable removes only `secrets`; the HTTPS switch keeps `secrets`; a malformed request is
  refused; outcomes in `secrets-status.json`.
- Update path: the broker tables are new and empty after the update (listed as new); with the profile off
  (what the old updater gives) no `secrets-helper` container exists, the backend answers
  `secret.unavailable` / `helper_disabled` for "Test connection", the other services are healthy and the
  data is intact; then, switched on the way the new updater does it, the helper turns healthy.

## Open questions for Avi

1. Bitwarden: the official SDK's licence is not OSI and restricts use; the sidecar talks to Bitwarden's
   public endpoints with its own small client (client credentials login, then decrypting the secret with
   the access token's key, as Bitwarden documents). It is tested against our mock, not against
   Bitwarden's cloud. **Decided by Avi on 2026-11-01: Bitwarden ships as a preview**, marked in the API,
   the UI and the docs. Still open: who checks it against a real Bitwarden account, and when it leaves
   preview.
2. Admins: settings are for org owners and instance admins. Should vault owners manage references bound
   to their own vault? **Decided by Avi on 2026-11-01: yes**, for the vaults they own (see Settings API).
3. Connector settings: resolving `{"$secret": ...}` inside connector credentials needs the sync jobs to
   call the broker. A follow-up PR, or in this release?
4. The resolution log reads `audit_event` without an index on `action` (a control migration would add
   one; this lane holds only tenant numbers). Fine at today's volumes; worth a control migration later?
   (A vault owner's log page reads up to 2000 of the org's rows to fill a page, which makes the index
   more worthwhile.)
5. Vault owners can register a reference to any item the org's provider token can read (threat model).
   Today the provider token's scope is the boundary, and the docs tell admins to scope it. Should a later
   PR add a per-provider allow-list of reference prefixes for vault owners (a tenant migration), or an
   org setting that turns vault-owner management off?
6. The sidecar as a compose profile (**decided by Avi on 2026-11-01**, replacing "always on"): fresh
   installs also start with it off. Should `install.sh` gain a `--password-managers` flag that writes
   `COMPOSE_PROFILES=secrets` up front?

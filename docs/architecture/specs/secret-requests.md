# Secret requests (lane B4): spec

Status: draft for Avi, written before the code. Branch `feat/secret-requests`, stacked on #125
(`feat/broker-ui`), on the credential broker (`docs/architecture/specs/broker.md`). Tenant migration
`0032_secret_requests`.

## Goal

People stop pasting passwords and API keys into chat. When an agent (Claude Code, ChatGPT, the web
chat, any MCP or REST client) needs a secret, it **asks for one**: Eunomia shows the person a small,
masked pop-up in the Eunomia web app, the person pastes the value there, and the agent gets back only a
**handle** (`{"$secret": "eu://<vault>/<name>"}`). The agent can then **use** the handle in an outbound
HTTPS call to the hosts the person approved, and **transform** it server side (base64, a Bearer or Basic
header, an HMAC, a JWT signature) into new handles, without ever seeing a value.

In Avi's words: "instead of people putting their passwords and keys in the chat, a Claude and chatbot
can open up a little pop-up for the keys for the user to paste into, and they can hook that in and they
can transform that. Without seeing it, they can manipulate it."

## Non-goals

- Reading a value back. No API, tool, export or UI returns a stored value or its ciphertext, to anyone,
  the person who pasted it and the org owner included. A value can be replaced or deleted, never read.
- A general secret store with folders, sharing, versions or rotation. Eunomia-held secrets are small,
  per-vault, write-only slots for agents; the customer's own password manager (op://, bws://) stays the
  place for an organisation's managed secrets.
- A general web fetch tool. `http_request` only runs with at least one handle in it, only to hosts the
  person approved for every handle in the call.
- Copying 1Password or Bitwarden secrets into Eunomia. Transforms take only `eu://` handles; an op:// or
  bws:// value is never written to Eunomia's storage (the broker's own non-goal).
- Decode, reveal, compare, length, slice, case or regex transforms (see "Why no oracles").
- MCP `2026-07-28` (stateless, `InputRequiredResult`). Our MCP server speaks `2025-11-25` and earlier;
  see "MCP" for what we do and open question 2.
- Change-log rows for held secrets. The change log (#114) is for vault content that agents sync; a
  secret is not content and must not reach `create-eunomia watch`.

## Words

- **Request**: a `secret_request` row: an agent asked for a secret named `name` in vault `vault`.
- **Held secret**: a `vault_secret` row: a value Eunomia holds as `enc:v1:` ciphertext, pasted (or
  derived), with the rules for using it.
- **Handle**: `eu://<vault key>/<name>`, written in tool arguments as `{"$secret": "eu://..."}` like the
  broker's op:// and bws:// references. The vault key is the id after `vault:` (names are ambiguous and
  change); `name` is `^[a-z0-9][a-z0-9_.-]{0,63}$`.
- **Derived secret**: a held secret made by `secret_transform` from other held secrets. It records its
  lineage (its inputs and the operation).

## Flows

### 1. Request

The agent calls `secret_request`:

```json
{"name": "stripe_api_key", "label": "Stripe secret key", "description": "To list this month's payouts",
 "vault_id": "Acme", "target": {"hosts": ["api.stripe.com"], "placements": ["header"]}}
```

Eunomia checks the caller may write the vault, stores a pending request (label and description through
#111's redaction, so an agent cannot park someone else's token in the description), audits
`secret.request`, and returns:

```json
{"request_id": "secret_request:k3...", "status": "pending",
 "fill_url": "https://eunomia.example.com/secrets/requests/k3...",
 "handle": {"$secret": "eu://8c0f.../stripe_api_key"},
 "expires_at": "2026-11-01T11:00:00Z",
 "next": "Ask the user to open fill_url and paste the secret there, never in chat. Then call secret_request_status."}
```

Never a value. The same call again (same vault, name and caller) while the request is pending returns
the same request, so a client that retries is safe. If a held secret with that name already exists and
this caller may use it, the answer is `{"status": "filled", "handle": ...}` and no pop-up is shown,
unless `replace: true`.

At most 10 pending requests per person (`rate.limited` beyond), so an agent cannot flood the pop-up.

### 2. The pop-up

- **Web app.** While signed in, the app polls `GET /api/secrets/requests?status=pending` every 5 seconds
  when the tab is visible (there is no user-level notification socket; the change-log WebSocket is per
  vault and about content, so polling is the smaller change). A pending request the person may fill opens
  a modal: who asked (the token's or OAuth client's name, the channel, the MCP session), the label and
  description (marked as written by the agent), the vault, the hosts it may be sent to and where in a
  request (header, query, body), and the request's expiry. A masked input (`type=password`,
  `autocomplete=off`, a client-side only Show toggle), **Save** and **Decline**; **Later** hides it for
  this tab. The same form is the standalone page `/secrets/requests/<id>` (the URL the agent shows).
  The person may edit the hosts (remove what the agent asked for, add others) and the placements before
  saving, and may tick "any agent of mine in this vault" (otherwise only the credential that asked may
  use it). The person, not the agent, decides where a secret may go.
- **MCP.** See "MCP" below: a URL-mode elicitation when the client declared it, otherwise the agent shows
  the URL.

### 3. Fill

`POST /api/secrets/requests/{id}/fill` from the browser session of **the person who asked** (the human
behind the token or OAuth client). Eunomia trims the value, checks 1 to 16384 characters with no NUL,
encrypts it (AES-256-GCM under `ENCRYPTION_KEY`, `connectors/crypto.rs`), stores the held secret, marks
the request `filled`, audits `secret.fill`, and answers `{"status": "filled", "handle": ...}`. The
plaintext is a `Zeroizing<String>` from the moment the JSON is parsed and is dropped right after
encryption.

### 4. Status

`secret_request_status {request_id}` (read-only, `memory:read`): `pending`, `filled` (with the handle),
`declined` or `expired`. Never a value.

### 5. Use

`http_request` with handles in headers, query or body:

```json
{"method": "GET", "url": "https://api.stripe.com/v1/payouts",
 "headers": {"Authorization": {"$secret": "eu://8c0f.../stripe_bearer"}}}
```

The handle resolves in `registry::call` exactly like an op:// reference (broker step 3 onwards), with the
held secret's own checks in place of `credential_ref`'s. The tool then checks the host and placement
against every resolved handle's grant, makes the call through the SSRF guard and returns status, content
type and the body, scrubbed.

### 6. Transform

`secret_transform {op, inputs, params, name?, expires_in_days?}` returns a new handle:

```json
{"op": "template", "inputs": {"key": "eu://8c0f.../stripe_api_key"}, "params": {"template": "Bearer {{key}}"}, "name": "stripe_bearer"}
-> {"handle": {"$secret": "eu://8c0f.../stripe_bearer"}, "lineage": ["vault_secret:..."], "allowed_hosts": ["api.stripe.com"], ...}
```

### 7. Decline, expiry, replace, delete

- Decline: the person who asked, an owner of the vault, or an org owner or instance admin.
  `secret.decline`.
- Expiry: a request lives `expires_in_minutes` (default 60, at most 10080, a week). Past that it reads
  as `expired` and cannot be filled; it is marked on the next read. A held secret may carry `expires_at`
  (the request's `secret_expires_in_days`, or a transform's); past it, it does not resolve
  (`secret.unavailable`, `denied`, reason "expired").
- Replace: `PUT /api/secrets/held/{id}` with a new value, browser session, by whoever filled it, an
  owner of the vault, or an org owner or admin. Its derived secrets are **deleted** (they hold the old
  value); agents re-derive. `secret.replace`.
- Delete: `DELETE /api/secrets/held/{id}`, same people, cascades to derived secrets. `secret.delete`.

## Data model (tenant migration `0032_secret_requests`)

### `secret_request`

| Field | Type | Notes |
|---|---|---|
| `vault` | record<vault> | where the secret goes |
| `name` | string | the stable key, as above |
| `label`, `description` | string | agent-written, redacted (#111), at most 120 and 1000 characters |
| `target_hosts` | array<string> | hosts the agent asked to send it to (`api.example.com`, `api.example.com:8443`, `*.example.com`) |
| `placements` | array<string> | subset of `header`, `query`, `body`; default `["header"]` |
| `connector` | option<string> | what it is for, shown only (no connector resolves handles yet, open question 4) |
| `requested_by` | record<user> | the human behind the credential |
| `actor_kind`, `actor_id`, `client_name` | string | the credential that asked (`token`/`api_token:...`, `oauth`/`oauth_client:...`, `user`), and its name for the pop-up |
| `channel`, `session` | string | `rest`, `mcp` or `chat`, and the MCP session |
| `status` | string | `pending`, `filled`, `declined`, `expired` |
| `replace` | bool | the agent asked to replace an existing secret |
| `secret_ttl_days` | option<int> | the held secret's lifetime, when the agent asked for one |
| `expires_at`, `created_at`, `decided_at`, `decided_by` | | |
| `secret` | option<record<vault_secret>> | the held secret once filled |

Indexes: `(vault, name, status)`, `(requested_by, status)`.

### `vault_secret`

| Field | Type | Notes |
|---|---|---|
| `vault`, `name` | | unique together; the handle is `eu://<vault key>/<name>` |
| `label` | string | |
| `value_enc` | string | ASSERT `string::starts_with($value, "enc:v1:")`: a plaintext value is refused by the database |
| `allowed_hosts` | array<string> | what the person approved; empty means no outbound use |
| `placements` | array<string> | where in a request a handle may go |
| `allowed_tools` | array<string> | default `["http_request", "secret_transform"]` |
| `allowed_actors` | array<string> | default the credential that asked; empty: any credential of a member of the vault |
| `derived_from` | array<record<vault_secret>> | lineage (empty for a pasted secret) |
| `op` | option<string> | the transform that made it |
| `request` | option<record<secret_request>> | the request it filled |
| `created_by`, `created_at`, `updated_at`, `expires_at` | | |

Both tables are SCHEMAFULL (no field can be added by a write), in `store::TENANT_TABLES`, copied by
`provisioning/legacy.rs` (empty on a legacy install), and carry isolation canary rows. No statement
outside the broker's resolution and the transform reads `value_enc`; every list reads fields by name.

## Who may do what

| Action | Who |
|---|---|
| `secret_request` | a credential with `memory:write` that may write the vault (`authz::authorize`, `WriteMemories`); a vault-restricted token only into its own vault (default vault: the restricted one, else personal) |
| See a request | the person who asked; an owner of its vault; an org owner or instance admin |
| **Fill** | **only the person who asked**, in a browser session, while they may still write the vault |
| Decline | the person who asked, an owner of the vault, an org owner or instance admin (browser session) |
| List held secrets | an owner of the vault, an org owner or admin, and the person who filled it |
| Replace, delete | the person who filled it, an owner of the vault, an org owner or admin (browser session) |
| Use (resolve) | as the broker: an active member of the vault, the restricted token's own vault, the tool in `allowed_tools`, the actor in `allowed_actors` (or empty), not expired |
| Activity log | as the resolution log (base rule, decided by Avi on 2026-11-01): org owners and instance admins every row; a vault owner the rows of the vaults they own (a row's `vault`, or the vault of its handle or of a handle an outbound call used; an outbound call's `lineage` keeps only their vaults' handles); anyone else `auth.forbidden`; a vault-restricted token `auth.scope` |

**Why only the asker fills.** The MCP specification (2025-11-25, URL mode) says the server MUST verify
that the user who opens the URL is the user the elicitation was for. More practically: if any vault
writer could fill, an agent of person A could get person B (a co-member) to paste B's key into A's
agent's slot. Vault and org owners keep control by declining and deleting, not by filling for others.

The REST routes are under `/api/secrets/` (the gate's account-level class, `vaults:admin`), so a
vault-restricted token never reaches them; fill, decline, replace and delete also require a browser
session, so no agent credential can fill its own request.

## API and MCP surface

### Tools (MCP and `POST /api/tools/<name>`)

| Tool | Scope | Arguments | Returns |
|---|---|---|---|
| `secret_request` | `memory:write` | `name`, `label`, `description`, `vault_id?`, `target? {hosts?, placements?, connector?}`, `expires_in_minutes?`, `secret_expires_in_days?`, `replace?` | `request_id`, `status`, `fill_url`, `handle`, `expires_at`, `next` |
| `secret_request_status` | `memory:read` | `request_id` | `request_id`, `status`, `handle?`, `decided_at?` |
| `secret_transform` | `memory:write` | `op`, `inputs {name: "eu://..."}`, `params {...}`, `name?`, `expires_in_days?`, `replace?` | `handle`, `name`, `op`, `lineage`, `allowed_hosts`, `placements`, `expires_at` |
| `http_request` | `memory:write` | `method`, `url`, `headers?`, `query?`, `body?` (string) or `json?` (any JSON) | `status`, `content_type`, `body`, `truncated`, `redirect` |

None is offered to the web chat agent in this PR (open question 5). `http_request` is the one built-in
tool in `broker::SECRET_TOOLS`.

### REST (`backend/src/routers/secret_requests.rs`)

| Route | What |
|---|---|
| `GET /api/secrets/requests?status=` | requests the caller may see, newest first, each with `can_fill`, `can_decline` |
| `GET /api/secrets/requests/{id}` | one request |
| `POST /api/secrets/requests/{id}/fill` | `{value, allowed_hosts?, placements?, any_agent?}` -> `{status, handle}` (session; asker only) |
| `POST /api/secrets/requests/{id}/decline` | -> `{status}` (session) |
| `GET /api/secrets/held` | held secrets the caller may see: name, handle, label, vault, hosts, placements, tools, actors, lineage, op, created and expiry; never the value or ciphertext |
| `PUT /api/secrets/held/{id}` | `{value}`: replace (session) |
| `DELETE /api/secrets/held/{id}` | delete, with derived secrets (session) |
| `GET /api/secrets/activity?limit=&before=` | the `secret.*` events the caller may see (as `GET /api/secrets/log`), newest first: the resolution log's shape (with `code`, the helper's `helper_disabled` or `helper_unreachable`) plus `action`, `lineage`, `host` |

### MCP

Researched on 2026-11-01 against modelcontextprotocol.io:

- **2025-11-25** (the newest revision our server speaks): servers MUST NOT use form-mode elicitation for
  passwords, API keys or tokens and MUST use URL mode. URL mode is `elicitation/create` with
  `mode: "url"`, `elicitationId`, `url`, `message`; the client declares `capabilities.elicitation.url`;
  `notifications/elicitation/complete {elicitationId}` MAY follow; and a server MAY instead answer the
  request with **`URLElicitationRequiredError`** (JSON-RPC error `-32042`, `data.elicitations: [...]`, each
  URL mode with an `elicitationId`), after which the client may retry the original request. The URL MUST
  NOT carry credentials or be pre-authenticated, SHOULD be HTTPS outside development, and the server MUST
  check the person who opens it is the person it was for.
- **2026-07-28** (newer): stateless, no `initialize` or `Mcp-Session-Id`; server-to-client requests only
  through `InputRequiredResult` (`resultType: "input_required"`, `inputRequests`, `requestState`); URL mode
  remains, `elicitationId`, `notifications/elicitation/complete` and `-32042` are removed. The password
  rule is unchanged.

Our `/mcp` is stateless JSON over Streamable HTTP: no SSE stream, `GET` is 405. It therefore cannot send
`elicitation/create` mid-call or the completion notification. What it does:

1. At `initialize`, when the negotiated version is `2025-11-25` and `capabilities.elicitation.url` is an
   object, the session id it hands out ends in `.u` (`<surface epoch>.<random>.u`). The id is the only
   session state we have, and a client forging the flag only changes what it is told itself.
2. `secret_request` with such a session, when the result is `pending`, is answered with the
   `URLElicitationRequiredError`:
   `{"code": -32042, "message": "...", "data": {"elicitations": [{"mode": "url", "elicitationId": "<request id>", "url": "<fill_url>", "message": "<label>: paste it in Eunomia, not in chat"}], "request_id": ..., "handle": ...}}`.
   The client shows the URL; when the person has filled it, the retried `secret_request` returns
   `filled` with the handle (the call is idempotent, flow 1).
3. Every other client gets the tool result with `fill_url` and `next`, and polls `secret_request_status`.

The fill URL is `<frontend origin>/secrets/requests/<request key>`: no token, nothing pre-authenticated;
the page needs the person's Eunomia browser session. A `fill_url` that is plain `http://` on a host that
is not loopback carries `"insecure_url": true` and is logged once as a warning.

## Resolution of `eu://` (changes to `secrets::broker`)

`reference::parse` gains `eu://<vault key>/<name>` (`Reference::Eu`). It never fits a provider kind,
so `POST /api/secrets/refs` refuses it. In `broker::prepare`, an `eu://` item skips the `credential_ref`
lookup and the sidecar: it reads `vault_secret` by `(vault, name)` among the caller's vaults and applies
the same checks plus expiry, then decrypts `value_enc` into a `Zeroizing<String>`. Everything after that
(all or nothing, the `secret.resolve` audit row, substitution into a copy of the arguments, the
task-local exact scrub, the result scrub, zeroising) is the broker's, unchanged.

**The secrets helper switched off** (the `secrets` compose profile, decided by Avi on 2026-11-01): only
the password-manager items (`op://`, `bws://`, those with a `credential_ref`) fail with
`secret.unavailable` / `helper_disabled`. A call of `eu://` handles only runs as usual: held secrets are
`enc:v1:` in the backend and never touch the sidecar. A call mixing both fails as a whole (all or
nothing), with the helper's reason. `eu://` is not a provider kind, so `GET /api/secrets/kinds` does not
list it and Settings has no "Preview" badge or helper state for it.

Two additions to `Prepared`:

- **Grants**: per resolved reference, its JSON pointer in the arguments and, for `eu://`, its allowed
  hosts and placements. `registry::call` puts them in a task-local for the tool (`http_request` reads
  them). op:// and bws:// references carry no hosts, so `http_request` refuses them (open question 1).
- **Encoded forms in the scrub set**: for each resolved value, its base64 (with and without padding),
  base64url, percent-encoded, form-encoded, lower-case hex and JSON-escaped forms join the exact-value
  set, so an upstream that echoes `Authorization: Basic <base64>` or a URL-encoded key is scrubbed too.

## Transforms

Inputs are `eu://` handles as plain strings (not `$secret` objects: the transform must know which
handle each value came from, for lineage, and the broker would substitute them first). Each input is
checked exactly as a resolution (`allowed_tools` must include `secret_transform`) and audited as a
`secret.resolve` with tool `secret_transform`.

| `op` | Inputs | Params | Output |
|---|---|---|---|
| `base64`, `base64url` | `value` | | standard padded, or URL-safe unpadded |
| `url_encode` | `value` | | percent-encoding of everything but RFC 3986 unreserved |
| `template` | any names | `template`: literal text with `{{name}}` slots, at most 4096 characters; every input used | the text with the values in (`Bearer {{key}}`, `{{user}}:{{pass}}`, `API_KEY={{key}}`) |
| `basic_auth` | `password`, optional `user` | `user` (literal) when no `user` input | `Basic base64(user:password)` |
| `hmac_sha256` | `key`, optional `message` | `message` (literal) when no input; `encoding`: `hex` (default), `base64`, `base64url` | the MAC |
| `sha256` | `value` | `encoding` | the digest |
| `jwt_hs256` | `key` | `claims` (a JSON object), `header` (`kid`, `typ` only), `expires_in_seconds` (adds `iat` and `exp`) | a signed JWT |
| `jwt_rs256`, `jwt_es256` | `key` (a PEM private key) | as above | a signed JWT |

JWTs use `jsonwebtoken` (already a dependency, MIT). Output is 1 to 16384 characters, or the transform
fails.

**Inheritance.** All inputs must be in one vault; the derived secret goes there. Its `allowed_hosts`,
`placements` and `allowed_tools` are the **intersection** of its inputs' (an empty host intersection
fails: there would be nowhere to send it). `allowed_actors`: the intersection of the non-empty lists
(empty means any member's credential). `expires_at`: the earliest of the inputs' and the requested.
`derived_from` lists the inputs; the audit row lists the whole lineage. A transform never widens what a
secret may do.

**Names.** `name` or `<op>_<first input>_<6 hex>`. An existing name is refused unless `replace: true`
and the existing secret is itself derived: a transform never overwrites a pasted secret.

### Why no oracles

Every allowed operation is one-way or expanding, and its result is another handle. Decode, compare to a
literal, length, slice, substring, case changes, regex match or "starts with" would each let an agent
learn bits of a value it cannot see (a compare is a guessing oracle; a slice or predicate would enable repeated guesses about the original value). The
minimum output of 1 character allows short keys and PINs. Errors name the input and the rule
broken, never content; the one bit that remains is whether a key parses as a PEM key for `jwt_rs256`.

## The consumer: `http_request`

Why an outbound HTTPS tool, not something narrower: a secret is only useful if something sends it
somewhere. B1 decided no built-in tool takes secrets because they store their arguments; an outbound
call stores nothing, and with the hosts the person approved it is the most direct "use without seeing".
The risks (SSRF, exfiltration) are real, so the tool is narrow:

- At least one handle; every handle's grant must allow the URL's host (exact, or `*.example.com` for
  subdomains only, with the port when not 443) and the placement (`/headers/...` is `header`,
  `/query/...` `query`, `/body` and `/json/...` `body`; a handle anywhere else, the URL included, is
  refused). op:// and bws:// references have no hosts and are refused.
- `https` only (tests map named hosts to a local plain HTTP mock, test builds only). No user or password
  in the URL, no IP literals (approved hosts are names).
- DNS through a guarded resolver: every address is checked at connect time, so a name that rebinds to a
  forbidden address fails: loopback, private (RFC 1918), CGNAT, link-local and metadata (169.254.0.0/16,
  fd00:ec2::), unique local, multicast, documentation and unspecified addresses, IPv4-mapped IPv6 unmapped
  first, and the compose `surrealdb` address. Always strict (`ALLOW_PRIVATE_LLM_URL` does not apply).
- No redirects: a 3xx comes back as `status` with `redirect: true` and no `Location`.
- Methods `GET`, `HEAD`, `POST`, `PUT`, `PATCH`, `DELETE`. At most 32 headers; `Host`, `Content-Length`,
  `Transfer-Encoding`, `Connection`, `Upgrade`, `Proxy-*`, `TE`, `Trailer`, `Cookie` refused. Request body
  at most 64 KiB. Connect 5 s, total 15 s. The response is read up to 256 KiB and at most 64 KiB of text
  returned (`truncated`); binary bodies come back as a size. Response headers are dropped except the
  content type (a `Set-Cookie` or echoed header could carry anything).
- The body goes through the exact scrub (values and their encoded forms) and #111's detectors before the
  registry's own result scrub. A transport error is reported by kind only (`timeout`, `connect`,
  `blocked`), never reqwest's text (it can carry the URL with a query value).
- Each call is audited `secret.http`: the host, method, status, the handles used. Not the path or query. A refused call is audited too: `denied` when a handle's grant does not allow the host or placement, `blocked` when the URL or an address it resolves to is refused, with status 0.

## Audit (control `audit_event`, `target` = the org key)

| `action` | When | `detail` (compact JSON, never a value) |
|---|---|---|
| `secret.request` | `secret_request` created one | `request`, `vault`, `name`, `hosts`, `channel`, `session` |
| `secret.fill` | filled | `request`, `secret`, `handle`, `vault`, `hosts`, `placements`, `any_agent` |
| `secret.decline` | declined | `request`, `vault` |
| `secret.resolve` | each use, as the broker (eu:// items: `ref` is the handle, `ref_id` the held secret) | the broker's fields |
| `secret.transform` | a transform | `op`, `handle`, `secret`, `lineage`, `outcome` |
| `secret.http` | an outbound call | `host`, `method`, `status`, `handles` |
| `secret.replace`, `secret.delete` | | `secret`, `handle`, `vault`, `cascade` (how many derived were removed) |

The Connectors page's resolution log reads `GET /api/secrets/activity` and shows the action.

## Redaction

- Tool output, failure capsules and the request's log lines: the exact scrub of every resolved and
  derived value (and its encoded forms) through the broker's `Prepared`, plus `http_request`'s own scrub.
- The pasted value: only the fill route and the replace route ever hold it; neither logs its body, the
  value is `Zeroizing` from deserialisation, the tool registry never sees it (it is not a tool), and no
  memory, skill, change-log or audit row is written from it. Request bodies are not logged by any layer
  (`tower-http` trace is not on the API). Tested with captured tracing at `trace`.
- Label and description go through `redact::redact` before they are stored or shown.

## Error codes (added to `docs/errors.md`)

| Code | HTTP | When |
|---|---|---|
| `secret.request_closed` | 409 | fill or decline of a request that is filled, declined or expired |
| `secret.host_denied` | 403 | `http_request` to a host, or with a placement, a handle's grant does not allow |
| `secret.outbound_blocked` | 403 | the SSRF guard refused the URL or an address it resolved to |
| `secret.outbound_failed` | 502 | the upstream did not answer (connect, TLS or timeout) |

Unchanged: `secret.unavailable` (a handle that does not resolve: unknown, expired, not allowed),
`validation.invalid`, `auth.forbidden`, `auth.scope`, `rate.limited`.

## Threat model

| Threat | Mitigation |
|---|---|
| A value pasted in the pop-up reaches the agent | no route or tool returns it; the agent only ever sees the handle; canary test over every tool, MCP and REST response |
| The value lands in a log, span, audit row, capsule, memory, skill or change-log row | it is never a tool argument; the fill route logs nothing of the body; canary test with captured tracing |
| Database dump or backup | `enc:v1:` only, schema ASSERT; `ENCRYPTION_KEY` is not in the database |
| **Prompt injection** makes the agent request secrets, or use them | a request only creates a pop-up the person must act on; label and description are marked as agent-written and redacted; use is limited to hosts the person approved; the pending cap stops floods |
| **Phishing via a fake pop-up**: an agent tells the person to paste into another site | the pop-up is only ever in Eunomia's own origin; the URL is shown in full; the pop-up and docs say Eunomia never asks for a secret in chat or on another site. A malicious page cannot be ruled out; the person checks the address (residual) |
| A fake request: agent A gets person B to fill A's slot | only the asker may fill |
| An agent fills its own request | fill needs a browser session; tokens and OAuth clients are refused |
| **SSRF** through `http_request` | https, named hosts only, approved hosts only, guarded resolver at connect time (rebinding), no redirects, private, loopback, link-local and metadata addresses refused |
| **Exfiltration through outbound calls**: the agent sends the key to its own server | only hosts the person approved for that handle; derived handles inherit the intersection |
| Exfiltration by storing the key upstream and reading it back (an approved API that echoes) | exact and encoded-form scrub of the response; #111 detectors; placements default to headers only. A service that transforms it (truncates, reverses) can still leak it to the agent (residual; the person chose to trust that host) |
| **Exfiltration through transforms** | no oracles (above); every output is a handle; minimum output 1 character |
| **Replay** of a fill or a request link | a request is filled once (`secret.request_closed` after); the link carries no authority; expiry; a stale MCP retry gets `filled` with the same handle, never a value |
| Org B sees org A's requests or secrets | per-org database; every statement filters by the caller's vaults; isolation canaries |
| A restricted token requests into another vault | `authz::authorize` on the vault; tested |
| A deleted or replaced key keeps working through a derived handle | delete and replace cascade to derived secrets |

Residual risks: the value is in backend memory during a fill and a call, and copies inside reqwest and
serde are not zeroised; the browser holds the value while the person types; an approved host that
transforms and echoes a key; a person approving a host they should not.

## Licence

No new dependency. `jsonwebtoken` (MIT), `hmac`, `sha2`, `base64`, `hex`, `url`, `reqwest`, `zeroize`
are already in `backend/Cargo.toml`.

## Tests

- Unit: handle parsing; every transform with known vectors (RFC 4231 HMAC-SHA256 cases 1, 2 and 4;
  the jwt.io HS256 example token; base64 RFC 4648 vectors; SHA-256 of "abc"); RS256 and ES256 sign and
  verify with a test key; inheritance (intersection, actors, expiry); the SSRF guard (private, loopback,
  link-local, metadata, CGNAT, IPv6 ULA and mapped, IP literals, http, userinfo, ports); placements; the
  encoded-form expansion; MCP session flag and the `-32042` shape.
- Integration (`backend/tests/secret_requests.rs`, in-memory 3.3.5, a mock upstream on loopback mapped to a
  test host): request, fill through the REST route, use in `http_request`, transform, use the derived
  handle; decline; expiry; replace (derived removed); delete; the DNS-rebinding refusal at connect time.
- **Canary**: one value through request, fill, resolve, every transform and `http_request` (an upstream
  that echoes it in the body, a header, and a 500 error, raw and base64 and URL-encoded); it is in no tool
  output, MCP response, REST response, captured log line, `audit_log`, `audit_event`, `tool_usage`,
  `failure_capsule`, `change`, memory or test snapshot, and the database holds it only as `enc:v1:`.
- Authz and isolation: only the asker fills; a token cannot fill; vault owners decline and delete; a
  restricted token only requests into its vault; org B sees nothing of org A.
- MCP: the `-32042` answer for a URL-capable `2025-11-25` session, the plain result otherwise.
- Playwright (`frontend/tests/secret-requests.spec.ts`): an agent's request opens the masked pop-up; Save
  and Decline; the agent's status call never carries the value.
- Gate: `update-path` from v2.0.7 and v2.0.8 lists `secret_request` and `vault_secret` as new and empty,
  then a request is created on the upgraded stack; idempotent; rollback.

## Owner decisions, 2026-11-03

1. Approved hosts and placements for op:// and bws:// references are a follow-up after #126. They remain unavailable to http_request until then.
2. MCP revision 2026-07-28 is scheduled for the next release. The current protocol remains unchanged.
3. A mixed-vault call remains visible to each vault owner, with only that owner's handles in its lineage.
4. Connector credential fields accept bare eu:// handles or a $secret object. A request must name the connector kind in target.connector, granting connector:<kind>. Background sync also needs the person to approve any agent. Test, sync, summaries and webhook checks resolve at use time, with the caller's vault and expiry checks. Provider HTTP calls enforce approved hosts and placements through the outbound SSRF guard, including pagination. Hosts must cover both token and API endpoints for OAuth; refresh_token needs body placement. Values and encoded forms are scrubbed before provider data reaches the ingest pipeline.
5. In-app chat access is approved after testing on real installs. The tools stay unavailable to chat until that gate passes.
6. Pasted and derived values may contain 1 to 16384 characters. Empty and whitespace-only values remain invalid. For http_request, any resolved form shorter than 8 characters causes the upstream response body and content type to be withheld. This avoids disclosing short keys through encoded echoes. Fixed status metadata remains available. Short connector credentials may cause broad redaction of provider data or a refused response.
7. Claude plugin and ChatGPT app packaging are a follow-up after #126.

The hosted installer must generate SECRETS_HELPER_KEY. Its source is outside Eunomia, so it remains a separate follow-up. The resolution index is control migration 0007; tenant 0027 remains free for the ontology.

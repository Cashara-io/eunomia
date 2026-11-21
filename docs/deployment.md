# Production deployment

`docker compose up` (see the main [README](../README.md)) is fine for local
dev, where every service talks to `localhost` over plain HTTP. Putting this
in front of real users needs three more things: TLS, env vars that aren't
`localhost`/default secrets, and knowing that the frontend's `NEXT_PUBLIC_*`
vars are baked in at *build* time, not read at runtime.

## 1. HTTPS

### Built in: Let's Encrypt via Caddy

The stack ships an optional `caddy` service (compose profile `https`) that serves
Eunomia at your own domain with a free Let's Encrypt certificate and renews it on its own.

You need:

- a domain (or subdomain) whose DNS A/AAAA record points at this machine;
- ports 80 and 443 reachable from the internet and not used by anything else
  on the machine (Let's Encrypt checks port 80 when issuing).

Turn it on in any of three ways:

- **Settings, HTTPS** (instance admins): enter the domain and an email for Let's
  Encrypt, click **Enable HTTPS**. The status reads *Pending* until the certificate is
  issued (usually under a minute), then *Active*. Problems (a port already in
  use, DNS not pointing here yet) show up there. **Disable** stops it.
- **The installer**: `--domain eunomia.example.com --acme-email you@example.com`.
  It writes the same `.env` values as the by-hand route below.
- **By hand**: add to `.env`, then `docker compose up -d`:

  ```bash
  COMPOSE_PROFILES=https
  EUNOMIA_DOMAIN=eunomia.example.com
  EUNOMIA_ACME_EMAIL=you@example.com
  FRONTEND_TRUST_FORWARDED=1
  FRONTEND_BIND=127.0.0.1
  PUBLIC_URL=https://eunomia.example.com
  ```

`./Caddyfile` proxies everything to the **frontend**, which forwards `/api`, `/mcp`,
`/oauth`, `/.well-known`, `/healthz` and `/readyz` to the backend, so the web app, the
API and MCP all live at `https://<domain>`. Certificates are kept in the
`eunomia-caddy-data` volume. Plain HTTP on port 80 redirects to HTTPS.

**How the client address stays correct.** Caddy is not in front of the backend, it is in
front of the frontend, which is the one peer the backend trusts (`TRUSTED_PROXIES=frontend`,
unchanged: Caddy does not go in that list). Caddy sets `X-Forwarded-For` to the connecting
client and `X-Forwarded-Proto: https`, replacing anything a client sent. For the frontend to
pass those on it needs `FRONTEND_TRUST_FORWARDED=1`, and that is only safe if nobody can
reach the frontend except through Caddy. So turning HTTPS on also sets
`FRONTEND_BIND=127.0.0.1`: port 3000 stops listening on the network (Caddy reaches the
frontend over the compose network, which that setting does not affect; agents on this
machine can still use `http://localhost:3000` and `http://localhost:8001`). It also sets
`PUBLIC_URL=https://<domain>`, which OAuth discovery needs. The backend's own port is
already loopback only. Turning HTTPS off removes `FRONTEND_TRUST_FORWARDED`,
`FRONTEND_BIND` and (if it is still the domain) `PUBLIC_URL`, so those three `.env` keys
belong to this feature while it is on. On Docker Desktop (macOS, Windows) Caddy may see the
Docker gateway instead of the real client address, so per-client rate limits then share
one bucket; native Linux Docker keeps the real address.

How Settings applies it: like updates (below), the backend never touches
docker or `.env`. It validates the domain and email and writes
`update-status/https.json`. Within 20 seconds the `updater` service validates
them again, sets the `.env` values, recreates `backend`, `frontend` and starts `caddy`,
and reports in `update-status/https-status.json`, probing the certificate until
it answers. Watch it with `docker compose logs caddy`. After each update the
updater reloads Caddy so a changed `Caddyfile` takes effect.

### Your own reverse proxy

Do not enable the built-in HTTPS if you run your own proxy (nginx, Traefik, an existing
Caddy): the two would fight over ports 80 and 443.

Put something in front of the `frontend` container (`:3000`) only. The
frontend already proxies `/api`, `/mcp`, `/oauth`, `/.well-known`, `/healthz`
and `/readyz` to the backend, and it is the one peer the backend trusts for
`X-Forwarded-For` (`TRUSTED_PROXIES=frontend`). Do not send any path straight to the
backend's port: it would see the Docker gateway address, so every user would share one
rate-limit bucket and `COOKIE_SECURE=auto` could not see https.
[Caddy](https://caddyserver.com/) gets automatic HTTPS (it provisions and renews a
Let's Encrypt cert itself) in a few lines, which is why it's recommended here over
hand-rolling nginx + certbot:

```caddyfile
# Caddyfile
eunomia.example.com {
    reverse_proxy localhost:3000
}
```

What reaches the backend (read from the Next 16.3 source: the rewrite to the backend is
proxied by Next's router before any page code runs, its proxy library has `xfwd` off, and
Next never adds `X-Forwarded-For` from the TCP peer on that path; `frontend/src/proxy.ts`
runs first and can only drop or keep what the client sent):

- A direct LAN client (the defaults, `FRONTEND_BIND=0.0.0.0`): `proxy.ts` drops any
  client-supplied `X-Forwarded-For`, `X-Forwarded-Proto` and `X-Real-IP`, so none reach the
  backend. The backend peer is the frontend container, which is trusted but sent no header,
  so every direct client shares one rate-limit bucket (the frontend's address) and
  `COOKIE_SECURE=auto` sees http. Put a reverse proxy in front if you need per-client limits. The consequence of one shared bucket: with a direct `:3000` install, a few failed sign-ins from one client can lock every other direct client out for the rest of the minute (the login limit is per address, and they all look like the same address).
- Caddy on the host in front of the frontend, with `FRONTEND_TRUST_FORWARDED=1`: `proxy.ts`
  keeps the headers. Caddy sets `X-Forwarded-For` to the connecting client and
  `X-Forwarded-Proto` itself (it replaces client-supplied values unless you configured
  Caddy's own `trusted_proxies`). Next passes them through unchanged, so the backend sees
  peer = frontend container, `X-Forwarded-For` = the real client, and uses the client.

For the Caddy-on-host case set in `.env`, and leave `TRUSTED_PROXIES` at its default
(`frontend`, already set by `docker-compose.yml`). Caddy is not the backend's peer, so it
does not go in `TRUSTED_PROXIES`. Add it only if its address ever shows up inside
`X-Forwarded-For` (the backend then takes that address for the client).

```
FRONTEND_BIND=127.0.0.1
FRONTEND_TRUST_FORWARDED=1
PUBLIC_URL=https://eunomia.example.com
```

`FRONTEND_BIND=127.0.0.1` stops anyone reaching `:3000` around the proxy, which is
what makes trusting the header safe. This behaviour is read from source, not yet checked
on a running stack (see the "Still to verify" list in `docs/architecture/foundation-plan.md`).

The compose file publishes the backend only on `127.0.0.1:8001`, so it is not
reachable from the LAN. Agents on the same machine keep using
`http://localhost:8001/mcp`; remote agents use `https://eunomia.example.com/mcp`.
To expose the backend port anyway, set `BACKEND_BIND=0.0.0.0` in `.env` (requests
then bypass the proxy chain, so the client address is the peer address).

An nginx equivalent needs its own `server { listen 443 ssl; ... }` block plus a
certbot container/cron to issue and renew the cert: more moving parts for
the same result as the built-in HTTPS above.

## 2. Env vars that need real values

`.env.example` documents every variable; these specifically must NOT keep
their local-dev defaults in production:

| Var | Local default | Production |
|---|---|---|
| `JWT_SECRET` | random per-process if unset | a fixed secret (`openssl rand -base64 32`) — sessions won't survive a restart otherwise, and every process must agree on one value |
| `ENCRYPTION_KEY` | none: a new install refuses to boot without one (at least 16 characters) | `openssl rand -base64 32`. It encrypts stored connector credentials and OpenAI keys, protects the per-org database passwords and derives the control database password. Keep a copy off the machine. Saved credentials are stored as `enc:v1:...` ciphertext; older values without the prefix still decrypt. If a stored value cannot be decrypted (a changed key, corrupted data, or a deployment that used to run without one) it is never sent anywhere: the feature reports an error and the credential has to be entered again in Settings or Connectors. `EUNOMIA_ALLOW_EMPTY_ENCRYPTION_KEY=1` skips the check for local development only (an empty key is a public, all-zero AES key, and the database passwords derived from or encrypted under it are computable by anyone). The backend refuses an empty key on every install, including one that already holds data written under it; see "Rotating ENCRYPTION_KEY" below. |
| `SURREAL_PASS` | `root` | a real password |
| `OPENAI_API_KEY` | blank (settable per-user instead) | set it server-wide, or rely on each user setting their own in Settings. It is only ever sent to `OPENAI_BASE_URL`; a user who points Settings at another endpoint uses their own key there |
| `OPENAI_CHAT_MODEL` | blank (automatic) | the model that extracts entities and relations, synthesizes answers and powers chat. Blank picks `gpt-4o-mini` on api.openai.com, otherwise the first chat model your endpoint lists at `/models` (for example a local Ollama or vLLM model). Each user can override it in Settings |
| `EUNOMIA_CHAT_MODEL_TIMEOUT_SECS` | `90` | How long one model call in the in-app chat may take, its streamed reply included. Past it the chat stops the turn with an error and the thread stays usable |
| `CORS_ALLOWED_ORIGINS` | `http://localhost:3000` | only needed for browsers calling the backend port directly; the bundled frontend goes through its own `/api` proxy |
| `FRONTEND_URL` | `http://localhost:3000` | same as above (this is what `CORS_ALLOWED_ORIGINS` defaults from in `docker-compose.yml`) |
| `SESSION_TTL_DAYS` | `30` | Browser sessions expire this many days after last use (extended at most once an hour). The session cookie itself also carries a hard 90 day limit. |
| `RATE_LIMIT_USER_PER_MIN` | `1200` | Requests per minute per signed-in user. `0` turns the limit off. Over the limit the API answers 429 `rate.limited` with `Retry-After`. |
| `RATE_LIMIT_TOKEN_PER_MIN` | `600` | Requests per minute per API token (a user's limit applies as well). |
| `RATE_LIMIT_AUTH_PER_MIN` | `20` | Login, signup, OAuth token and failed-credential attempts per minute per client address (read from `X-Forwarded-For`, but only when the connecting peer is in `TRUSTED_PROXIES`). |
| `RATE_LIMIT_WEBHOOK_PER_MIN` | `120` | Source webhook and extension inbox deliveries per minute per client address. |
| `EXTENSION_BUFFER_MAX` | `10000` | Waiting plus leased events one extension's webhook inbox holds before it answers 429 `extension.buffer_full`. See [extensions.md](extensions.md). |
| `MAX_REQUEST_BODY_BYTES` | `1048576` | Largest request body any route accepts (413 over it). Raise it only if a route you use needs more. |
| `TRUSTED_PROXIES` | loopback only (`127.0.0.0/8`, `::1/128`); `docker-compose.yml` sets `frontend` | Comma-separated proxies whose `X-Forwarded-For` is believed: CIDRs, bare addresses or hostnames. A hostname (the compose service `frontend`) is resolved at boot and again every 60 seconds; the last good answer is kept and failures are logged at warn. The right-most address that is not itself trusted is the client. A LAN or bridge peer that is not listed cannot spoof its address. Only if you point a proxy straight at the backend (not recommended, see section 1): add its address or name (`TRUSTED_PROXIES=frontend,203.0.113.5`). `none` trusts nobody (every client is its peer address). |
| `PUBLIC_URL` | `http://localhost:8001` | The address MCP clients reach Eunomia at, no trailing slash, e.g. `https://eunomia.example.com`. Used in OAuth discovery documents and as the audience of OAuth tokens, so it must match what clients connect to. |
| `SIGNUP` | `open` | Who can create an account once the install has a first user (the first user can always sign up). `open`: anyone who can reach the server. `invite`: only emails listed in `SIGNUP_ALLOWLIST` (comma separated). `closed`: nobody. A typo closes signup. The default is `open` because a vault invitation needs an existing account (`no user with email`), so a team's second member has to be able to sign up before anyone can invite them. **On an instance reachable by people you do not trust, set `SIGNUP=invite` or `closed` after your team has signed up.** The same gate applies with `EUNOMIA_SIGNUP_ORG=personal`. Once the install has users, an address listed in `EUNOMIA_ADMIN_EMAILS` can never sign up (so nobody can claim an operator address first): register the account, then list it. |
| `SIGNUP` + `ALLOW_PRIVATE_LLM_URL` | see above | Defaults `open` and allowed together: anyone who can reach the server can sign up and point a model URL at a private address. The backend logs one warning at boot when both are in effect. Set `SIGNUP=invite` or `ALLOW_PRIVATE_LLM_URL=0` on a reachable instance. |
| `SIGNUP_ALLOWLIST` | empty | Emails allowed to sign up when `SIGNUP=invite`. |

**Organisation invitations override `SIGNUP`.** An owner or admin of an organisation can invite an email from Settings > People (see [Organisation invitations](#organisation-invitations)). A valid invitation link is the permission to create that one account, with that one email, under `SIGNUP=invite` or `closed`. An address in `EUNOMIA_ADMIN_EMAILS` cannot be invited or sign up this way, by password or single sign-on.
| `AUTH_MODE` | `password` | How people sign in. `password`: email and password only (what every install did before OIDC). `both`: email and password, plus "Continue with {provider}". `oidc`: the provider only; password login and signup answer 403 `auth.password_disabled` and the web app hides the form. Needs the `OIDC_*` settings below for `both` and `oidc`; a typo or a missing setting stops the backend at boot rather than falling back to another method. See "Single sign-on" below. |
| `OIDC_ISSUER` | none | The provider's issuer URL, exactly as it appears in its discovery document (`{issuer}/.well-known/openid-configuration`), for example `https://dev-123.okta.com`, `https://accounts.google.com`, `https://login.microsoftonline.com/<tenant>/v2.0` or a Keycloak realm URL. https only (http for `localhost`). |
| `OIDC_CLIENT_ID` | none | The client id you registered for Eunomia at the provider. |
| `OIDC_CLIENT_SECRET` | none | The client secret. Leave it out for a public client: PKCE is always used, so the secret is optional. Never logged. |
| `OIDC_SCOPES` | `openid email profile` | Scopes requested. `openid` is always added. The email claim is required to create or match an account. |
| `OIDC_PROVIDER_NAME` | `single sign-on` | The label on the button: "Continue with {name}", and "Signed in with {name}" in Settings. |
| `OIDC_INVITE_ACCEPT_UNVERIFIED_EMAIL` | `false` | Does one thing only: when the provider sends **no** `email_verified` claim at all, a new account may still accept the organisation invitation whose link this sign-in started from, if the email matches it (the invitation link is the second proof). It never applies to linking to an existing account, taking an account over, an `EUNOMIA_ADMIN_EMAILS` address, or the `SIGNUP=invite` allowlist: for those a missing claim always counts as not verified. A claim sent as `false` is never overridden. **Warning:** some providers let users, or the admins of other tenants, set the `email` claim to any address (a multi-tenant Microsoft Entra application is the well-known case). With such a provider, anyone holding an invitation link could accept it with an address they do not own, so leave this off unless you know the provider only issues addresses it controls. |
| `OIDC_REQUIRE_MFA` | `false` | `true` refuses a single sign-on sign-in unless the ID token says a second factor was used (`amr`, RFC 8176, or an `acr` in `OIDC_MFA_ACR`). Any other value stops the backend at boot. See [security/mfa.md](security/mfa.md). |
| `OIDC_MFA_ACR` | none | Comma-separated `acr` values your provider uses for multi-factor sign-in. |
| `COOKIE_SECURE` | `auto` | `auto`: the session cookie is `Secure` when `PUBLIC_URL` is https or a trusted proxy (`TRUSTED_PROXIES`) sent `X-Forwarded-Proto: https`; the browser usually talks to the frontend origin, which can be https while `PUBLIC_URL` is not. `true` / `false` force it. |
| `CAPSULE_ARGS` | `off` | `off`: failure capsules keep only a hash and the JSON shape of a failed call's arguments. `redacted`: keep the arguments with secrets masked, so `eunomia replay` can re-run them. See [debugging](debugging.md). |
| `AUDIT_RETENTION_DAYS` | `365` | The `prune_auth` job deletes audit events older than this. `0` keeps them forever. |
| `TOOL_USAGE_RETENTION_DAYS` | `90` | The same job deletes the hourly tool-call counters (Settings, Usage) older than this. `0` keeps them forever. |
| `EUNOMIA_EXPORT_MAX_MB` | `64` | The largest data export archive (Settings > Data, Export my data) the server keeps; a bigger one fails with a message saying so. Archives are kept for seven days. Erasure records are never pruned. See [subject-rights.md](privacy/subject-rights.md). |
| `SESSION_RETENTION_DAYS` | `7` | The same job deletes browser sessions that expired (or were revoked) this many days ago. |
| `CHANGE_LOG_RETENTION_DAYS` | `90` | The scheduler trims each vault's change log ([projects](projects.md#retention)) to rows newer than this, about hourly. `0` keeps rows whatever their age. |
| `CHANGE_LOG_MAX_ROWS` | `100000` | The same trim keeps at most this many of a vault's newest change log rows. `0` means no cap. A client whose cursor falls below what is kept gets `changes.resync`. |
| `RATE_LIMIT_REFRESH_PER_MIN` | `300` | OAuth refresh-token grants per minute per client and address, apart from `RATE_LIMIT_AUTH_PER_MIN` (which still covers code exchange, registration, login and signup). Many users behind one address refresh through the same app. |

A verified API token, OAuth access token or session cookie is remembered by the backend process for 20 seconds, so a busy credential costs no database lookup per request (never past the credential's own expiry). Revoking it through this process (Settings, `/oauth/revoke`, logout) takes effect at once; with several backend replicas, the others may honour it for up to those 20 seconds. A token's "last used" time is written at most once a minute.

Failed sign-ins are also limited per account and client address: after 10 failures in 15 minutes from one address that email gets 429 `rate.limited` from that address until the window passes (a success clears the count), so a stranger cannot lock a known account out for everyone. A looser ceiling of 100 failures per account across all addresses stops guessing spread over many addresses. OAuth token requests from a registered client get the same treatment after 30 failures; clients identified by a public metadata URL (CIMD) rely on the per-address limit instead, so a stranger cannot lock out every user of that app. Both counters live in the backend process and hold at most 50,000 keys (the oldest windows are dropped first).

| `ALLOW_PRIVATE_LLM_URL` | `1` | Whether a user's OpenAI-compatible base URL (Settings) may point at a private or loopback address, as a local model server such as Ollama needs. Link-local addresses (cloud metadata such as 169.254.169.254) and the compose services `surrealdb` and `backup` are always refused. Set `0` on an install shared with people you do not trust. The check runs when the URL is saved and again on every model call (hostnames are re-resolved at connect time, redirects are not followed). Remaining limits: the `surrealdb` address is resolved once per process, and only `0` keeps requests off other private services. |
| `EUNOMIA_CONNECTIVITY_PROBE` | blank | What the offline check connects to before a scheduled sync, for users with **Skip syncs while offline** on (Settings, General; default on). Blank uses `1.1.1.1:443`, `8.8.8.8:443` and `9.9.9.9:443`: a TCP connect only (no DNS, no data sent), all tried at once with a 2 second timeout, any one answering means online; the answer is cached for 30 seconds (10 when offline). Set a comma separated `host:port` list if your network blocks those, or `off` to never probe (always treated as online). The probe connects directly and ignores `HTTP_PROXY`/`HTTPS_PROXY`: a host that reaches the internet only through a proxy (or behind a firewall that blocks those addresses) reads as offline and skips every scheduled sync. The backend logs a warning naming this variable the first time that happens; point it at an address the host can reach directly, set `off`, or turn **Skip syncs while offline** off in Settings. |
| `EUNOMIA_ALLOW_CONNECTOR_BASE_URL` | `0` | Whether a connector's `config.base_url` (and OAuth `token_url`) is honoured, to point a connector at a self-hosted provider such as GitHub Enterprise. Off by default: on a shared server it would let any user choose which hosts the backend calls. Even when on, requests use the same outbound guard as model calls (metadata and the database refused, no redirects). See [connectors](connectors.md). |
| `EUNOMIA_SIGNUP_ORG` | `join` | `join`: a new user joins the install's one org (the first user of a fresh install creates it and owns it). `personal`: every signup gets an org of their own. Either way, someone who signs up through an organisation invitation joins the inviting org instead. **A host that serves several customer organisations must run `personal`.** Under `join` every account is in the operator's own organisation, so every customer is one admin grant away from instance-wide rights. |
| `EUNOMIA_ORG_POOL_CAP` | `256` | How many org database sessions one process keeps open (least recently used are dropped and signed in again on demand). |
| `EUNOMIA_DB_CONNECTIONS` | `8` | WebSocket connections one process opens to SurrealDB. Every database handle is signed in on each and statements take them in turn: SurrealDB works through one connection's requests largely one at a time, so a single connection caps the process at roughly one database core. More connections means more parallel queries (and one sign-in per connection the first time an org is used). |
| `EUNOMIA_DB_INFLIGHT` | `32` | Statements one process sends to SurrealDB at once; the rest wait in arrival order. Beyond the database's cores, more parallel queries only share its CPU, so heavy ones (recall, entity search) slow down behind light ones and the latency tail grows. Raise it with a bigger database host. |
| `BACKUP_ENCRYPTION_KEY` | none (the `backup` service generates one, see below) | Encrypts nightly backups. Recommended: generate it before the first `docker compose up` (`openssl rand -base64 32`), put it in `.env`, and keep a copy off the machine, since backups cannot be read without it. The auto-updater adds one when it updates an existing install. If it is still empty, the `backup` service creates a key on first start, saves it to `/backups/.backup-key` in its own volume and logs a one-time "COPY IT SOMEWHERE SAFE" notice. Tradeoff: that key lives in the same volume as the backups, so someone who copies the volume gets both and the encryption protects nothing against them; losing the volume loses the key too. It only guards against someone who sees the backup files without the volume's key file. Setting the key in `.env` keeps the two apart. |
| `SECRETS_HELPER_KEY` | empty | The key the backend and the `secrets-helper` sidecar share (the credential broker, `docs/password-managers.md`). Any long random string (`openssl rand -hex 32`); the auto-updater adds one when it updates an existing install and when it switches the sidecar on. While it is empty the sidecar still answers only the backend, on the internal `secrets` network no other service joins. It is not a password manager token: those are entered in Settings, Connectors and stored encrypted in the database. |
| `COMPOSE_PROFILES` | empty | Optional services, comma separated: `https` (Caddy, above) and `secrets` (the `secrets-helper` sidecar for password managers, `docs/password-managers.md`). Settings switches each on and off through the updater, which adds or removes only its own name. By hand: edit the line, then `docker compose up -d`. Set it here, not with `--profile`: the backend reads this list to know whether the sidecar is on. |

To restore, run `backend/scripts/restore.sh <backup name or file> [--wipe]` from the repo root. A backup of an install with org databases is a directory holding one file per database; each one is restored into its own database. The import needs more than the 60 second query and transaction limits of the hardened SurrealDB, so the script stops the backend, restarts SurrealDB with `docker-compose.import.yml` (the same flags minus those two limits), restores, and then recreates the hardened server and starts the backend again, even if the restore fails. The app is down meanwhile. `docker compose exec backup eunomia-backup restore ...` also works but imports into the hardened server, so a large database can hit the transaction limit. A nightly backup stops with an error (and writes nothing) when it cannot list the databases, rather than saving only the old single database.

Backup files are encrypted with AES-256-CBC and authenticated with an HMAC-SHA256 over the ciphertext, checked before anything is decrypted or imported, so a modified file is refused. Backups written by earlier versions (no authentication) still restore.

Each database export is checked before it becomes a backup: the server is asked for every table's row count just before and just after the export, the records in each table's `INSERT` statements of the dump are counted, and the two must agree (and the server must still answer afterwards). A table nobody wrote to during the export must match its count exactly; one written meanwhile must lie between its two counts, and if it does not, the database is exported again (at most 3 times). An export cut short by a dying server (for example an out-of-memory kill), or one missing rows for any other reason, is refused: the run fails with a `does not hold the rows the server counted: <table>(<n> rows counted, <m> in the export)` log line, writes no file and rotates nothing out. A database with no rows is a valid backup.

## Organisation invitations

On a host that runs many organisations (`EUNOMIA_SIGNUP_ORG=personal`), an organisation's owner, its admins (a role an invitation can give) and the instance admins invite colleagues from Settings > People:

- Invite an email as `member` or `admin`. Only the organisation's owner (or an instance admin, in their own organisation) can invite, resend or withdraw an `admin` invitation; an org admin manages `member` invitations only. Nobody can be invited as the owner by an organisation's admins (the owner of a new organisation on a hosted service claims it with a link the operator API makes: see [Running a hosted service](#running-a-hosted-service)). An org admin can invite members and see seats; it is not an instance admin (no server settings, no other organisations).
- An address listed in `EUNOMIA_ADMIN_EMAILS` can never be invited, and an invitation can never create it (by password or single sign-on). The refusal says only that the address can't be invited.
- **The admin gets a link** (`<frontend>/invite#<token>`) to copy and send themselves; with email configured (see [Email](#email-password-recovery-and-emailed-invitations)) it is also emailed to the invited address. It works once, expires after 7 days, and is shown once; Resend makes a new link (the old one stops working), Revoke withdraws it. Only a sha256 of the token is stored; the token is in no log or audit row, and it rides in the URL fragment, which browsers never send to a server.
- The person opens the link. The page names the organisation and the role and asks them to confirm before anything is created or joined. Then, without an account, they sign up with the invited email by password, or by single sign-on: the button posts the link's token to the server, which keeps it in the signed sign-in cookie (never in a URL, so never sent to the provider), and only that invitation is honoured, only when the signed-in email is the invited one. Signed in already, they accept with the button; the account's email must be the invited one (trimmed, lowercased).
- **An invitation is never used by email alone.** A first single sign-on without a link follows the host's normal sign-up rules (`SIGNUP`, `EUNOMIA_SIGNUP_ORG`), whatever invitations exist for the address.
- **An account belongs to one organisation for now.** An account already in another organisation cannot accept (`invitation.other_org`); invite a different email. There is no organisation switcher yet. Accepting an `admin` invitation as an existing member of that organisation makes them an admin (an owner stays the owner).
- Inviting an email that has an account in another organisation looks exactly like inviting one with no account, and when such an invitation is not accepted, the inviting organisation's log says only that it could not be accepted.
- Limits (per process): creating and resending, 20 a minute per organisation and per admin; opening or accepting links, 20 a minute per address or account; and after 10 links that don't work in 15 minutes from one address or account, further tries are refused for a while.
- `GET /api/org/seats` gives active members, suspended members and pending (unexpired) invitations, for billing on seats. Counts only. On a hosted service an organisation can have a seat limit that these count against (`org.seat_limit`, see [Running a hosted service](#running-a-hosted-service)).
- **Audit.** Invitations sent, resent, withdrawn, accepted and refused (expired, or could not be accepted) are written to the security ledger. Settings > Audit log shows the viewer's **own organisation** to its owner and admins (see [Who may administer what](#who-may-administer-what)), so each customer reads its own rows and never another organisation's.

## Who may administer what

Three kinds of authority, kept apart in code (`backend/src/org_admin.rs` holds one function per capability, and every route calls it) and here.

- **Organisation owner and organisation admin** (`membership.role`) run **their own organisation, and nothing outside it**. The organisation is always the caller's own (their oldest membership), never a request value, and a person is looked up through that organisation's membership before anything about them is read or changed: an account in another organisation gets the same `auth.not_found` as one that does not exist. An invitation can make a member an `admin`; only the owner changes roles afterwards. There is one owner per organisation, set when it is created or handed over (Settings > People > Data > Make workspace owner); nobody changes the owner's role.
- **Instance admins** (`authz::is_admin`: the install's first account, an address in `EUNOMIA_ADMIN_EMAILS`, or an account made an admin in Settings > People, which sets `user.admin`) keep every right they had. Inside their own organisation they are treated like its owner, as invitations always did.
- **Instance operators** (the install's first account, or an address in `EUNOMIA_ADMIN_EMAILS`) keep what spans organisations or the server: the instance ledger, the server-wide two-step verification switch, debug routes, updates, HTTPS and the AI provider, erasing another person's account, lifting a legal hold, and making an account an instance admin. An organisation's owner or admin passes none of those gates.

| Capability | Plain member | Org admin | Org owner | Instance admin | Instance operator |
|---|---|---|---|---|---|
| See the organisation's people (role, status, two-step verification, last activity) | no | own org | own org | own org | own org |
| Invite members; withdraw or resend a member invitation; see seats | no | yes | yes | yes | yes |
| Invite an `admin`; manage `admin` invitations | no | no | yes | yes | yes |
| Suspend or restore a member | no | members | members and admins | as before | as before |
| End a member's sessions; revoke their API tokens and connected apps | no | members | members and admins | as before | as before |
| Change a member's role (`member` or `admin`) | no | no | yes (never the owner's) | yes (never the owner's) | yes |
| Act on the owner, or on an account that is an instance admin or operator | no | no | no | as before | as before |
| Read, verify and export the organisation's audit log | no | yes | yes | yes | yes |
| Start, decide and complete the organisation's access reviews | no | yes | yes | yes | yes |
| Require two-step verification of the organisation's members | no | yes | yes | yes | yes |
| Rename the organisation | no | no | yes | no (unless owner) | no (unless owner) |
| The instance ledger (sign-ins with an unknown email) | no | no | no | only while the install has one organisation | yes |
| Server-wide two-step verification switch; make someone an instance admin | no | no | no | no / as before | yes |
| Erase another person's account; lift a legal hold; update, HTTPS, debug | no | no | no | update, HTTPS, debug as before | yes |

Rules that hold for every row: nobody acts on their own account through these routes (their own Settings do that); a **suspended** account has no powers at once (its sessions end, and its tokens and apps are refused); an organisation admin cannot act on another admin, and nobody but an instance operator acts on the owner. An address in `EUNOMIA_ADMIN_EMAILS`, the first account, and an account carrying the `user.admin` flag are **protected**: an organisation's owner or admin cannot suspend, restore, sign out or revoke them (`org.protected_member`). An organisation's owner, like anyone, can erase only their own account; erasing another person's stays with that person and the server operator.

**Removing a member.** There is no "remove from the organisation" distinct from erasing the account, which stays with the person and the operator. Suspending a member is how an organisation takes someone out: the account cannot sign in, its sessions end at once, and its tokens and apps are refused until it is restored. "Revoke all access" ends sessions, tokens and apps without suspending.

**Per-organisation two-step verification.** An owner or admin turns on "Require two-step verification" for the organisation in Settings > Organisation (`PUT /api/admin/org/security`). A member must use two-step verification when the server-wide switch or their organisation's policy requires it (`mfa::required_for`, asked at every enforcement point). Turning it on behaves for that organisation's members as the server-wide switch does: a password sign-in without it must enrol before it gets a session, a session that never passed a second factor is refused on its next request, a new account joining the organisation enrols at sign-up, and a member cannot turn it off while it is required. Members who sign in only through single sign-on are left to their identity provider, as for the server-wide switch. API tokens and app connections keep working. The policy binds only the organisation's own members, and an organisation cannot turn off the server-wide switch.

**Naming the organisation.** An organisation created under `EUNOMIA_SIGNUP_ORG=personal` is named after its founder's email until its owner renames it (Settings > Organisation, `PUT /api/admin/org/name`: 1 to 80 characters, no `@`). The name is shown to people invited to it.

**Audit.** Role changes, renames, policy changes, sign-outs and revocations are written to the organisation's ledger, attributed to the person who acted (`org.role`, `org.rename`, `org.require_mfa`, `admin.sessions_end`, `admin.access_revoke`, `admin.app_revoke`, plus the earlier `user.suspend`, `user.restore`, `admin.token_revoke`).

## Email, password recovery and emailed invitations

Eunomia sends email only when you give it a mail server. **With `SMTP_HOST` unset (the default) email is off and nothing in the product changes**: no route sends anything, invitations are copied and sent by hand as before, and the web app does not show "Forgot password?". A self-hosted install that configures no mail server behaves exactly as it did.

One path, SMTP, works with any provider (Resend's SMTP relay, Postmark, SES, your own server):

| Var | Default | Meaning |
|---|---|---|
| `SMTP_HOST` | empty (email off) | The mail server. Setting it turns email on. |
| `SMTP_PORT` | `587` (`465` with `SMTP_TLS=tls`, `25` with `none`) | The server's port. |
| `SMTP_USERNAME`, `SMTP_PASSWORD` | empty | Set both or neither (Resend: username `resend`, the API key as the password). Never logged or printed. |
| `SMTP_FROM` | none, required with `SMTP_HOST` | The sender, for example `Eunomia <no-reply@example.com>`. It must be an address your provider lets you send from. |
| `SMTP_TLS` | `starttls` | `starttls`: connect, then upgrade; nothing is sent if the server will not upgrade. `tls`: TLS from the first byte (usually port 465). `none`: no encryption, for a relay on a trusted local network only. TLS is rustls with built-in roots (no OpenSSL). |

A setting that cannot work (no `SMTP_FROM`, a bad port, a username without a password) stops the backend at boot, like a bad `AUTH_MODE`, rather than run without the mail you asked for. The web app learns whether email works from `email_enabled` in `GET /api/auth/bootstrap`.

- **A send waits at most 8 seconds** for the mail server. A failure is logged (`email not sent: ...`, with what the message was for and the error) and never carries the recipient, the body or a link. A mail failure never fails the request that wanted the mail: a reset request answers as always, and an invitation answers `emailed: false` so the admin copies the link.
- **Links** are `PUBLIC_URL` plus `/reset-password#<token>` for a reset. `PUBLIC_URL` must therefore be the address people open in a browser (see above). An invitation email carries exactly the link the admin can copy (`<frontend>/invite#<token>`, which is `FRONTEND_URL`, the first `CORS_ALLOWED_ORIGINS` entry, when set). On a single-origin deployment these are the same address. Tokens sit in the fragment, which browsers never send to a server.

**Forgot and reset password.** The login page shows "Forgot password?" when email is on. `POST /api/auth/password/forgot` with an email answers `{"ok": true}` for every well-formed address, whether or not an account exists, and the work done does not differ (the mail goes out from a task of its own), so it cannot be used to find out who has an account. An account that has a password and is not suspended is sent a link; a single sign-on account has no password and is sent nothing. The link works once, expires after 30 minutes, and only its sha256 is stored (on the account row, so asking again replaces the older link). Limits, per process: 10 requests per client address and 3 per target email in 15 minutes (past the address limit `rate.limited`; past the email limit no mail and the same answer), no new link within a minute of the last (kept in the database, so it holds across replicas), and 10 links that did not work per client address in 15 minutes. With email off the route answers 503 `email.unavailable`.

`POST /api/auth/password/reset` with the token and a new password (the sign-up rules) sets the password in one transaction that also spends the link. It **ends every browser session of the account** and drops its pending sign-in steps, and starts none: the person signs in with the new password. **Two-step verification is untouched**: the next sign-in still asks for the second factor, and a reset neither removes nor bypasses enrolment. **API tokens and app connections are not revoked**, the same as when single sign-on is linked to an account or an admin suspends it (sessions end, tokens are revoked one at a time by their owner or an admin). Revoke tokens in Settings, API tokens, after a suspected compromise. A password is never created for an account that has none.

**Change password.** Settings, Security: `POST /api/account/password` with the current password and a new one, from a browser session only (an API token cannot). The current password is counted like the one asked to set up two-step verification: 10 wrong in 15 minutes and it is refused, on every replica. An account with two-step verification passes the same step-up as for a new API token (a code from the last 10 minutes, or `mfa_code`). Every other session ends; the one making the change stays.

**Invitations by email.** With email on, creating or resending an organisation invitation also emails the link to the invited address. The admin still gets the link to copy, and the answer says `emailed` (the mail server took it). Resending uses the same limits as before (20 a minute per organisation and per admin). Without email nothing changes.

**Audit.** `auth.password_forgot` (every request; the account is named only when there is one, and the detail records whether a link was sent), `auth.password_reset` (completed, or refused as expired) and `auth.password_change` (changed, or a wrong current password) are written to the security ledger. No token, link or password is ever in an audit row, a log line or an error message. `org.invite` and `org.invite_resend` rows gain `emailed=true|false` only when email is on.

## Running a hosted service

A company can buy a workspace on your website and never talk to you: the website's billing service tells this server to create an organisation for the buyer, size it, pause it when payment fails and resume it when they pay. **With none of the settings below set (the default) nothing here exists**: there are no operator routes (they answer 404), no organisation has a plan, a seat limit or a suspended state, and an organisation behaves exactly as it always did. A self-hosted install is unaffected.

A hosted server runs with sign-up closed to the public, so the operator API (and organisation invitations) is the only way a new organisation comes into being:

```bash
SIGNUP=invite
EUNOMIA_SIGNUP_ORG=personal
EUNOMIA_OPERATOR_TOKEN=<openssl rand -base64 48>
EUNOMIA_BILLING_URL=https://billing.example.com/manage
EUNOMIA_MAX_ORGS=500            # optional
SMTP_HOST=...                    # optional, see Email above: the owner's claim link is also emailed
```

| Var | Default | Meaning |
|---|---|---|
| `EUNOMIA_OPERATOR_TOKEN` | unset (no operator API) | The bearer token of the operator API, at least 32 characters; a shorter one stops the backend at boot. It is not a user credential: it opens nothing but `/api/operator/v1/...`, and no session, API token or OAuth token opens those. Compared in constant time. Never logged, never in an audit row or an error. |
| `EUNOMIA_MAX_ORGS` | unset (no limit) | The most organisations this server holds, the server's own included. Creating another is refused with `operator.at_capacity`. A whole number. |
| `EUNOMIA_BILLING_URL` | unset | Where an organisation's owner goes to add seats or pay: `manage_url` in the plan status and the button on the suspended page. An `http(s)` address. |
| `RATE_LIMIT_OPERATOR_PER_MIN` | `300` | Operator API calls a minute, in all (the billing service is the one caller). A wrong token is throttled per client address like any failed credential. |

The server warns at boot when `EUNOMIA_OPERATOR_TOKEN` is set while `SIGNUP=open` (a hosted server should not let the public sign up). Putting the operator routes behind a network allow-list of the billing service's addresses is a sensible second layer; the token is the first.

### Plans, seats and the unmanaged organisation

An organisation record gains optional fields (control migration 0015): a plan (`trial`, `team` or `enterprise`), a seat limit, a status (`active` or `suspended`) with the reason and time it was suspended, a trial end (display only: **this server never suspends on its own**, the billing service decides) and an opaque external reference. An organisation with none of them set is **unmanaged and unlimited**: every organisation on a self-hosted install, and the server's own.

**Seats in use are active members plus pending (open, unexpired) invitations.** A suspended member does not count. When an organisation has a limit, these are refused with `org.seat_limit` (409), whose message says how many seats the workspace has:

- creating an invitation, and resending one that had expired (it takes a seat again);
- accepting an invitation, when that would take the organisation past its limit (an invitation already holds its seat, so accepting normally changes nothing; it is refused only for an organisation that is already over its limit);
- restoring a suspended member.

Each check and its write are one transaction that also writes the organisation's `seat_guard`, so two changes racing for the last seat meet on one row: the first commits, the other retries and finds the seat taken (tests: `two_people_racing_for_the_last_seat_do_not_both_get_it`, `two_restores_racing_for_the_last_seat_do_not_both_get_it`). Lowering a limit below the seats in use is refused the same way (`operator.seats_in_use`).

**Plan status for members.** `GET /api/org/plan` (any member, also while suspended) answers the plan, status, seat limit, seats in use, trial end and `manage_url`; an unmanaged organisation answers with nulls:

```json
{ "plan": "team", "status": "active", "seat_limit": 10, "seats_in_use": 7, "trial_ends_at": null, "manage_url": "https://billing.example.com/manage" }
```

### What suspension does

A suspended organisation's members get `org.suspended` (403) for every request that reads or writes workspace content: REST, MCP tools, uploads, connector sync, chat, webhook deliveries and peer sync into it. It is enforced **at one point, the request gate** (`gate.rs`, `plan.rs`): every authenticated route is refused unless it is on a short allow-list, so a route added later cannot forget it. `backend/tests/hosted.rs` walks every route in the OpenAPI document as a member of a suspended organisation and expects the refusal.

| Still allowed while suspended | Refused |
|---|---|
| Signing in and signing out | Every REST route that touches content: vaults, entities, documents, chat, connectors, sources, settings, tokens, the organisation's invitations and seats, audit log, access reviews |
| `GET /api/auth/me` and `GET /api/org/plan` (the plan status, with `manage_url`) | `/mcp` and every tool |
| Changing your own password (`POST /api/account/password`) | Uploads, live change streams (an open stream is closed within the recheck interval) |
| Your own data export and erasure (`/api/export/subject`, `/api/account/erasure`, `/api/account/activity`): people keep their data-subject rights when a company stops paying | Source and extension webhooks, shared-space (peer) links into the organisation |
| For an owner or admin, the People list (`GET /api/admin/people`) so they can see who is there | Everything else |

**Background work pauses and resumes without loss.** The job queue does not claim a job of a suspended organisation's member (connector sync, extraction, embedding, anything that costs model spend): it stays `ready` and runs after the resume. A job already running when the organisation is suspended is cancelled at its next lease check (about a third of the lease, 10 seconds by default) and handed back to the queue without using an attempt. Never paused: schema migrations of the organisation's database and a person's own export and erasure jobs.

**How fast.** The server that handles the operator call applies a suspension or a resume at once; any other replica within 3 seconds (the plan state is cached for that long per process). Nothing is deleted; there is no per-organisation switch for sign-in, and a member's sessions and tokens stay valid (they just reach nothing).

The web app shows a suspended workspace as one full-page state instead of a wall of failed requests: what happened, and for the owner a button to `manage_url` when there is one. Settings > People shows seats in use against the limit, with a link to `manage_url`, and the seat-limit refusal in words.

### The operator API

Base path `/api/operator/v1`, JSON, `Authorization: Bearer <EUNOMIA_OPERATOR_TOKEN>`. The document is in the OpenAPI description under the tag `operator`. A wrong or missing token, and any other kind of credential, is 401 `auth.unauthorized`; **the routes do not exist (404) while the token is unset**. Errors are the usual problem+json with a stable `code` ([errors.md](errors.md)). It reads and writes the control database only: organisation records, membership and invitation counts. It never reads anything from an organisation's own database (memories, documents, chat), which a test checks by planting a canary there.

**Every call is on the instance ledger** (Settings > Audit log for the server's operators, and `GET /api/admin/audit`), as actor `operator` with id `operator-api`, with the action (`operator.org_create`, `operator.org_list`, `operator.org_get`, `operator.org_update`, `operator.org_suspend`, `operator.org_resume`, `operator.owner_claim_resend`, `operator.capacity`), its target (`ref:<external_ref>` for a create, `org:<id>` otherwise), its outcome (`ok` or the error code) and the request's parameters (plan, seats, the owner's email, the suspension reason). A call refused by the gate (a wrong token) leaves an `auth.failed` row holding a hash of what was presented; a call refused by the rate limit is not recorded.

An organisation as the API shows it (`OrgSummary`), in every response below:

```json
{
  "id": "6f1c2f0a3b9d4c5e8a7b1d2e3f4a5b6c",
  "name": "Acme Ltd",
  "plan": "team",
  "status": "active",
  "seat_limit": 10,
  "seats": { "active_members": 3, "suspended_members": 1, "pending_invitations": 2, "in_use": 5 },
  "trial_ends_at": null,
  "suspended_at": null,
  "suspended_reason": null,
  "external_ref": "sub_1Nxyz",
  "owner_email": "owner@acme.example",
  "owner_claimed": true,
  "owner_claim_expires_at": null,
  "created_at": "2026-11-21T03:12:44Z"
}
```

#### Create an organisation: `POST /orgs`

Provisions the organisation's database and makes a **single-use, expiring (7 days) owner claim** for the owner's email. `external_ref` makes the call idempotent: the organisation's id is derived from it, so a retried webhook returns the same organisation and never a second one.

```http
POST /api/operator/v1/orgs
{ "name": "Acme Ltd", "owner_email": "owner@acme.example", "plan": "team", "seat_limit": 10,
  "trial_ends_at": "2026-12-05T00:00:00Z", "external_ref": "sub_1Nxyz" }
```
```json
{
  "organisation_id": "6f1c2f0a3b9d4c5e8a7b1d2e3f4a5b6c",
  "created": true,
  "claim_link": "https://eunomia.example.com/invite#q8Wv...",
  "claim_expires_at": "2026-11-28T03:12:44Z",
  "email_enabled": true,
  "emailed": true,
  "organisation": { "...": "an OrgSummary: owner_claimed false, seats.pending_invitations 1" }
}
```

- The **claim** is an organisation invitation of kind `owner` (the existing invitation machinery, with no inviting account). The person who opens `claim_link` sees the new workspace's name, creates their account (a password, or single sign-on, with two-step verification rules applying as for any sign-up; `SIGNUP=invite` does not matter) and becomes the organisation's one owner. The link works once; only a sha256 of its token is stored, and the token is in no log or audit row.
- `email_enabled` says whether this server sends email; `emailed` says whether the mail server accepted the owner's copy. When `emailed` is false, deliver `claim_link` yourself.
- **A retry** with the same `external_ref` and owner returns the same organisation with `created: false`; it has no `claim_link` (the link is shown once, when it is made: after a lost reply, call the resend route). A retry that finds a half-made organisation (a crash between steps) finishes it.
- `name`: 1 to 80 characters, no `@`. `seat_limit`: 1 to 1,000,000. `trial_ends_at` is optional.
- Refused: `operator.owner_in_other_org` (409: the email already has an account; an account belongs to exactly one organisation, so use another email), `operator.owner_email_reserved` (400: an address in `EUNOMIA_ADMIN_EMAILS` can never be made an owner this way), `operator.reference_reused` (409: that reference belongs to an organisation with another owner), `operator.at_capacity` (409: `EUNOMIA_MAX_ORGS` reached; a retry of an existing organisation is not refused), `validation.invalid` (400).

#### List and get: `GET /orgs`, `GET /orgs/{org_id}`

```http
GET /api/operator/v1/orgs?limit=50&offset=0
```
```json
{ "organisations": [ { "...": "OrgSummary" } ], "total": 12, "limit": 50, "offset": 0 }
```

`GET /orgs/6f1c2f0a3b9d4c5e8a7b1d2e3f4a5b6c` answers one `OrgSummary` (above). An id that is malformed or names no organisation is `operator.org_not_found` (404). The list includes the server's own organisation, with no plan. Seat usage uses the same counts as `GET /api/org/seats`.

#### Update: `PATCH /orgs/{org_id}`

Only the fields you send change: `name`, `plan`, `seat_limit` (a number, or `null` for no limit), `trial_ends_at` (a time, or `null` to clear).

```http
PATCH /api/operator/v1/orgs/6f1c2f0a3b9d4c5e8a7b1d2e3f4a5b6c
{ "plan": "enterprise", "seat_limit": 25 }
```

The answer is the updated `OrgSummary`. **Lowering the seat limit below the seats in use is refused** and changes nothing, so the billing service can tell the customer to remove people first:

```json
{ "status": 409, "code": "operator.seats_in_use", "detail": "The organisation uses 5 seats (active members plus pending invitations), more than that. Ask the customer to remove people or withdraw invitations first." }
```

#### Suspend and resume: `POST /orgs/{org_id}/suspend`, `POST /orgs/{org_id}/resume`

```http
POST /api/operator/v1/orgs/6f1c2f0a3b9d4c5e8a7b1d2e3f4a5b6c/suspend
{ "reason": "payment_failed" }
```

The answer is the `OrgSummary` with `"status": "suspended"`, `suspended_at` and `suspended_reason` set. The reason (1 to 200 characters) is for the operator: it is on the ledger and the organisation record and is never shown to members. Suspending again keeps the first time. `resume` takes no body and answers the `OrgSummary` with `"status": "active"`. See [What suspension does](#what-suspension-does). Suspending the organisation the server's own operators belong to is allowed and locks them out of its content: resume it through this API.

#### Resend the owner claim: `POST /orgs/{org_id}/owner-claim/resend`

While the organisation is unclaimed: a new token and a new 7-day expiry, and **the old link stops working at once**.

```json
{ "claim_link": "https://eunomia.example.com/invite#Zk3r...", "claim_expires_at": "2026-11-28T09:40:02Z", "email_enabled": true, "emailed": true }
```

Refused: `operator.owner_claimed` (409: the owner has already claimed it), `operator.owner_in_other_org` (409: the owner's email has got an account since, so the claim could never be used).

#### Capacity: `GET /capacity`

```json
{ "organisations": 12, "max_organisations": 500, "at_capacity": false }
```

`max_organisations` is `null` when `EUNOMIA_MAX_ORGS` is unset. The create route checks the limit in the transaction that writes the organisation; two servers creating at the limit at the same instant can in theory both pass, so keep a little headroom.

## Single sign-on (OpenID Connect)

An install can delegate sign-in to an OpenID Connect provider (AuthKit/WorkOS, Okta, Entra ID, Google, Keycloak, any compliant provider) instead of, or alongside, Eunomia's own email and password. SAML is not supported yet; the account mapping below only needs verified claims, so a SAML provider could feed the same code later.

1. At the provider, create a "web application" client (authorization code flow) with the redirect URI `{PUBLIC_URL}/api/auth/oidc/callback`. `PUBLIC_URL` must be the origin people open in the browser (the one the cookie is set on), for example `https://eunomia.example.com`.
2. Set `AUTH_MODE=both` (or `oidc`), `OIDC_ISSUER`, `OIDC_CLIENT_ID` and, unless the client is public, `OIDC_CLIENT_SECRET`; set `OIDC_PROVIDER_NAME=Okta` (or your provider) for the button label. Restart the backend.

**Flow.** The login page's button goes to `GET /api/auth/oidc/start?next=...`: the backend discovers the provider (cached) and redirects the browser with `state`, `nonce` and a PKCE challenge. Those, and where to return to, travel in a short-lived signed cookie (`eunomia_oidc`: HttpOnly, SameSite=Lax, 10 minutes, scoped to `/api/auth/oidc`; served over https it is `__Host-eunomia_oidc` with `Path=/` and `Secure`, so no other host, not even a sibling subdomain, can set it, and the same goes for `eunomia_oidc_link`); nothing is stored server-side, and a callback from a different browser has no cookie and is refused. At `GET /api/auth/oidc/callback` the backend checks `state`, exchanges the code with the PKCE verifier, and validates the ID token: signature against the provider's JWKS (cached, refetched when a token names an unknown `kid`, at most once every 30 seconds), `iss`, `aud` (and `azp`), `exp`, `nbf` and the `nonce`. Only RS*/PS*/ES*/EdDSA are accepted; `none` and `HS*` never are. Then it starts the same session cookie password login does and sends the browser to `next`. A failure sends the browser back to the login page with a message; every attempt, good or bad, is in the audit ledger (`auth.oidc_login`, `auth.oidc_link`, and `auth.register` for a new account).

**Agents.** Eunomia is already an OAuth 2.1 server for MCP clients (Claude Desktop and others). `/oauth/authorize` needs a browser session, so with OIDC on, an unauthenticated authorize request goes to the login page with `next` set to the consent page, the person signs in at the provider, comes back to the consent page, approves, and the client's token belongs to the same Eunomia account and vaults as their browser session. Nothing else to configure.

**Accounts.** A provider identity is `(issuer, subject)`, stored in the control database (`identity`, unique). The first time one signs in:

- if an account already has that email, the identity is linked to it **only when the provider says the email is verified** (`email_verified`). Otherwise the sign-in is refused. Providers that never send `email_verified` (Entra ID by default) therefore cannot link to an existing account; they can still create new accounts.
  - An account **with no password** (made by single sign-on) is linked straight away, and its other browser sessions are signed out.
  - An account **with a password** is not linked silently, because anyone could have registered the address with a password of their own before its owner first signed in. The person is sent to a page that asks: either type the account's password (it is linked, nothing is removed; wrong guesses count against the same throttle as the login form), or start a new account because they don't know it. Starting fresh **sets the existing account aside**: it is suspended (no session, API token, connected app, shared-space link or share code, extension, source webhook or queued job works for it any more), its sign-in address is replaced by a `set-aside:` placeholder, and its data stays where it is for a server operator to review, restore or erase. A new, empty account is made for the person exactly as a first sign-in would (the same `SIGNUP` rules; refused, with nothing set aside, if they would refuse it). Nothing of the old account is inherited. Someone who simply forgot their own password loses nothing, but needs an operator to get it back, and reconnects their apps. Both are in the audit ledger (`auth.oidc_link`, `auth.oidc_set_aside`). The provider sign-in waits for the choice in a short-lived signed cookie (`eunomia_oidc_link`, 10 minutes), bound to that identity and account and spent by either choice. A suspended account can be neither linked nor replaced there.
  - An account is linked to **at most one identity per provider**: a sign-in with a different identity at the same provider and the same address is refused (an operator relinks it).
- otherwise, when the sign-in started from an organisation invitation link (the invitation page's button), the account is made in that invitation's organisation, whatever `SIGNUP` says, if the invitation is still open, the signed-in email is the invited one and the provider verified it; the invitation is used up. A sign-in that did not start from a link never uses an invitation;
- otherwise a new account is made (same org membership and personal vault as password signup, and the same `SIGNUP` / `SIGNUP_ALLOWLIST` gate; an unverified email never satisfies `SIGNUP=invite`). It has no password, and password login for it always fails.

The next sign-ins find the account by `(issuer, subject)`, so a changed email at the provider does not create a second account.

**Operator addresses.** List an address in `EUNOMIA_ADMIN_EMAILS` only after its account exists and you know its credentials are the operator's (its password, if it has one, and its API tokens). An operator address follows the same linking rule as any other.

**Hosted instance: the first account is yours.** The install's first account is an instance admin for good, and on a host with several organisations it is one of the few accounts that can read the instance security ledger: every tenant's failed sign-ins, with the email that was tried (docs/audit-chain.md, "Ledgers"). On an instance that serves other people's organisations, create the operator's account first, before sign-up opens to anyone else, and list the operators in `EUNOMIA_ADMIN_EMAILS`.

**Provider-only bootstrap.** In `AUTH_MODE=oidc` the first sign-in on an empty install becomes the first user and the admin, exactly like the first password signup. That means anyone who can complete a sign-in at your provider is the admin if they get there first, so either restrict the provider application to the people you want, or list the admin in `EUNOMIA_ADMIN_EMAILS`: while the install is empty only a provider-verified sign-in with a listed address can become the first user. Later, a verified sign-in with a listed address is always allowed to create its account (operators exist through the provider), whatever `SIGNUP` says; an unverified one is refused.

**Notes.** A public provider (Google, a shared AuthKit tenant) lets anyone with an account sign in: set `SIGNUP=invite` or `closed`, or restrict who the provider application is assigned to. The provider's discovery, JWKS, token and userinfo endpoints must be https (http only for loopback); requests have a 10 second timeout, never follow redirects and are capped at 1 MiB. If the ID token has no email, the userinfo endpoint is asked (its `sub` must match). Sign-in attempts at the callback share the per-address `RATE_LIMIT_AUTH_PER_MIN` limit.

### Upload and MCP body handling

Both the bundled Caddy proxy and a direct `:3000` install reach the same Next route handlers for `/api/documents` and `/mcp`. These paths bypass `src/proxy.ts`, because Next clones request bodies before that proxy runs. A body-bearing request first checks credentials with the fixed internal backend, without reading the incoming body. Both use a bodyless `HEAD` on their actual backend route, which authenticates and returns the scope-bound size limit. OAuth access tokens are accepted only at `/mcp`. These checks are uncached. Only a 200 response permits streaming the original body. Missing, fake, expired or revoked credentials fail before the body is read. Backend failures fail closed. The backend still authenticates and authorises the actual request and enforces its upload size and concurrency limits.

All other Next proxy routes have a 1 MiB cloning ceiling. Large uploads do not require raising that ceiling or configuring a bundled reverse proxy. The streaming handlers apply the same forwarding-header policy as `src/proxy.ts`. If you supply your own reverse proxy, keep request buffering disabled there, for example `proxy_request_buffering off` for nginx, so it does not spool an unauthenticated upload before Next can refuse it. Bundled Caddy streams requests without a request-buffer configuration.

## 3. The browser only ever talks to the frontend

The frontend calls same-origin `/api/*`, and `frontend/next.config.ts`
rewrites that to `BACKEND_INTERNAL_URL` (`http://backend:8001` in the
image, the compose service name). The browser never needs the backend's
host, so one prebuilt image works whether you open it as `localhost`, a LAN
IP, or a domain -- no rebuild, no CORS. Put your reverse proxy / TLS in
front of the frontend port only.

`NEXT_PUBLIC_API_URL` still exists as a build-time override for running the
frontend somewhere it can't reach the backend over a private network; it is
inlined at `bun run build`, so changing it requires rebuilding the image.

## Updates

Settings → Updates shows the running release and, when a newer one is
published, **Update now**. A sidebar badge appears when an update is waiting.
**Check now** asks GitHub immediately instead of waiting for the 10-minute
check. Only an instance admin (the first user, or an email in `EUNOMIA_ADMIN_EMAILS`) can use **Update now** and **Check now**: the update restarts the stack. After you click, the page shows progress, Eunomia restarts on the new
release's prebuilt images (a few seconds of downtime, no build), and the page
reloads itself. Data lives in the `eunomia-surreal-data-v3` volume (installs
from before 2.0 keep their old SurrealDB 2.x data on `eunomia-surreal-data`, see
[upgrading-to-surrealdb-3.md](upgrading-to-surrealdb-3.md)) and isn't touched.

How: the stack includes a small `updater` service (`scripts/updater.sh`) that
runs `scripts/auto-update.sh` every 20 seconds. It works the same on every
install and OS, with nothing to schedule on the host. The web-facing backend
never gets git or the docker socket: it only drops a marker file in
`update-status/`. The updater has the socket but no ports, and its only input
is that marker. The worst a compromised backend can do is ask for the newest
release tag. The script checks out the tag, pins `EUNOMIA_IMAGE_TAG` in `.env`,
pulls and restarts, and appends to `update-status/history.log`. It refuses
(and says so in Settings) if tracked files like `docker-compose.yml` have
local edits. A lock in `update-status/` keeps an old host cron job and the
container from ever running at once. A restart that fails while the backend
is still on a slow first boot (moving data, running migrations) is retried
for up to 30 minutes, with the progress in Settings, before it is reported as
failed. On every run the updater also starts any service left `Created` or
stopped once the backend is healthy, so a half-finished restart never leaves
the site down.

Installs from before the updater existed: run the installer once more from
the folder that contains your install. It updates in place and keeps your
data.

## Org databases and the one-time move

Each org's data lives in its own database, `org_<uuid>`, in the `SURREAL_NS` namespace; accounts, credentials and the job queue live in the `control` database. See [architecture/tenancy.md](architecture/tenancy.md). The first boot of a release with tenancy on an existing install moves the old single database (`SURREAL_DB`, default `eunomia`) into that layout automatically, copying every table in batches and refusing to finish unless the row counts match. A row the new schema refuses is skipped, not copied: the log lists each one (table and record id) and the final log line and a `legacy.move` audit event give the count per table. Nothing in the old database is changed or deleted; the backend logs the `REMOVE DATABASE` command to use once you have checked the app. If the boot stops with "the data move did not verify", nothing is served from the half-moved org; fix the cause and restart (the move resumes). Take an export first, as the upgrade scripts do.

`ENCRYPTION_KEY` now also protects the per-org database passwords: keep it.

Saved credentials are stored as `enc:v1:...` ciphertext; older rows without the prefix still
decrypt. A value that cannot be decrypted (a changed `ENCRYPTION_KEY`, corrupted data, or a
deployment that used to run without a key) is never sent anywhere: the feature reports an error and
the credential has to be entered again in Settings or Connectors. Keep `ENCRYPTION_KEY` with your
backups.

### Rotating ENCRYPTION_KEY

Older installs ran with an empty `ENCRYPTION_KEY`, so their stored connector credentials, OpenAI keys and org database passwords were encrypted under a public all-zero key, and the control database password is derived from the key. The backend refuses to boot on an empty key (unless `EUNOMIA_ALLOW_EMPTY_ENCRYPTION_KEY=1`, dev only), whether or not the install already holds data: with it, every database credential would be computable from public information. "Update now" and the `surreal-upgrade` step that runs on every `docker compose up` of a 1.x install write a key and `ENCRYPTION_KEY_LEGACY_EMPTY=1` into `.env` before this release's backend starts, so an install updated that way needs nothing; otherwise step 1 below is the fix:

1. Set `ENCRYPTION_KEY` (`openssl rand -base64 32`) **and** `ENCRYPTION_KEY_LEGACY_EMPTY=1` in `.env`, then restart the backend. With the flag on, a value that does not decrypt under the new key is tried under the old empty one, and the log notes each time that happens. Every new write uses the new key.
2. At that boot the control database user gets a password derived from the new key, and every org database password that was stored under the empty key is **replaced** (a new password, derived from the new key, set on the org's database user and stored under the key; the old one stops working), because the old one was readable by anyone who could reach the control database. The log says how many were replaced. Connector credentials and OpenAI keys are re-encrypted when you save them again: reconnect each connector (or re-enter its credentials) and re-enter the OpenAI key in Settings.
3. When the log no longer shows the "decrypted a value written with the empty ENCRYPTION_KEY" line over a few days of normal use, remove `ENCRYPTION_KEY_LEGACY_EMPTY` from `.env`.

An install whose existing key is shorter than 16 characters keeps working: it boots with a loud error in the log and `/readyz` carries a `warning`, because refusing would lock the owner out of data written under it (a new install with a short key is refused). Moving to a strong key is a key change, so the next paragraph applies: nothing re-encrypts for you.

Changing a real key later has no such fallback: values written under the old key stop decrypting, so reconnect the connectors, re-enter the OpenAI key, and expect org databases to be unreachable until their users are redefined (see [architecture/tenancy.md](architecture/tenancy.md)). Do not change it casually. Two-step verification secrets are encrypted under the key too: after a change, an authenticator code answers `auth.mfa_unreadable` (409, never a 500, no attempt counted, audited as `auth.mfa_secret_unreadable`). The person signs in with a recovery code, turns two-step verification off in Settings with another recovery code (allowed even while it is required) and sets it up again. With `ENCRYPTION_KEY_LEGACY_EMPTY=1` a secret written under the empty key still decrypts. See [security/mfa.md](security/mfa.md).

Erasure records are keyed hashes under the key, and a hash cannot be re-keyed. After any key change, set `ENCRYPTION_KEY_PREVIOUS` to the old key (comma separated for several) for as long as backups from before the change exist, so a restored account that was erased is still recognised and erased again ([privacy/subject-rights.md](privacy/subject-rights.md#resync-and-restore-suppression)).

## Backups

See [INSTALLATION.md](../INSTALLATION.md#backups): the nightly encrypted export, where the key lives, and how to restore.

## Support diagnostics

See [Getting help](support.md) to download a diagnostics zip from Settings, or
collect logs from the command line when the web app is down.

## Readiness evidence for self-hosters

Read the [readiness catalogue](compliance/README.md), [shared responsibilities](compliance/responsibilities.md) and [applicability notes](compliance/applicability.md) before planning an assessment. Done, Partial and Missing describe source implementation, not your installation's operation. Record host settings, access reviews, restore drills and unknowns separately. The customer-selected model data path is excluded and assumed encrypted; Eunomia's own storage, ordinary transport and maker AI governance remain in scope. Policy placeholders belong to the separate organisational track.


### Upload transfer budgets

HTTP document uploads, MCP request bodies and the Next streaming handlers use the same policy: 15 seconds of grace, then a minimum average of 64 KiB/s over a sliding 10-second window. A stopped sender is refused with 408 within about 26 seconds of its last data, even after a large initial burst. Rate samples use one-second buckets and include the boundary bucket to allow for scheduling jitter. The first rate check is after grace plus the window. Each body also has a total ceiling of `min(15 minutes, 15 seconds + transport limit / 64 KiB/s)`, rounded up to a second. Using the authenticated transport limit also covers chunked bodies that have no declared size. MCP's limit includes its JSON envelope and base64 overhead. A rejected or cancelled backend read releases its large-body slot.

For example, a 25 MiB body at 1 Mbit/s takes about 210 seconds, which fits this policy. A sender below 64 KiB/s must use a faster connection. Next can shorten the grace and window with `EUNOMIA_UPLOAD_GRACE_MS` and `EUNOMIA_UPLOAD_WINDOW_MS`, useful for fast isolated proxy tests. These settings never extend production limits. The old `EUNOMIA_UPLOAD_BODY_TIMEOUT_MS` setting is retired.

Use `bun run dev` or `bun run start`. These launch `scripts/server.mjs`, which sets the incoming HTTP request timeout to 960 seconds. Running `next start` directly would retain Node's five-minute incoming-body timeout. The Next route execution allowance and rewrite proxy timeout are also 960 seconds. Streaming fetch uses a dedicated Undici dispatcher with no header deadline during upload, rather than its default five-minute deadline. The Node dispatcher keeps a five-second connection limit. Bun uses an explicit `timeout: false` fetch option so its idle timer also defers to the body budget. See [Undici's timeout options](https://undici.nodejs.org/api/Dispatcher) and [Bun's fetch timeout option](https://bun.com/reference/globals/BunFetchRequestInit/timeout).

The bundled Caddy configuration has no shorter body or upstream deadline and streams requests, as described in [Caddy's proxy settings](https://caddyserver.com/docs/caddyfile/directives/reverse_proxy). For custom nginx installations, set `proxy_request_buffering off`, `client_body_timeout 960s`, `proxy_send_timeout 960s` and `proxy_read_timeout 960s`. Other ingress services and hosting platforms must also allow at least 960 seconds. The application still enforces its rate and size budgets.

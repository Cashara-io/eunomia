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
| `CORS_ALLOWED_ORIGINS` | `http://localhost:3000` | only needed for browsers calling the backend port directly; the bundled frontend goes through its own `/api` proxy |
| `FRONTEND_URL` | `http://localhost:3000` | same as above (this is what `CORS_ALLOWED_ORIGINS` defaults from in `docker-compose.yml`) |
| `SESSION_TTL_DAYS` | `30` | Browser sessions expire this many days after last use (extended at most once an hour). The session cookie itself also carries a hard 90 day limit. |
| `RATE_LIMIT_USER_PER_MIN` | `1200` | Requests per minute per signed-in user. `0` turns the limit off. Over the limit the API answers 429 `rate.limited` with `Retry-After`. |
| `RATE_LIMIT_TOKEN_PER_MIN` | `600` | Requests per minute per API token (a user's limit applies as well). |
| `RATE_LIMIT_AUTH_PER_MIN` | `20` | Login, signup, OAuth token and failed-credential attempts per minute per client address (read from `X-Forwarded-For`, but only when the connecting peer is in `TRUSTED_PROXIES`). |
| `RATE_LIMIT_WEBHOOK_PER_MIN` | `120` | Source webhook deliveries per minute per client address. |
| `MAX_REQUEST_BODY_BYTES` | `1048576` | Largest request body any route accepts (413 over it). Raise it only if a route you use needs more. |
| `TRUSTED_PROXIES` | loopback only (`127.0.0.0/8`, `::1/128`); `docker-compose.yml` sets `frontend` | Comma-separated proxies whose `X-Forwarded-For` is believed: CIDRs, bare addresses or hostnames. A hostname (the compose service `frontend`) is resolved at boot and again every 60 seconds; the last good answer is kept and failures are logged at warn. The right-most address that is not itself trusted is the client. A LAN or bridge peer that is not listed cannot spoof its address. Only if you point a proxy straight at the backend (not recommended, see section 1): add its address or name (`TRUSTED_PROXIES=frontend,203.0.113.5`). `none` trusts nobody (every client is its peer address). |
| `PUBLIC_URL` | `http://localhost:8001` | The address MCP clients reach Eunomia at, no trailing slash, e.g. `https://eunomia.example.com`. Used in OAuth discovery documents and as the audience of OAuth tokens, so it must match what clients connect to. |
| `SIGNUP` | `open` | Who can create an account once the install has a first user (the first user can always sign up). `open`: anyone who can reach the server. `invite`: only emails listed in `SIGNUP_ALLOWLIST` (comma separated). `closed`: nobody. A typo closes signup. The default is `open` because a vault invitation needs an existing account (`no user with email`), so a team's second member has to be able to sign up before anyone can invite them. **On an instance reachable by people you do not trust, set `SIGNUP=invite` or `closed` after your team has signed up.** The same gate applies with `EUNOMIA_SIGNUP_ORG=personal`. Once the install has users, an address listed in `EUNOMIA_ADMIN_EMAILS` can never sign up (so nobody can claim an operator address first): register the account, then list it. |
| `SIGNUP` + `ALLOW_PRIVATE_LLM_URL` | see above | Defaults `open` and allowed together: anyone who can reach the server can sign up and point a model URL at a private address. The backend logs one warning at boot when both are in effect. Set `SIGNUP=invite` or `ALLOW_PRIVATE_LLM_URL=0` on a reachable instance. |
| `SIGNUP_ALLOWLIST` | empty | Emails allowed to sign up when `SIGNUP=invite`. |
| `COOKIE_SECURE` | `auto` | `auto`: the session cookie is `Secure` when `PUBLIC_URL` is https or a trusted proxy (`TRUSTED_PROXIES`) sent `X-Forwarded-Proto: https`; the browser usually talks to the frontend origin, which can be https while `PUBLIC_URL` is not. `true` / `false` force it. |
| `CAPSULE_ARGS` | `off` | `off`: failure capsules keep only a hash and the JSON shape of a failed call's arguments. `redacted`: keep the arguments with secrets masked, so `eunomia replay` can re-run them. See [debugging](debugging.md). |
| `AUDIT_RETENTION_DAYS` | `365` | The `prune_auth` job deletes audit events older than this. `0` keeps them forever. |
| `SESSION_RETENTION_DAYS` | `7` | The same job deletes browser sessions that expired (or were revoked) this many days ago. |
| `RATE_LIMIT_REFRESH_PER_MIN` | `300` | OAuth refresh-token grants per minute per client and address, apart from `RATE_LIMIT_AUTH_PER_MIN` (which still covers code exchange, registration, login and signup). Many users behind one address refresh through the same app. |

A verified API token, OAuth access token or session cookie is remembered by the backend process for 20 seconds, so a busy credential costs no database lookup per request (never past the credential's own expiry). Revoking it through this process (Settings, `/oauth/revoke`, logout) takes effect at once; with several backend replicas, the others may honour it for up to those 20 seconds. A token's "last used" time is written at most once a minute.

Failed sign-ins are also limited per account and client address: after 10 failures in 15 minutes from one address that email gets 429 `rate.limited` from that address until the window passes (a success clears the count), so a stranger cannot lock a known account out for everyone. A looser ceiling of 100 failures per account across all addresses stops guessing spread over many addresses. OAuth token requests from a registered client get the same treatment after 30 failures; clients identified by a public metadata URL (CIMD) rely on the per-address limit instead, so a stranger cannot lock out every user of that app. Both counters live in the backend process and hold at most 50,000 keys (the oldest windows are dropped first).

| `ALLOW_PRIVATE_LLM_URL` | `1` | Whether a user's OpenAI-compatible base URL (Settings) may point at a private or loopback address, as a local model server such as Ollama needs. Link-local addresses (cloud metadata such as 169.254.169.254) and the compose services `surrealdb` and `backup` are always refused. Set `0` on an install shared with people you do not trust. The check runs when the URL is saved and again on every model call (hostnames are re-resolved at connect time, redirects are not followed). Remaining limits: the `surrealdb` address is resolved once per process, and only `0` keeps requests off other private services. |
| `EUNOMIA_ALLOW_CONNECTOR_BASE_URL` | `0` | Whether a connector's `config.base_url` (and OAuth `token_url`) is honoured, to point a connector at a self-hosted provider such as GitHub Enterprise. Off by default: on a shared server it would let any user choose which hosts the backend calls. Even when on, requests use the same outbound guard as model calls (metadata and the database refused, no redirects). See [connectors](connectors.md). |
| `EUNOMIA_SIGNUP_ORG` | `join` | `join`: a new user joins the install's one org (the first user of a fresh install creates it and owns it). `personal`: every signup gets an org of their own. |
| `EUNOMIA_ORG_POOL_CAP` | `256` | How many org database sessions one process keeps open (least recently used are dropped and signed in again on demand). |
| `EUNOMIA_DB_CONNECTIONS` | `8` | WebSocket connections one process opens to SurrealDB. Every database handle is signed in on each and statements take them in turn: SurrealDB works through one connection's requests largely one at a time, so a single connection caps the process at roughly one database core. More connections means more parallel queries (and one sign-in per connection the first time an org is used). |
| `BACKUP_ENCRYPTION_KEY` | none (the `backup` service generates one, see below) | Encrypts nightly backups. Recommended: generate it before the first `docker compose up` (`openssl rand -base64 32`), put it in `.env`, and keep a copy off the machine, since backups cannot be read without it. The auto-updater adds one when it updates an existing install. If it is still empty, the `backup` service creates a key on first start, saves it to `/backups/.backup-key` in its own volume and logs a one-time "COPY IT SOMEWHERE SAFE" notice. Tradeoff: that key lives in the same volume as the backups, so someone who copies the volume gets both and the encryption protects nothing against them; losing the volume loses the key too. It only guards against someone who sees the backup files without the volume's key file. Setting the key in `.env` keeps the two apart. |

To restore, run `backend/scripts/restore.sh <backup name or file> [--wipe]` from the repo root. A backup of an install with org databases is a directory holding one file per database; each one is restored into its own database. The import needs more than the 60 second query and transaction limits of the hardened SurrealDB, so the script stops the backend, restarts SurrealDB with `docker-compose.import.yml` (the same flags minus those two limits), restores, and then recreates the hardened server and starts the backend again, even if the restore fails. The app is down meanwhile. `docker compose exec backup eunomia-backup restore ...` also works but imports into the hardened server, so a large database can hit the transaction limit. A nightly backup stops with an error (and writes nothing) when it cannot list the databases, rather than saving only the old single database.

Backup files are encrypted with AES-256-CBC and authenticated with an HMAC-SHA256 over the ciphertext, checked before anything is decrypted or imported, so a modified file is refused. Backups written by earlier versions (no authentication) still restore.

Each database export is checked before it becomes a backup: the server is asked for every table's row count just before and just after the export, the records in each table's `INSERT` statements of the dump are counted, and the two must agree (and the server must still answer afterwards). A table nobody wrote to during the export must match its count exactly; one written meanwhile must lie between its two counts, and if it does not, the database is exported again (at most 3 times). An export cut short by a dying server (for example an out-of-memory kill), or one missing rows for any other reason, is refused: the run fails with a `does not hold the rows the server counted: <table>(<n> rows counted, <m> in the export)` log line, writes no file and rotates nothing out. A database with no rows is a valid backup.

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
container from ever running at once.

Installs from before the updater existed: run the installer once more from
the folder that contains your install. It updates in place and keeps your
data.

## Org databases and the one-time move

Each org's data lives in its own database, `org_<uuid>`, in the `SURREAL_NS` namespace; accounts, credentials and the job queue live in the `control` database. See [architecture/tenancy.md](architecture/tenancy.md). The first boot of a release with tenancy on an existing install moves the old single database (`SURREAL_DB`, default `eunomia`) into that layout automatically, copying every table in batches and refusing to finish unless the row counts match. Nothing in the old database is changed or deleted; the backend logs the `REMOVE DATABASE` command to use once you have checked the app. If the boot stops with "the data move did not verify", nothing is served from the half-moved org; fix the cause and restart (the move resumes). Take an export first, as the upgrade scripts do.

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

Changing a real key later has no such fallback: values written under the old key stop decrypting, so reconnect the connectors, re-enter the OpenAI key, and expect org databases to be unreachable until their users are redefined (see [architecture/tenancy.md](architecture/tenancy.md)). Do not change it casually.

## Backups

See [INSTALLATION.md](../INSTALLATION.md#backups): the nightly encrypted export, where the key lives, and how to restore.

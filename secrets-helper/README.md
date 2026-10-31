# secrets-helper

The credential broker's sidecar (`docs/architecture/specs/broker.md`, `docs/password-managers.md`). The
backend calls it over the internal compose network to resolve `op://` and `bws://` references in the
org's own 1Password (Connect server or service account) or Bitwarden Secrets Manager. The backend never
links a provider SDK; this service does the provider calls and nothing else.

- Stateless: the provider token comes with each request (the backend stores it encrypted) and is gone
  when the request ends. No volume, no config file, no token in the environment or the logs.
- Internal API v1: `GET /healthz`, `POST /v1/resolve`, `POST /v1/check` (see the spec). Calls must carry
  `Authorization: Bearer $SECRETS_HELPER_KEY` when the key is set.
- 1Password Connect: the Connect REST API on the customer's server. Service accounts: the official
  `@1password/sdk` (MIT, a WASM core, runs headless). Bitwarden Secrets Manager: a small client of
  Bitwarden's public identity and API endpoints (`src/providers/bitwarden*.ts`), not the Bitwarden SDK,
  whose licence is not open source.

```sh
npm ci && npm test      # unit tests, and the mock Connect and Bitwarden servers through the real server
npm run build && SECRETS_HELPER_KEY=dev node dist/src/main.js   # listens on :8080
```

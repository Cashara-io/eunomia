# Secret requests: keys without the chat

Agents often need a password, an API key or a token. Pasting it into the chat puts it in the
conversation, the model's context and every log on the way. With secret requests, the agent asks
Eunomia for the key instead: you paste it into a small, masked pop-up in the Eunomia web app, and the
agent gets back only a **handle**, such as `{"$secret": "eu://8c0f.../stripe_api_key"}`. It can then use
the key, and build things from it (a `Bearer` header, a Basic auth header, an HMAC signature, a JWT),
without ever seeing it.

Design and threat model: `docs/architecture/specs/secret-requests.md`.

## What you see

1. Your agent calls `secret_request` (for example "Stripe secret key, to list this month's payouts, for
   api.stripe.com").
2. In the Eunomia web app, a pop-up opens within a few seconds on whatever page you are on. If the app is
   not open, the agent shows you a link: `https://<your Eunomia>/secrets/requests/<id>`. MCP clients that
   support it (protocol 2025-11-25, URL elicitation) open the link for you.
3. The pop-up shows who asked (the token's or app's name, and whether over MCP, the API or the web
   chat), the agent's reason (in its own words: read it with care), the vault it goes into, the handle
   the agent will get, and the hosts it may be sent to.
4. Paste the key, adjust the hosts and where in a request it may go (headers by default; query string
   and body only if you tick them), and choose **Save**. **Decline** tells the agent no; **Later** hides it
   for now.

By default only the agent credential that asked can use the key. Tick "Let any of my agents in this
vault use it" to share it with your other tokens and apps.

Eunomia never asks for a secret in chat or on another site. Paste one only on a page of your own
Eunomia address.

## What happens to the key

- It is encrypted (AES-256-GCM under `ENCRYPTION_KEY`) before it is stored, in your organisation's
  database. **Nobody can read it back**: not the agent, not you, not an owner. You can replace it or delete
  it under **Connectors, Secrets agents asked for**.
- When an agent uses the handle, Eunomia decrypts it for that one call, sends it only to a host you
  approved, and takes it (and its base64, URL-encoded and hex forms) out of whatever comes back before the
  agent sees it.
- Every request, fill, decline, use, transform and outbound call is in the resolution log on the
  Connectors page, with who, which handle and the outcome. Never the value. Organisation owners and
  instance admins see every row; a vault owner sees the rows of the vaults they own.
- Held secrets do not need the secrets helper (the sidecar for 1Password and Bitwarden Secrets Manager,
  which is a preview). With password managers switched off, requests, fills, transforms and
  `http_request` with `eu://` handles keep working; only `op://` and `bws://` references answer
  `secret.unavailable` with reason `helper_disabled`.
- Replacing or deleting a key also deletes everything derived from it.

## For agents

```json
{"name": "stripe_api_key", "label": "Stripe secret key", "description": "To list this month's payouts",
 "target": {"hosts": ["api.stripe.com"]}}
```

`secret_request` returns `request_id`, `status`, `fill_url` and `handle`. Poll `secret_request_status`
until it is `filled`. Then:

```json
{"op": "template", "inputs": {"key": "eu://8c0f.../stripe_api_key"}, "params": {"template": "Bearer {{key}}"}, "name": "stripe_bearer"}
```

with `secret_transform` gives a new handle, and

```json
{"url": "https://api.stripe.com/v1/payouts", "headers": {"Authorization": {"$secret": "eu://8c0f.../stripe_bearer"}}}
```

with `http_request` makes the call. Transforms: `base64`, `base64url`, `url_encode`, `template`,
`basic_auth`, `hmac_sha256`, `sha256`, `jwt_hs256`, `jwt_rs256`, `jwt_es256`. A derived key may only go
where every key it came from may go. There is no transform that decodes, compares or reveals a key.

`http_request` only calls `https` hosts you approved, never private, loopback or cloud metadata
addresses, follows no redirects, and needs at least one handle (it is not a general web fetch).

## Who can do what

- Anyone whose agent can write a vault can ask for a secret in it; a token restricted to one vault only
  asks into that vault.
- Only the person whose agent asked can fill the request, from the browser. Owners of the vault and of
  the organisation can decline it, and can replace or delete held secrets in their vaults.
- Another organisation never sees your requests or secrets.

## Limits

- A key is 8 to 16384 characters (shorter ones cannot be scrubbed from text safely).
- At most 10 requests waiting per person; a request waits 60 minutes unless the agent asks for longer (at
  most a week).
- 1Password and Bitwarden Secrets Manager (preview) references (`op://`, `bws://`) cannot be sent by
  `http_request` yet: they have no approved hosts.
- `eu://` is not a password manager: it is not one of the provider kinds (`GET /api/secrets/kinds`) and
  cannot be registered as a reference under Password managers.

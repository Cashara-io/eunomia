# Password managers (1Password and Bitwarden)

Eunomia can let a tool use a secret that stays in your organisation's own password manager. Eunomia
stores only a **reference** to the secret, such as `op://Ops/Square/access_token`. When a tool call
names that reference, Eunomia asks your password manager for the value, hands it to that one tool
call, and logs that it did. The value is never saved: not in memory, skills, the audit log, failure
reports, the change log or the server's logs.

Supported:

- **1Password Connect**: a Connect server you run, with a Connect token.
- **1Password service accounts**: a service account token (it starts with `ops_`).
- **Bitwarden Secrets Manager**: a machine account access token (it starts with `0.`), on Bitwarden's
  US or EU cloud or a self-hosted server.

Design and threat model: `docs/architecture/specs/broker.md`.

## Who can set it up

An owner of the organisation or an instance admin (the install's first user, or an address in
`EUNOMIA_ADMIN_EMAILS`), signed in through the browser. Everyone else sees that only an owner or admin
can manage password managers.

## 1. Connect a password manager

Open **Connectors**, scroll to **Password managers** and choose **Set up** on the one you use.

### 1Password Connect

1. In 1Password, create a Connect server and an access token that can read the vaults you need
   ([1Password's guide](https://developer.1password.com/docs/connect/get-started/)). Run the Connect
   server where the Eunomia host can reach it.
2. In Eunomia, enter the **Connect server URL** (for example `https://connect.example.com:8080`) and the
   **Token**, then **Save**.
3. Choose **Test connection**. It should say how many vaults the token can see.

### 1Password service account

1. In 1Password, create a service account with read access to the vaults you need
   ([1Password's guide](https://developer.1password.com/docs/service-accounts/get-started/)) and copy
   its token.
2. In Eunomia, paste the **Token** and **Save**, then **Test connection**.

### Bitwarden Secrets Manager

1. In Bitwarden Secrets Manager, create a machine account, give it access to the projects you need, and
   create an access token for it ([Bitwarden's guide](https://bitwarden.com/help/access-tokens/)).
2. In Eunomia, paste the **Token**. Leave **Server URL** empty for Bitwarden's US cloud, enter
   `https://vault.bitwarden.eu` for the EU cloud, or your own server's address if you self-host.
3. **Save**, then **Test connection**. It should say it signed in as the machine account.

The token field is write-only. The token is encrypted (AES-256-GCM under the server's
`ENCRYPTION_KEY`) before it is stored and is never shown again, not even to an admin. **Replace** saves
a new one; **Remove** deletes it. A token change applies to the next tool call.

## 2. Register references

A tool can only use a reference that an owner or admin has registered. Under **References**, choose
**Add reference** and fill in:

- **Reference**: `op://<vault>/<item>/<field>` or `op://<vault>/<item>/<section>/<field>` for 1Password
  (copy it with **Copy Secret Reference** in the 1Password app), or `bws://<secret id>` for Bitwarden
  (the secret's id, shown in Secrets Manager).
- **Resolved by**: which of your password managers holds it.
- **Bound to vault**: only members of this Eunomia vault can use it. A token restricted to another vault
  cannot use it.
- **Allowed tools**: the tools that may receive the value. `*` allows any tool that takes secrets. Empty
  allows none.
- **Allowed people or tokens** (optional): `user:...`, `api_token:...` or `oauth_client:...` ids. Empty
  allows any member of the vault.

The table shows how often each reference was used in the last 7 days and how often it was refused.

## 3. Use a reference in a tool call

Pass the reference as an object with one key, `$secret`, wherever the tool expects the value:

```json
{"api_key": {"$secret": "op://Ops/Square/access_token"}}
```

If anything stops the value from being used (the reference is not registered, not allowed for this
tool, person or vault, not found in the password manager, or the password manager cannot be reached),
the tool does not run and the call fails with `secret.unavailable` (`docs/errors.md`). There is never a
fallback to a stored value.

The built-in memory, entity, skill, document and vault tools do not take secrets, because they store
their arguments. The first tools that take secrets are connector tools that call an outside service.

## 4. The resolution log

**Resolution log** lists every request for a secret: when, which reference, which tool, who (the person,
API token or OAuth client) and through which channel, and the outcome: resolved, denied, not found or
provider error. It never shows the value. Admins read it over the API too: `GET /api/secrets/log` and
`GET /api/secrets/counts`.

## How it runs

The password manager calls are made by a small service, `secrets-helper`, that runs next to the backend
in Docker Compose. It publishes no port and only the backend can reach it. It keeps nothing: the backend
sends the token with each request, and nothing is written to its disk or its logs. Updates install it
like the other services. `SECRETS_HELPER_KEY` in `.env` is the key the two share; the updater adds one if
it is missing (`docs/deployment.md`).

## What it protects against, and what it does not

- A copy of the database or a backup does not contain any value, and the token only as ciphertext.
- The value is removed from the tool's result before it goes back to the agent, and from anything the
  tool writes to memory in the same call. Known token shapes (including 1Password service account and
  Bitwarden access tokens) are redacted everywhere else, as for any secret an agent pastes.
- A tool that is allowed to receive the value is trusted with it: that is the point. Allow only the tools
  that need it.
- A value the tool changes (for example encodes) before returning it is not recognised by the exact
  match, only by the shape detectors.

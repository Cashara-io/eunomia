# Extensions

Your own scripts (Python, JS, anything that speaks HTTP) can feed Eunomia: scraping, recurring jobs, bulk imports. This is **phase 1**: Eunomia gives an extension a registry entry, a webhook inbox that buffers deliveries, and an API to take them in batches. It does **not** run your code. The script runs where you run it (your machine, a GitHub Action, cron, a container) and talks to Eunomia over HTTP with an API token. Hosting code is phase 2, below.

Code: `backend/src/extensions.rs` (service), `backend/src/routers/extensions.rs` (HTTP), `backend/src/store/extensions.rs` (statements), migration `tenant/0020_extensions.surql`. UI: Connectors, Extensions. Issue: #91.

## How it fits together

```
sender --POST /hooks/ext/{key}--> inbox (buffer) --claim--> your script --tool calls--> memory
 (webhook, cron,                  per extension    ack/nack   (anywhere)
  another service)
```

1. Create an extension (Connectors, Extensions, or `POST /api/extensions`). You get a **webhook URL** and a **secret**. The secret is shown once; rotate it for a new one.
2. Anything that can POST JSON sends to the URL. Each delivery is stored as an event.
3. Your script **claims** a batch, does its work (typically `memory_write` calls), then **acks** what succeeded and **nacks** what did not. A scheduled job reads the buffer the same way: it just claims whatever has piled up since its last run.

## Sending to the inbox

`POST {PUBLIC_URL}/hooks/ext/{key}` with a JSON body of at most 256 KiB. Prove you hold the secret in one of two ways:

```sh
# 1. the secret as a bearer token
curl -X POST "$URL" -H "Authorization: Bearer $SECRET" -H 'Content-Type: application/json' -d '{"name":"Ada","note":"likes tea"}'

# 2. a signature over the exact body: X-Eunomia-Signature: sha256=<hex HMAC-SHA256, keyed with the secret>
BODY='{"name":"Ada","note":"likes tea"}'
SIG=$(printf '%s' "$BODY" | openssl dgst -sha256 -hmac "$SECRET" -hex | sed 's/^.* //')
curl -X POST "$URL" -H "X-Eunomia-Signature: sha256=$SIG" -H 'Content-Type: application/json' -d "$BODY"
```

Prefer the signature when the sender can compute it: the secret never travels, and a captured request cannot be altered. If a signature header is present it decides; a bad one is not rescued by a good bearer.

A sender's own `X-Delivery-Id`, `X-GitHub-Delivery` or `Idempotency-Key` header names the delivery: sending the same id again stores nothing and answers `{"status":"duplicate"}`. Without one, every delivery is new.

| Answer | Meaning |
|---|---|
| `202 {"status":"accepted","id":"extension_event:..."}` | Buffered |
| `401 auth.unauthorized` | Wrong secret or signature, unknown extension, malformed URL. These look identical on purpose |
| `403 extension.disabled` | Authentic, but the extension is switched off |
| `400 validation.invalid` / `413` | Not JSON, or over 256 KiB |
| `429 extension.buffer_full` | The inbox holds its maximum of unprocessed events; retry after the consumer has acked some |
| `429 rate.limited` | The per-address webhook limit (`RATE_LIMIT_WEBHOOK_PER_MIN`, default 120) |

## Taking events out

These routes take a normal personal API token. Claim, ack and nack need `memory:write`; listing the inbox needs `memory:read`; managing extensions (create, edit, rotate, delete) needs the `connectors` scope. A vault-restricted token cannot use them: an extension belongs to the account.

| Route | Does |
|---|---|
| `POST /api/extensions/{id}/events/claim` `{max: 1..500, lease_secs: 5..3600}` | Leases the oldest claimable events (defaults 100 and 300 s) and returns them with the extension's `vault_id` (null is your personal vault). Empty `events` means nothing is waiting |
| `POST /api/extensions/{id}/events/ack` `{ids}` | Marks leased events done |
| `POST /api/extensions/{id}/events/nack` `{ids, error?}` | Gives them back at once, with a reason |
| `GET /api/extensions/{id}/events?status=dead` | Looks at the inbox without leasing, newest first (for dead letters) |

Semantics:

- **At least once.** An event is leased to one consumer at a time. If the lease runs out without an ack (the script crashed, or was slow), the next claim hands it out again. Do your work idempotently, or keep the event's `delivery_id` to skip repeats.
- **Attempts.** Each claim counts. An event handed out five times without an ack is dead-lettered (`dead`, kept 30 days, visible in the list and `?status=dead`). Nack on the fifth attempt also kills it.
- **Order.** Oldest first within a claim; there is no ordering guarantee across a retry.
- **A batch** stops at about 8 MiB of bodies, so `events` can be shorter than `max`. Loop until it comes back empty.
- Acking an event that is not leased (already done, never claimed, someone else's) counts nothing; `acked` in the answer says how many did.

## Example consumer

Standard library only. It claims a batch, writes one fact per event, acks the ones that worked and nacks the rest, and repeats until the inbox is empty. The senders post `{"name": "...", "note": "..."}`.

```python
#!/usr/bin/env python3
"""Drain an Eunomia extension inbox: claim a batch, write each event as a fact, ack."""
import json, os, urllib.error, urllib.request

BASE = os.environ.get("EUNOMIA_URL", "http://localhost:8001").rstrip("/")
TOKEN = os.environ["EUNOMIA_TOKEN"]        # a personal API token with memory:write
EXT = os.environ["EUNOMIA_EXTENSION"]      # the extension id, extension:...


def post(path, body):
    req = urllib.request.Request(
        BASE + path, json.dumps(body).encode(),
        {"Authorization": f"Bearer {TOKEN}", "Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=60) as resp:
        return json.load(resp)


def handle(event, vault_id):
    """One event, one fact. The shape of event["body"] is whatever your sender posts."""
    body = event["body"]
    args = {"subject_name": body["name"], "subject_kind": "person", "text": body["note"]}
    if vault_id:
        args["vault_id"] = vault_id
    post("/api/tools/memory_write", args)  # once memory_write_batch lands (#87): one call per 100 events


claimed = acked = 0
while True:
    batch = post(f"/api/extensions/{EXT}/events/claim", {"max": 100, "lease_secs": 300})
    if not batch["events"]:
        break
    ok, bad = [], []
    for event in batch["events"]:
        try:
            handle(event, batch["vault_id"])
            ok.append(event["id"])
        except (urllib.error.URLError, KeyError, TypeError) as err:
            bad.append((event["id"], str(err)))
    if ok:
        post(f"/api/extensions/{EXT}/events/ack", {"ids": ok})
    for event_id, err in bad:
        post(f"/api/extensions/{EXT}/events/nack", {"ids": [event_id], "error": err})
    claimed, acked = claimed + len(batch["events"]), acked + len(ok)
    print(f"batch of {len(batch['events'])}: {len(ok)} acked, {len(bad)} nacked")
print(f"done: claimed {claimed}, acked {acked}")
```

Run it from a schedule. A GitHub Actions workflow every 15 minutes (the shortest cron GitHub honours is 5 minutes, and runs can start late; the Eunomia URL must be reachable from GitHub's runners):

```yaml
name: drain-eunomia-inbox
on:
  schedule:
    - cron: "*/15 * * * *"
  workflow_dispatch: {}
jobs:
  drain:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: python3 extensions/drain.py
        env:
          EUNOMIA_URL: ${{ vars.EUNOMIA_URL }}
          EUNOMIA_EXTENSION: ${{ vars.EUNOMIA_EXTENSION }}
          EUNOMIA_TOKEN: ${{ secrets.EUNOMIA_TOKEN }}
```

## Security model

- **The secret.** 32 random bytes, `exs_` prefix. It is stored encrypted with `ENCRYPTION_KEY`, not hashed, because checking an HMAC signature needs the key itself (a hash of it cannot verify one). It is decrypted in memory for the length of one check, never logged, and returned once, by create and rotate. Rotating replaces it at once: the old secret and signatures made with it stop working. A database copy without `ENCRYPTION_KEY` does not reveal it; one with both does.
- **Constant time.** Signatures are checked with the HMAC library's constant-time verify; a bearer secret is compared after hashing both sides to a fixed length, so timing does not say how much matched.
- **No enumeration.** An unknown extension, a malformed URL and a wrong secret all answer the same 401. The URL key names the owner (`{user key}_{random}`); that is how the unauthenticated route finds the org database, the same reason source webhooks carry the owner id. It is an identifier, not a credential.
- **Guessing.** Secrets are 256 random bits; the gate also limits the inbox per client address.
- **Bounds.** Body at most 256 KiB, read with a 10 second timeout before any database work. At most 10,000 waiting or leased events per extension (`EXTENSION_BUFFER_MAX`); the check is not locked against the insert, so a burst at the edge can overshoot by a few. Finished events are pruned after 7 days, dead ones after 30, by the scheduler leader.
- **Ownership.** Every route is owner-scoped; another user's extension is `extension.not_found` (404), in the same org or another. Create, edit, rotate and delete are written to the audit ledger (`extension.*`), without the secret. Deliveries are not audited one by one.
- **Why code runs outside the server.** Running a user's script inside the Eunomia process would put it next to the database credentials, the `ENCRYPTION_KEY`, every org's data and the network. That boundary is a maintainers' decision (see phase 2), so phase 1 keeps the server a data plane only: the script holds one revocable token with the scopes you chose, and Eunomia never executes it.
- Payloads are untrusted input. The buffer stores them as text and returns them as JSON; nothing interprets them. A script that feeds them to a model should treat them like any other web content.

## Phase 2: hosted runtimes (proposal)

Goal: write the script, push it to git, and Eunomia runs it on a schedule or per batch, with no machine of your own.

- **A separate worker container**, never the API process. It has no database credentials, no `ENCRYPTION_KEY`, no access to SurrealDB's network. It talks to the backend only over HTTP, with the same claim/ack and tool APIs as above.
- **A sandboxed runtime per run.** Deno started with `--allow-net=<eunomia host>` and no file, env, run or FFI permissions is the leading candidate (JS/TS only, permissions are first class); WebAssembly (Python via a wasm build, or JS via QuickJS) is the alternative where Deno's model is not enough. Plus the container's own limits: CPU and memory caps, a wall-clock timeout, a read-only filesystem, no outbound network except the Eunomia host (and an optional per-extension allow-list for scraping).
- **A scoped token per run**, minted by the worker's supervisor for one run: `memory:write` (and `memory:read` if asked for), restricted to the extension's target vault, expiring a few minutes after the timeout, revoked when the run ends. The script never sees a long-lived credential.
- **Config as code.** A manifest in a git repo, for example `eunomia.yaml`: extension name, target vault, entrypoint, `schedule: "*/15 * * * *"`, `trigger: batch` (run when N events or T minutes of events are waiting, with `max` and `lease_secs`), network allow-list, limits. The registry row records the repo, path and commit.
- **Deploy via GitHub Action.** On push to the default branch an Action reads the manifest and calls `PUT /api/extensions/{id}/deployment` with the bundle and commit sha, using a token scoped to extension management only. Eunomia stores the bundle immutably and runs the newest one; rollback is redeploying an older sha.
- **Observability.** Per-run records (start, end, exit, events claimed/acked, bytes of log capped) in the extension's page; failing runs back off like failing source syncs.

Open questions:

1. Who may enable hosted runs: instance admins only, or any user? On a shared org one user's code is every user's neighbour.
2. Sandbox choice: Deno alone, or wasm to allow Python? Where does the isolation boundary sit when the host is a single self-hosted box with no container runtime to spare?
3. Egress: is "Eunomia host only" enough, or do scrapers need an allow-list, and who audits it? Server-side request forgery from the worker to internal addresses must be blocked either way.
4. Quotas: runs per day, CPU seconds, storage for bundles and logs, per user and per org. What happens on a self-host install with no billing?
5. Secrets for the script's own third-party APIs: a per-extension secret store (encrypted, injected at run time), and whether a script may read them back out through its logs.
6. Batching in the platform: should a scheduled job get the buffer's events as its input directly (push) instead of claiming them (pull)?
7. Multi-replica scheduling: the leader-with-lease pattern from `docs/architecture/jobs.md` fits, but a run that outlives its lease needs a fencing story.

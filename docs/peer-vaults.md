# Sharing a memory space with another Eunomia

A memory space can be shared with a **different Eunomia install**: a teammate's, a second computer of
yours, a small Eunomia on a laptop that hangs off a big one at the office. Each Eunomia keeps a full
copy of the space, and the copies keep each other in step by themselves.

(Inviting someone by email is different: that gives a person access to a space on *this* Eunomia.)

## For everyone

**To share a space**

1. Open **Memory spaces**, pick the space, and find **Share with another Eunomia**.
2. Choose what the other Eunomia may do: **Can add and change** or **Can only read**.
3. Press **Create share code**, then **Copy**, and send the code to the person running the other
   Eunomia (chat, email, whatever you use). A code works **once** and expires after **7 days**.

**To join a shared space**

1. On the other Eunomia, open **Memory spaces** and press **Join a shared space**.
2. Paste the code and press **Join**. A new space appears with a full copy, and it stays in step.

**What you will see** next to each connected Eunomia:

| Words | Meaning |
|---|---|
| Up to date | Both copies hold the same things. |
| Syncing… | Changes are on their way. |
| Can't reach the other Eunomia, will keep trying | It is off, asleep or on another network. Nothing is lost: changes wait and go across when it is back. |
| Haven't heard from the other Eunomia lately | (On the Eunomia that shared the space.) The other side has not checked in for a few minutes. |
| A change failed its integrity check | Something arrived damaged or altered and was refused. See below. |
| Sharing stopped. Your copy stays here. | The other side stopped sharing. Your copy is now a normal space. |

**Copies match ✓** means both Eunomias computed the same fingerprint of everything shared. Each
Eunomia also shows a short **ID** (like `3f9a-07c2`): the share code's maker can compare it with what the
joiner sees, to be sure they reached the right one.

**Stop sharing** works from either side. Both sides keep the copy they have; changes simply stop
travelling. The person who shared the space can also cancel a code that was not used yet.

**What is shared:** people, organisations, places, code entities and your own kinds of entity; facts
(memories); relations between them; and saved skills. **Not shared yet:** uploaded documents (facts
already learned from a document *are* shared, as plain facts), and the summarised "observations" each
Eunomia writes from facts: each side makes its own.

**Chains and trees work.** A space you joined can be shared again from your Eunomia. Changes travel
along every connection, so a big Eunomia can share with a second big one, and each of them with
several small ones; a change made anywhere reaches everyone.

## How it connects

The share code holds a list of addresses for the sharing Eunomia, best first:

1. **The local network.** `EUNOMIA_LAN_ADDRS` (the installer writes it with
   `scripts/lan-addrs.sh --env .env`; port 3000, the web port; "Update now" writes it too on Linux
   hosts that lack it), then the private address you opened the app on in your browser (for example
   `192.168.1.20:3000`, so sharing works with no setup when you browse by the machine's network
   address rather than `localhost`), then, when the backend runs outside a container, the private
   address its default route uses. Private means 10/8, 172.16/12, 192.168/16, 169.254/16, and
   100.64/10 (Tailscale). Share codes made by an AI app through `vault_share_code` have no browser
   address, so they rely on the others.
2. **Its public name:** `https://<EUNOMIA_DOMAIN>` when HTTPS is on, then `PUBLIC_URL` unless it is
   `localhost`.
3. **Anything else** in `EUNOMIA_PEER_ADDRS` (comma separated: a Cloudflare tunnel hostname, a port
   forward, `host:port` or full URLs).

The joining Eunomia asks each address in turn (1.5 seconds each) "who are you?"
(`GET /api/peer/v1/{org}/hello`) and uses the first that answers as the Eunomia named in the code.
From then on the **joining Eunomia always calls the sharing one**, roughly every 30 seconds and on
**Sync now**. So only the sharing Eunomia must be reachable: a laptop behind home NAT can join a space
from an office server, but not the other way round. If none of the addresses works, put the sharing
Eunomia on a network both can reach (same Wi-Fi, Tailscale, a tunnel) and add that address.

Set `EUNOMIA_INSTANCE_NAME` to choose how an Eunomia introduces itself to others (otherwise the
joiner uses the person's email and the sharer is named after its address).

## Protocol

Everything below is in `backend/src/peers/`; the routes are in `backend/src/routers/peers.rs`.

**Share code.** `eunomia1:` + base64url of JSON
`{v: 1, org, share, secret, inst, vault, role, addrs}`: the host's org key (the peer routes are per
org), a share id, a 32-byte random secret, the host's instance id (128 random bits per org database,
`peer_instance:self`), the vault's name, `edit` or `read`, and the addresses. The host stores the code's
row (`peer_share`) with the secret **encrypted at rest** with `ENCRYPTION_KEY`, like connector
credentials, and wipes it when the code is used. (The first design stored only a hash of the secret;
but both sides key an HMAC with the secret, so the host needs the secret itself.)

**Join.** The guest sends `POST /api/peer/v1/{org}/join` with
`{share, guest_instance, guest_org, guest_name, guest_addrs, nonce_g, mac}`, where
`mac = HMAC-SHA256(secret, "join", share, guest_instance, guest_org, guest_name, guest_addrs, nonce_g)`
(each part length-prefixed). The host checks the MAC (constant time), that the code is unused and
unexpired, and marks it used in a compare-and-set, so two joins racing for one code cannot both win.
It answers `{link, nonce_h, vault_name, role, host_instance, host_name, mac}` with its own MAC over the
guest's nonce and its answer: the guest only goes on if the host proved it knows the secret too, so an
address that answers in the host's place cannot impersonate it. Both derive the **link key**
`K = HMAC-SHA256(secret, "eunomia-peer-link-v1", link, nonce_g, nonce_h)` and store it encrypted at
rest (`peer_link.key_enc`). The secret never crosses the network.

**Every later message** is `POST /api/peer/v1/{org}/links/{link}` with `{link, ts, nonce, data}`, where
`data` is AES-256-GCM under `K` with associated data `POST|path|link|ts|nonce`. The answer is sealed the
same way with `response|link|request nonce|ts|nonce`, which binds it to its request. Both sides refuse
messages more than 5 minutes off their clock and nonces already seen (an in-memory set per link). This
gives confidentiality and integrity even over plain HTTP on a LAN. An unauthenticated caller learns only
the instance id (`hello`) and 401s.

**Rate limits.** The `/api/peer/...` routes are public and limited per client address like webhooks
(`RATE_LIMIT_WEBHOOK_PER_MIN`, default 120 a minute).

## Sync

The sync cursor is each vault's change log (`changes.rs`, [the change log](agents.md#change-log)).
One exchange (`op: "exchange"`):

- The guest sends its own changes after the cursor the host acknowledged (`changes`, `upto`), the host's
  seq it has applied (`after`), and its last digest.
- The host applies them (only on an `edit` link; a `read` link counts them as `ignored`), and answers
  with its own changes after `after` (`changes`, `next`, `more`), the guest seq it took (`ack`), its
  digest when this page caught the guest up, and a report of what it applied.
- Pages are at most 500 change-log rows and 512 KiB of items; a sync runs up to 20 pages.

For each changed row the sender reads the item **now** and sends its synced fields and their hash, or a
tombstone when it is gone (`delete`, or `merge` with the winner when an entity was merged away).

**Echoes.** What a link writes is logged with actor `peer:<link id>`, on both sides (they share the
id). Rows with that actor are never sent back over the same link, but they are sent over other links:
that is how a change relays down a tree.

**Identity.** `peer_item` maps each local item to the other side's id per link (`remote`), and keeps
the item's **origin** (`<instance id>:<record id>` where it was first written). The receiver finds the
local item by the sender's id, then by the id the sender says the receiver uses (`peer_id`), then by
origin (so an item reaching an instance over two paths lands on one row), then by natural key (an
entity's kind and name, a skill's name). Every id a peer names is checked to be of the right kind and
in the linked vault, so a peer can never reach another vault.

**Writes** go through the normal service paths (`entities::service`, the skill tool), so the change
log, entity dedupe, secret redaction and stale-observation marking all behave as for a local write.

## Integrity

- Each item carries `hash = sha256(canonical JSON {kind, data})`, keys sorted. The receiver recomputes
  it and **refuses** a mismatch; refusals are counted on the link
  (`integrity_failures`, status `integrity_problem`, "A change failed its integrity check"), on
  whichever side saw them, and both sides see the other's counts in the last report. AES-GCM already
  rejects anything altered in transit, so a refusal means a bug or a tampered sender.
- An item whose content already matches the local item is a **no-op** and writes nothing: this also
  ends cycles in a mesh.
- **Conflicts.** When an item changed on both sides since they last agreed (the per-link base hash in
  `peer_item`), the later `updated_at` wins, ties broken by the larger hash. Both sides decide alike,
  so they converge. Clocks matter here: keep them within a minute or two (NTP).
- When a write comes out different from what was sent (the name matched another entity, a secret was
  redacted), the receiver logs it as its own change, so the result travels back and both copies agree.
- **Digest.** sha256 over the sorted item hashes of everything sync carries in the vault. The host
  sends its digest when a guest is caught up; the guest computes its own and shows **Copies match ✓**
  or **Copies differ — syncing**. `GET /api/peers/links/{id}/verify` recomputes both on demand.

## API

For the UI and agents (session or token, like other vault routes):

| Route | Does |
|---|---|
| `POST /api/vaults/{vault}/share-codes` `{role, expires_in_hours?}` | A one-time code (owner only) → `{id, code, role, expires_at, addresses, fingerprint}` |
| `DELETE /api/vaults/{vault}/share-codes/{id}` | Cancel an unused code |
| `GET /api/vaults/{vault}/peers` | Links with status, last sync, digests; unused codes; this instance's addresses |
| `POST /api/peers/join` `{code, name?}` | Join → `{vault, link}` |
| `POST /api/peers/links/{id}/sync` | Sync now (a guest; a host just reports) |
| `GET /api/peers/links/{id}/verify` | Recompute digests |
| `DELETE /api/peers/links/{id}` | Stop sharing (owner only); both keep their copy |

Tools: `vault_share_code`, `vault_join_shared`, `vault_peers`, `vault_peer_sync`.

Between instances: `GET /api/peer/v1/{org}/hello`, `POST /api/peer/v1/{org}/join`,
`POST /api/peer/v1/{org}/links/{link}`. The frontend's `/api/*` proxy forwards them, so port 3000 is
the only port a peer needs.

## Limits and notes

- **Documents** are not shared yet; observations are not shared (each side derives its own).
- The **sharing Eunomia must be reachable** from the joining one (see "How it connects").
- A **read-only** guest's own edits stay on that guest, and its copy then differs ("Copies differ").
- Memory `created_at` is not carried: a copied fact is dated when it arrived.
- **Several backend replicas:** the scheduler leader runs the guest syncs; any replica answers a peer.
  Replay protection is per process, so within the 5 minute window a replica that did not see a nonce
  would accept its replay, and every write it could cause is idempotent.
- **Handing the main copy over (#134)** — making the other Eunomia the one that holds the original —
  is not done yet. Today: the new main copy shares the space back with a fresh code, and the old side
  stops sharing and joins it. Follow-up: flip a link's direction in place.

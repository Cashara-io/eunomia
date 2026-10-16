# Concepts

## Vaults

A vault is a separate scope for entities and memories. Everyone has a
**personal vault**, where writes go by default. Make more for anything you
want kept apart, like a project or service, a client, a team or your homelab. Invite people to
share a vault, and they accept from the Vaults page. Roles are `owner` and `member`.

Agents can name a vault instead of using its id: every tool's `vault_id`
takes a name, e.g. `memory_write` with `vault_id: "Acme"`. The memory skill
tells agents to keep each area's memory in its own vault, and to create
vaults only when you ask.

- **Clone** copies a vault into a new one you own.
- **Merge** takes two vaults and creates a **new** vault from copies of
  both. The originals are never changed. Entities with the same kind and name
  are folded together: aliases are unioned, identical facts are kept once,
  relations are de-duplicated, and two observations are joined. Use it from
  the Vaults page or the `vault_merge` tool.

**Isolation guarantees.** Every read and write checks that you are an
active member of the vault right now: a pending invitation gives no access,
and removing someone or leaving takes effect on the next call. A vault name
only matches vaults you belong to. Relations and entity merges stay inside one
vault. Without a `vault_id`, `recall`, `reflect`, `entities_search` and
`search` read your personal vault plus every org vault you belong to (each hit
names its vault), so facts kept in an org vault never look absent; someone else's
personal vault you've joined is read only when named. To read a chosen set of
vaults together, pass `vault_ids: ["Acme", "Homelab"]` (names or ids; with
`vault_id` too, both are read); every one must be a vault you belong to. Every
other tool, and the export, uses your own personal vault.

**Synced records** are filed in a vault by their connector's routing (see
[connectors](connectors.md#which-vault-records-go-to)). A record left in your
personal vault (the default) is yours alone: even someone you share your
personal vault with never sees it. A record routed to another vault is read by
that vault's active members, in `search`, `get`, `list`, `links`, `recall` and
the vector cloud, and the facts extracted from it are written there. Another
member's records carry their owner's key in their id (`<owner>:github:...`), so
ids stay unambiguous in a shared vault.

## Entities and memories

Entities are people, organisations, locations, repositories, files and
symbols, plus any **custom kinds** your agents create. Each one has **memories**:

| Type | Meaning |
|---|---|
| `world` | an objective fact |
| `experience` | something that happened |
| `observation` | the entity's consolidated belief, one per entity, revised in place |

Memories support full create, read, update and delete (`memory_write`,
`entities_get`/`recall`, `memory_update`, `memory_delete`). Adding, editing
or deleting a fact marks the entity's observation *stale*; a deleted fact
also leaves the observation's lineage (`source_memories`), and an observation
built from nothing but that fact is deleted with it. The next consolidation
rebuilds a stale observation from the entity's surviving facts alone; until
then `recall` leaves it out (`entities_get` still shows it, with
`status: "stale"`).

**Outdated facts.** When new facts arrive (`memory_write`, or a sync), the
chat model checks them against the entity's earlier facts, in the background
(a few seconds after the write). An earlier fact a
newer one shows is no longer true ("no access to the repo", then "was granted
access") is marked `status: "superseded"`: `recall` leaves it out,
`entities_get` still shows it, and editing it makes it current again. Recall
also favours recent memories, and every hit carries its date. Without a chat
model nothing is marked, so check dates on time-sensitive facts.

**Finding and de-duplicating entities.** `entities_search` matches names and
aliases, then the text of each entity's facts, so a handle that only appears
in a fact still finds its entity. `memory_write` with a decorated name
("Jane Doe (Acme/JD)") resolves to an existing "Jane Doe" and adds the parts
as aliases; when it does create an entity, it returns `possible_duplicates`
(same kind, a shared name word) for you to `entity_merge`.

Within a vault an entity's name is unique per kind, case-insensitively, so
two concurrent writes about "Ada" land on one entity, and each entity has at
most one observation. Merging two entities happens in one transaction: the
loser's facts and relations move to the winner, its name and aliases become
the winner's aliases, and if both had an observation they become one stale
observation to be rebuilt. Relations (`works_at`, `calls`, …) link entities,
and an edge with the same label is stored only once.

### Entity kinds

The six built-in kinds are fixed. Any other kind is your agent's own: write
a memory with `subject_kind` set to a new lowercase name (`db`, `table`,
`field`, `defect`, `decision`, `task`, ...) and the kind exists from then on
in that vault. A kind name starts with a letter, then letters, digits or
`_`, at most 32 characters, and a vault holds at most 50 custom kinds.
`entity_types` lists the built-in kinds and the custom kinds in the vaults
you read, with how many entities each has. A custom-kind entity works like
any other: memories, relations, search, recall, merge (with one of the same
kind), vault clone and merge, export. Kinds belong to their vault: another
vault, and anyone who is not a member, never sees them. The Entities page
shows a filter for each custom kind in the vault on screen.

## Synced records

Connectors (Up Bank, GitHub, Gmail, …) sync raw records into a cache. With
a model key, records are embedded for semantic search. Use `search`/`get`
for these, and `recall` to search records and memories together. `search`
filters (sources, types, `since`/`until`) apply before ranking and pages
reach up to 1000 results (`offset` + `limit`, `has_more`); `list` filters
take `field`, `field__gte`/`__gt`/`__lte`/`__lt`/`__ne` or
`payload__<field>`, and reject anything else.

When a record is deleted at its source it becomes a tombstone: `get`,
`search`, `list`, `links` and `recall` treat it as gone, and a link to or
from it disappears. Facts already derived from it are not deleted -- a
memory is its own assertion and outlives its source; its `source` then
points at a record `get` reports as not found. To forget such a fact, delete
it with `memory_delete` (which also invalidates the observation, as above).

## Recall

`recall` runs five arms and fuses them (reciprocal rank fusion), then boosts
results that several arms agree on and recent ones:

1. **semantic**: embeddings over records (needs a model key)
2. **keyword**: full-text over records
3. **memory text**: full-text (BM25) over memories, so a fact is found by what it says
4. **graph**: entities named in the question, then their memories and linked records
5. **temporal**: an explicit `time_range`

The semantic arm runs alongside the four database arms, and every arm has a
4-second deadline: a slow or failing embedding provider drops only the
semantic arm. Inputs are bounded (query at
most 2000 characters, `limit` 1-100, `max_tokens` 1-100000, `time_range` two
ISO 8601 dates with since <= until); anything else is a clear error.

## The graph

Entities → Graph shows the entity graph. Drag nodes to rearrange them,
scroll or use the buttons to zoom, and click a node to open it. The Code
page shows the code graph the same way.

## The vector cloud

Entities → Cloud shows the *shape of what's stored*: every memory and synced
record in a vault as a point. The full vectors are
projected to 3D with PCA, so the axes are the three directions of greatest
spread (PC1–PC3), not entities. Pick several vaults to layer them as
coloured clouds in the **same** space. A vault and its merge, or two teams'
vaults, are directly comparable. Without a model key the space is lexical
(points that share words sit together) rather than semantic.

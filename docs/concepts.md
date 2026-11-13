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

A vault can also be shared with a **different Eunomia install** (a teammate's, another
computer of yours) by a one-time share code: each install keeps a full copy and they stay in step
by themselves. See [sharing with another Eunomia](peer-vaults.md).

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
`entities_get` still shows it, and editing it (a typo fix) keeps it replaced;
deleting the fact that replaced it makes it current again. Recall
also favours recent memories, and every hit carries its date. Without a chat
model nothing is marked, so check dates on time-sensitive facts.

**A fact's two dates.** Every raw fact has two dates, used for different things.
This is the one definition; recall and supersession both follow it.

- *When it happened* (`occurred_at`): the date the writer gave (`memory_write`'s
  `occurred_at`), or for a fact extracted from a synced record, that record's
  date (a message's sent time, an event's start, a page's last edit). It may be
  in the future (a meeting next month). For recall and display a memory's date
  is `occurred_at`, else its last write (`updated_at`, else `created_at`): that
  is what `time_range`, recency and every hit's `occurred_at` use.
- *When we were last told it* (`asserted_at`): only a new claim is a telling.
  - An extracted fact: its record's date, never later than the moment the fact
    was written (a source cannot tell us something after we read it). A record
    that is synced again does not tell its unchanged statements again: the same
    text (ignoring case, spacing and a closing full stop) on the same entity
    from the same record is the same telling, so no second row is written and
    its date does not move. Statements new in the changed record are told at
    its new date. (Facts that disappear from a record are not retracted yet.)
  - A fact a person or agent wrote: the write time, whatever `occurred_at` they
    passed, also when it cites a source record.
  - An edit that changes the text of a current fact: the edit time (the person
    is telling it again), and the fact is checked again. An edit that changes
    only the type tells nothing; an edit of a replaced fact changes its text
    only (it stays replaced).
  - A fact written before this existed falls back to its last write, except a
    fact drawn from a record, which is dated from that record (by a background
    job, or on the spot when a check compares it first).

  Only `asserted_at` decides which fact is current: the fact we were told most
  recently replaces the earlier one, never the other way round, and two facts
  told at the same moment go to the later write. Two facts from the same record
  are never decided between. A check marks a fact only if neither fact changed
  while it was deciding. A replaced fact keeps the id of the fact that replaced
  it (`superseded_by`); deleting that fact makes it current again (only the
  direct one: a fact further down a chain stays replaced), and it is checked
  again.

So an old email imported today cannot replace a newer fact, a correction made
today wins even when it says "true since March", a fact from next month's
calendar event does not outrank today's correction (nor does it after the event
is synced again), and an edit counts as being told again. Limits: a fact
written directly about the past ("worked at Acme in 2019"), or an agent
restating an old fact, is told today, so only the model, which sees both dates,
keeps it from replacing a current fact.

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

### The project ontology

A vault's owners can review its kinds and link types on the Memory spaces
page (Kinds and links). Each one is `observed` (used, never reviewed),
`proposed`, `approved` or `rejected`. Agents read it with `ontology_get`,
propose with `ontology_propose`, and `ontology_suggest` has the server's
model propose (or hands the agent the profile when there is no model). Only
an owner in the app decides. A write under a rejected kind goes to the kind
it was merged into (the result's `kind_mapped` says so), or is refused when
it has none. A relation of a rejected link type is refused too, and the
error names the approved ones; a link type nobody reviewed still works and
shows as `observed`.

Each kind can have typed properties: a name, a datatype (text, number,
integer, money, percent, bool, date, datetime or enum) and a unit, such as
a `price` in AUD on `product`. They are per-vault data, reviewed the same
way, and the review gates writes. `entity_set_property` takes a value only
for an approved property, only when it parses as the datatype, and sends a
value for a rejected property to the one it was merged into. Values are
rows with a date (`as_of`) and a source, so the newest is the current value
(`entities_get` and `entities_neighbourhood` show it) and the older ones
stay as history. Kinds and properties also keep `synonyms`, other names
written in the vault's own data; `ontology_suggest` keeps only the ones it
finds there, kind by kind.

A kind can also have certified metrics: a named number the server computes
from the typed values, such as `avg_price` on `product` (the average of
its `price`). A metric is per-vault data with a typed expression: an
aggregate (sum, count, average, min, max, count of different values, or a
ratio of two), filters on the kind's properties, and an optional date
property and grain for a series. The server checks the expression against
the kind's properties and runs it itself, so no query text ever comes from
an agent or a model. Metrics are reviewed like properties, and the review
gates them: only an approved metric runs by name with `metric_eval`, a
proposed one runs only when the caller asks to try it (and the result says
it is unapproved), and a rejected one runs the metric it was merged into.
Each run reads every entity's latest value as of a date (default now) and
returns the number with its unit and the ids of the entities it came from.
A run can be narrowed to the entities linked to one entity, such as the
gross margin on one supplier's products, and the result says that pick in
words. A ratio's two sides always count the same entities, so a blank cell
on one side leaves that entity out of both, with a warning.
A filter value that matches no listed value comes back as a warning naming
the nearest ones, never as a silent 0. The in-app Chat answers number
questions this way and draws the result as a `metric` view; it never adds
up values itself.

Metrics also answer "what if". `scenario_run` takes hypothetical changes: a
value for one entity's property (`set`), a property of every entity of a
kind that filters or a relation pick multiplied (`scale`) or moved by an
amount (`shift`), or an entity taken out of the picture (`remove`). Each
change is checked as a write would be: only an approved property changes,
the value must fit its datatype, and only numbers scale or shift. The server
puts the changes over the real values in memory, runs the chosen metrics on
both through the same code as `metric_eval`, and returns each number before
and after with the difference, every value it changed, and the entities
within a few relations of the changed ones. Nothing is saved: the what-if
lives only for that one call, and every number in it is the server's. The
model still decides which changes the question means, which metrics matter,
and what the reached entities have to do with it. In the in-app Chat the
answer is drawn as a `scenario` view, which the server runs again itself.

The map can also build itself. `ontology_automap` (or Auto-map in Kinds
and links) profiles the fields of the records a vault already holds, or the
columns of an uploaded CSV: each field's type, how often it is empty,
whether every value differs, its commonest values, the full list when there
are 25 or fewer, its date format, and min and max for numbers. It reads a
sample of the newest 1000 records of each record type and stores nothing.
With a server model it then proposes, for each record type, one kind, a
typed property for each field (each property remembers the field it comes
from) and links for fields that hold another record's key or an entity's
name. Every link is checked against the values first: a share of the
field's values must be the id of a record of the target type or the name of
an entity of the target kind. A link that matches nothing is dropped, and a
kept one says how much matched ("82% of values match"). Everything lands as
`proposed`, for an owner to decide. Without a model, the agent gets the
profiles and the key fields that match, and proposes itself.

Once an owner approved a CSV's kind and its properties, its rows can be
imported (`document_import_rows`, or Import rows on the document): each
row becomes an entity of that kind, found or created by a name column, with
a value for each column that feeds an approved property. Values are checked
as `entity_set_property` checks them and name the document as their source.
A re-run updates the values and never makes an entity twice, and a bad
cell is reported with its line and never stops the rest. Approved custom
kinds also feed entity extraction: besides people, organisations and
locations, extraction from a synced record or document may write entities
of the vault's approved kinds (at most eight per prompt, never a proposed
or rejected one).

Before a number or a what-if, agents and the Chat check the question's
coverage (`ontology_coverage`): its key phrases are matched to the approved
kinds, properties, metrics and link types, by name and synonym and else by
meaning, and a phrase nothing approved covers ("gross margin", when the
vault has no such metric) is reported instead of guessed at, with a
suggestion of what to propose. Matching by meaning compares embeddings, kept
on each approved term, under a cosine distance of 0.6. On the Harbour Coffee
demo, phrases for terms the vault has measured 0.41 to 0.60 from them, and
phrases for terms it lacks 0.64 to 0.83, so 0.6 sits in the gap (NVIDIA's
0.75 would count a missing gross margin or rent as covered). Without a
model, phrases are split from the question and matched by name and synonym.
Readiness (`ontology_readiness`, and the summary at the top of Kinds and
links) counts what is approved, proposed and rejected, and lists what still
blocks answering numbers in plain words.

An owner can switch a vault to **Only approved kinds and links**. Then a
write under a custom kind that is not approved, or a relation under a link
type that is not approved, is refused with the approved ones to use instead,
and the unapproved one is filed as a proposal for the owner. Built-in kinds
always pass, extraction leaves refused items out, and the switch is off
until an owner turns it on.

Typed properties, certified metrics, the literal check on metric filters,
record auto-mapping, the coverage check and the readiness summary follow
ideas from NVIDIA Auto Ontology (Apache 2.0).
See `THIRD_PARTY_NOTICES.md`.

The ontology is also how agents and the in-app Chat reason about knock-on
effects. Nothing about what affects what is hardcoded: asked "Truck 7 is
late, what does that affect?", the model reads the link types and their
descriptions (`ontology_get`), the entities around the truck with their
facts (`entities_neighbourhood`), decides which way the delay travels, and
shows its answer with `show_view`, which checks every id and path before
the app draws it. A clear description on a link type ("a late truck delays
what it carries") is what lets the model get the direction right.

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

`recall` runs seven arms and fuses them (reciprocal rank fusion), then boosts
results that several arms agree on and recent ones:

1. **semantic**: embeddings over records (needs a model key)
2. **memory semantic**: embeddings over remembered facts, so a fact is found by
   what it means ("Grace's occupation" finds "builds compilers for a living");
   needs a model key
3. **keyword**: full-text over records
4. **memory text**: full-text (BM25) over memories, so a fact is found by what it says
5. **graph**: entities named in the question, their memories and linked
   records, then one hop along their relations: the relations themselves (a hit
   of kind `relation`, e.g. "Ada works at Acme") and the neighbours' newest
   facts, only inside the vault being searched
6. **temporal**: an explicit `time_range`
7. **relation**: relations joining two entities the question names ("how is
   Ada connected to Acme"), counted as their own signal

Facts are embedded in the background right after they are written (and by the
embed job for anything older); editing a fact drops its vector until it is
embedded again, and a query only meets vectors from the same model. A
semantic hit must reach a minimum similarity, so an unrelated nearest
neighbour never rides in on rank alone.

A fact can carry when it happened: `memory_write` takes `occurred_at` (a fact
written today about last March; see "A fact's two dates" above). The temporal
arm and recency use it, and with
a `time_range` "recent" means "inside that range". Superseded facts are left
out unless `include_history` is true, which returns them marked
`status: "superseded"`.

The query is embedded once, alongside the database arms, and every arm has a
4-second deadline: a slow or failing embedding provider drops only the
semantic arms. Inputs are bounded (query at
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

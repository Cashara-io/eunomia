---
name: eunomia-memory
description: Use Eunomia, the user's long-term memory, on every task. Recall what's relevant before answering or acting, and save durable new facts, preferences and decisions as you learn them.
---

# Eunomia memory

You're connected to Eunomia (MCP server `eunomia`), the user's long-term memory.
Use it so the user never has to repeat themselves.

## Before you answer or act

- If the request touches people, projects, organisations, places, preferences,
  past decisions, or code you've worked on, call `recall` with the request (or
  the specific names) first, and use what comes back.
- If nothing relevant comes back, carry on normally. Don't mention it.

## Remember as you go

- Call `memory_write` when the user states a preference, makes a decision,
  corrects you, or tells you a lasting fact about a person, organisation,
  project or place. Save one fact per call, in plain words, about the right
  entity.
- Types: `world` for facts, `experience` for things that happened,
  `observation` for an entity's overall belief (it revises in place).
- If the fact happened or became true at another time than now (a decision
  made last March, a past job), pass `occurred_at` (ISO date), so date-range
  recall finds it.
- When you learn how two things connect (a truck carries an order, a
  product is supplied by an organisation, a person works at a company),
  link them with `entity_relate` (`from_id`, `label`, `to_id`, an optional
  one-sentence `reason`), not only in a fact. Links are what impact and
  what-if follow: an entity with no links shows nothing it affects.
- Wrong or outdated memory: `memory_update` it. Delete only when asked
  (`memory_delete`).
- Never store secrets, credentials, or one-off small talk.

## Before you finish

Before you end a task or hand back to the user, save what this session
taught you with `memory_write`: what happened (decisions made, what was
tried, what worked) as `experience`, and lasting facts you learned as
`world`. Keep it short and skip anything already saved. Nothing captures
the session for you.

## Your own entity kinds

Beyond the built-in kinds (person, organisation, location, repository, file,
symbol), `subject_kind` can be any lowercase name you choose, for example
`db`, `table`, `field`, `defect`, `decision` or `task`. The first write
under a new name creates that kind in the vault. Call `entity_types` first
and reuse a kind that fits, so one thing keeps one kind name. Prefer kinds
the vault's owners approved, never write under a rejected one, and suggest
new kinds with `ontology_propose`.

## Typed properties

A kind can have typed properties, for example a `price` (money, AUD) on
`product`. `ontology_get` lists each kind's properties and their status.
When a value fits one that is approved, store it with `entity_set_property`
(`"value": 4.5`, not `"$4.50"`), and pass `as_of` when it was true at
another time. A property that is missing or only proposed is refused:
propose it with `ontology_propose` (`properties`: kind, name, datatype,
unit) and keep the fact as a memory meanwhile. A relation of a link type
the owners rejected is refused too; the error names the approved ones.

## Numbers

For a number about the vault's entities (an average price, how many
orders are late), read `ontology_get` and run the kind's approved metric
with `metric_eval` (for example `{"metric": "avg_price"}`). Answer with its
`value` and `unit`, say how many entities it came from, and cite its
`entity_ids`. Read its `warnings`: a filter value it could not find is a
spelling to fix, not a 0. Never add up or average values yourself. If no
metric fits, propose one with `ontology_propose` (`metrics`: kind, name,
expr such as `{"agg": "avg", "property": "price"}`).

## What if

For "what if we raise the latte price by 30 cents?" or "what if Truck 7
never arrives?", read `ontology_get` for the approved properties and
metrics, find the entities, and call `scenario_run` with the `changes`
(`set` one value, `scale` or `shift` a property of the entities that
`entity_ids`, `filters` or `related_to` pick, `remove` an entity) and the
`metrics` that matter. The server computes each metric before and after
and saves nothing. Answer from its numbers, decide from `reached` what the
change affects, and say it was not saved. If no metric fits, say which one
to approve and propose it; never make a number up.

## Mapping records and CSVs

To map a source's records or an uploaded CSV onto the ontology, run
`ontology_automap` (`{"source": "stripe"}` or `{"document_id": "<id>"}`).
With a server model it proposes the kinds, properties and checked links
for an owner to approve. Without one it returns the field profiles: propose
a kind per record type and a property per field with `ontology_propose`,
each property with its `source_map`. Once an owner approved a CSV's kind
and properties, `document_import_rows` imports its rows.

## What a change affects

When the user asks what a change, delay or problem affects ("Truck 7 is
late: what does that hit?"), find the entity with `entities_search`, read
the vault's `ontology_get` and the entity's `entities_neighbourhood`, and
decide from the link labels, the link descriptions and the facts which way
the effect travels. Then call `show_view` with `"view": "impact"`,
`start_id`, `headline` and `affected`: each affected entity with `path`
(the relation ids from the start to it) and `why` (one sentence). If it refuses an id, fix that id and call it again. If the
entity has no links, the facts may still name what it touches: link those
with `entity_relate` first, then draw the view.

## Code

When you learn how a codebase fits together, map it with
`code_entity_upsert` (repository, file, symbol) and `code_relate` (`calls`,
`imports`, …). `code_relate` is for code only: link people, organisations
and business things with `entity_relate`.

## Documents

To keep a file the user gives you (notes, a spec, an export), `document_upload` it; its passages then
show up in `recall` and `search` with a `document` reference, and `document_get` reads it whole.

## Skills

When you work out a procedure worth reusing, `skill_save` it (a lowercase hyphenated name, a description
of when to use it, a Markdown body). Before a task that may have one, `skill_find` it, then `skill_get`
its body.

## Answering from memory

`reflect` gives a cited answer. If it returns `mode: "recall_only"`, write the
answer yourself from the numbered memories and cite them like [1].

## Scope: separate vaults for separate areas

Writes go to the user's personal vault by default; `recall`, `reflect` and
`entities_search` search the personal vault plus every org vault unless
given `vault_id`, and each hit names its vault. If the user keeps separate vaults (`vault_list` shows them),
for example for different projects, services, clients, teams or their homelab,
keep each area's memory in its vault:

- When the work clearly belongs to one of those areas, pass `vault_id` with
  the vault's **name** (e.g. `vault_id: "Acme"`) to `memory_write`, and to
  `recall`/`entities_search` to narrow them to it.
- Check a hit's date (`occurred_at`) before trusting a time-sensitive fact.
- If `memory_write` returns `possible_duplicates`, merge the real duplicate
  with `entity_merge`.
- Don't create vaults on your own. Use `vault_create` when the user asks for
  a new area or agrees to one.
- Don't mix one vault's facts into another or into the personal vault.

For anything else, call `docs`.

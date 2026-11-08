# AI agents & MCP

Eunomia is an MCP server (Streamable HTTP) at `http://localhost:8001/mcp`
(also proxied at `http://localhost:3000/mcp`). The backend port listens on this machine only; agents on other machines use the frontend URL, e.g. `https://eunomia.example.com/mcp` (see [deployment](deployment.md#1-https)). It exposes the same tools the
in-app chat uses. Every call is scoped to the token's user and logged in the
audit log if it changes anything.

## Connecting agents

The installer does this for you. To do it again, or for an agent you install
later:

```bash
cd eunomia
./scripts/connect-agents.sh --email you@example.com --password '…'          # every agent
./scripts/connect-agents.sh --token <token> --agents claude,codex           # an existing token
```

| Agent | Config written |
|---|---|
| Claude Code | `~/.claude.json` → `mcpServers.eunomia` |
| Codex | `~/.codex/config.toml` → `[mcp_servers.eunomia]` |
| Hermes | `~/.hermes/config.yaml` → `mcp_servers.eunomia` |
| Gemini CLI | `~/.gemini/settings.json` |
| Cursor | `~/.cursor/mcp.json` |
| Windsurf | `~/.codeium/windsurf/mcp_config.json` |
| OpenCode | `~/.config/opencode/opencode.json` |
| VS Code | `…/Code/User/mcp.json` → `servers.eunomia` |
| Claude Desktop | `…/Claude/claude_desktop_config.json` (through `npx mcp-remote`, needs Node) |

- Each agent gets its own token (Settings → API tokens, named after it), so
  you can revoke one without touching the others. Re-running replaces the
  entry and revokes the old token.
- Configs for agents you haven't installed yet are written too, so they're
  connected the first time they run (`--detected-only` turns that off).
- Other settings in those files are preserved. A file the script can't
  parse, or one with its own hand-written `eunomia` entry, is left untouched
  and reported.
- ChatGPT and Claude.ai (web/desktop) need a public `https://` URL. Add them
  by hand (below).

## Connecting one agent by hand

The onboarding wizard's **Connect your AI app** step, and the app picker on
**Home**, do this for you: pick the app and copy the command or config with
your address already filled in (an API token, if you need one, is under
Advanced). The steps it shows are these. Use the address you open
Eunomia at, plus `/mcp` (for example `http://localhost:3000/mcp`, or
`https://eunomia.example.com/mcp`). A `localhost` or LAN address only works
for agents on that computer or network. Agents learn when to recall and what
to remember from the server itself, so there is nothing else to install.

### Claude Code

```bash
claude mcp add --transport http --scope user eunomia https://eunomia.example.com/mcp \
  --header "Authorization: Bearer <token>"
```

`--scope user` makes it available in every project (the default, `local`, is
the current project only). Check it with `claude mcp list` (eunomia shows
`✔ Connected`), restart any open session, and `/mcp` inside Claude Code lists
it. To sign in with OAuth instead of a token, leave out `--header`, then run
`/mcp` in a session (or `claude mcp login eunomia`) and approve the consent
page; see [OAuth](#sign-in-with-oauth-no-pasted-token). Use one or the other:
with a header set, Claude Code doesn't fall back to OAuth.

### Codex

Add to `~/.codex/config.toml` (the Codex CLI, IDE extension and app share it):

```toml
[mcp_servers.eunomia]
url = "https://eunomia.example.com/mcp"
http_headers = { Authorization = "Bearer <token>" }
```

To keep the token out of the file, use
`bearer_token_env_var = "EUNOMIA_TOKEN"` instead of `http_headers` and export
`EUNOMIA_TOKEN` where Codex runs, or add it from the shell with
`codex mcp add eunomia --url https://eunomia.example.com/mcp --bearer-token-env-var EUNOMIA_TOKEN`.
A raw `bearer_token` key is rejected for HTTP servers. For OAuth, run
`codex mcp add eunomia --url …` with no token and `codex mcp login eunomia`.
Check it with `codex mcp list`, then `/mcp` in a new Codex session.

### ChatGPT

ChatGPT connects from OpenAI's servers, so Eunomia needs a public `https://`
address (Settings → HTTPS, or a tunnel), and `PUBLIC_URL` must be that address
(the HTTPS setup sets it). A `localhost` or LAN install can't be reached.

1. In ChatGPT on the web, open [chatgpt.com/plugins](https://chatgpt.com/plugins),
   choose **+**, then **Add custom MCP server**.
2. Name it Eunomia, enter the server URL including `/mcp`, and set
   authentication to **OAuth** (ChatGPT has no option for a pasted bearer
   token; it registers itself with Eunomia's OAuth server).
3. Select **I understand and want to continue**, then **Create as a plugin**,
   and approve access on Eunomia's consent page.
4. Install it from your personal plugins, then in a chat type `@` and pick
   Eunomia.

Custom MCP servers depend on your ChatGPT plan and workspace policy: OpenAI
lists Business, Enterprise and Edu with full (read and write) access, and Pro
with read-only tools, which can `recall` but not save. Sources:
[OpenAI: connect ChatGPT](https://developers.openai.com/plugins/deploy/connect-chatgpt),
[developer mode and MCP apps](https://help.openai.com/en/articles/12584461-developer-mode-and-mcp-apps-in-chatgpt).

## The memory skill

Admins can add **AI instructions** (a page under Advanced in the sidebar, shown to admins only):
the instructions that tell agents when to recall and what to remember. An admin can view them,
edit them, or reset them to the built-in default. It reaches agents three ways:

- **Every MCP client** gets it as the server's connection instructions, so an
  edit applies the next time the agent connects.
- **Claude Code, Codex, Hermes** get a `skills/eunomia-memory/SKILL.md`
  (re-run `connect-agents.sh` to refresh the file copies).
- **Claude Code** also gets two hooks in `~/.claude/settings.json`:
  `SessionStart` injects your current skill, and `UserPromptSubmit` calls
  `recall` with each prompt and injects the relevant memories before Claude
  sees it. The hooks are silent and never block a prompt if Eunomia is down.

By hand, for any MCP client: URL `http://localhost:8001/mcp`, header
`Authorization: Bearer <token>`. Create a token in Settings → API tokens.
This is the fallback that works with every client.

### Sign in with OAuth (no pasted token)

MCP clients that support OAuth (Claude Code, Cursor, and others following the
MCP authorization spec, revision 2026-07-28) need only the URL:

```bash
claude mcp add --transport http eunomia http://localhost:8001/mcp
```

The client discovers Eunomia's authorization server, opens your browser, and
shows a consent page: who is asking, where you return to afterwards, and what
it may do (read memory, write memory, manage vaults, manage sources). Allow
it and the client holds a 15-minute access token plus a refresh token it
rotates on its own. Nothing is pasted.

- **Manage apps**: Settings → Connected apps lists every app you approved.
  Disconnecting one revokes its tokens immediately.
- **Scopes**: `memory:read`, `memory:write`, `vaults:admin`, `connectors`. A
  token that only has `memory:read` is refused (HTTP 403, `insufficient_scope`)
  when it calls a tool that writes. The default request is read plus write.
- **Bound to this server**: tokens are issued for the MCP URL only, work on
  `/mcp` only, and are rejected anywhere else.
- **Public URL**: set `PUBLIC_URL` (default `http://localhost:8001`) to the
  address clients reach Eunomia at, with no trailing slash. It becomes the
  token audience and the base of the discovery documents, so set it to
  `https://eunomia.example.com` when you put Eunomia behind a domain. Behind
  the frontend on port 3000, use `http://localhost:3000`; `/.well-known/*` and
  `/oauth/*` are proxied there too. Changing it invalidates existing tokens.
- **Client registration**: clients identified by an HTTPS metadata URL (Client
  ID Metadata Documents) work without registering; older clients register
  through `/oauth/register`. Eunomia fetches metadata documents only from
  public HTTPS hosts. A self-hosted install with no internet access can still
  use clients that register dynamically.
- **Discovery**: `/.well-known/oauth-protected-resource` and
  `/.well-known/oauth-authorization-server`. An unauthenticated `/mcp` call
  answers `401` with a `WWW-Authenticate: Bearer resource_metadata="..."`
  header pointing at the first.
- **Not supported**: confidential clients (client secrets), the implicit and
  password grants, `plain` PKCE.

## Do I need an OpenAI key?

No. The connected agent is the model.

| Without a key | With a key (recommended to save tokens) |
|---|---|
| `recall`: keyword + full-text over memories + graph + recency | adds semantic (embedding) search |
| `reflect` returns the numbered memories; your agent writes the answer | Eunomia writes the cited answer |
| `consolidate_observations`: your agent writes the belief with `memory_write` type `observation` | Eunomia synthesizes it |
| Vector cloud uses a lexical (word) space | Real embedding space |
| In-app Chat is off | In-app Chat works |

## Tools

| Tool | Does |
|---|---|
| `docs` | Read these docs. No args lists them, `topic` reads one |
| `recall` | Best memories and records for a question, within a token budget |
| `reflect` | Cited answer from recalled memories (or the memories, without a key) |
| `search` / `get` / `list` / `links` | Raw synced records from connectors |
| `memory_write` | Remember a fact (`world`, `experience`) or set an entity's `observation` |
| `memory_write_batch` | Remember up to 100 facts in one call, one result per fact; for imports |
| `memory_update` / `memory_delete` | Edit or delete one memory |
| `entities_search` / `entities_get` / `entities_graph` | Find and read people, orgs, places, code and custom kinds |
| `entities_neighbourhood` | The entities within 1 to 3 relations of one entity, both directions, with their latest facts and the review of each link type between them; for working out what a change affects. At most 150 entities and 600 relations (`truncated` says when more were in reach) |
| `show_view` | Show your answer as a view: `impact` (what a change affects, each entity with its path of relations and why), `subgraph` (how entities connect), `metric` (an approved metric's number, which the server runs again) or `scenario` (a what-if, which the server runs again). Every id is checked; returns the view with names filled in plus a plain-text version |
| `entity_types` | The built-in entity kinds and the custom kinds agents created (any new lowercase `subject_kind` creates one), each with its review status |
| `ontology_get` | One vault's kinds and link types, with counts, samples, descriptions, synonyms and review status, and each kind's typed properties and metrics |
| `ontology_suggest` / `ontology_propose` | Propose kinds, merges, link types, typed properties, metrics and synonyms for a vault owner to approve in the app; without a server model `ontology_suggest` returns the profile for you to review |
| `ontology_coverage` | Whether a question can be answered from the vault's approved ontology: its key phrases, each matched to an approved kind, property, metric or link type (or not), a `coverage` share and a suggestion. Call it before a number or a what-if ([coverage](#coverage-and-readiness)) |
| `ontology_readiness` | The vault's kinds, properties, metrics and link types by status, whether it takes only approved kinds and links, and what still blocks answering numbers, in plain words ([readiness](#coverage-and-readiness)) |
| `metric_eval` | Run an approved metric by name (`avg_price`): the number, its unit, as of when, and the entity ids it came from; optional extra filters, `as_of` and a series by period ([certified metrics](#certified-metrics)) |
| `scenario_run` | A what-if: hypothetical changes (`set`, `scale`, `shift`, `remove`) put over the real values in memory, the chosen approved metrics before and after, the values changed and the entities the changes reach. Nothing is saved ([what if](#what-if)) |
| `ontology_automap` | Profile the fields of synced records (all, one `source` or one `record_type`) or an uploaded CSV (`document_id`), and with a server model propose a kind per record type, a typed property per field and links whose values were checked; without a model, the profiles and matching key fields for you to propose ([auto-mapping](#auto-mapping)) |
| `document_import_rows` | Import an uploaded CSV's rows as entities of an approved kind with values of its approved properties; idempotent, bad cells reported by line ([auto-mapping](#auto-mapping)) |
| `entity_set_property` | Set one approved typed property of an entity (a `price` on a `product`), checked against its datatype and kept with its date and source ([typed properties](#typed-properties)) |
| `entity_update` / `entity_merge` / `entity_delete` | Edit, de-duplicate, delete entities |
| `code_entity_upsert` / `code_relate` | Map repositories, files, symbols and their relations |
| `consolidate_observations` | Fold new facts into each entity's belief |
| `vault_list` / `vault_create` / `vault_clone` / `vault_merge` | Manage vaults (separate scopes for projects, clients, teams, …); merge two into a new one. Any tool's `vault_id` also accepts a vault's name |
| `vault_invite` / `vault_members` / `vault_remove_member` / `vault_leave` / `vault_rename` / `vault_delete` | Sharing |
| `vault_get` | One vault: role, kind, created_at, member count |
| `vault_invitations` / `vault_accept_invitation` / `vault_decline_invitation` | See and answer invitations addressed to you |
| `vault_set_role` / `vault_transfer_ownership` | Promote or demote a member; hand a vault to another member (owner only, last-owner guard) |
| `document_upload` / `document_list` / `document_get` | Upload a document (text, Markdown, JSON, CSV, text PDF) and read its extracted text; its passages are found by `search`, `recall` and `get`, each hit naming the document ([documents](documents.md)) |
| `document_download` / `document_export` / `document_delete` | The HTTP route for the original bytes, the passages-and-vectors manifest, deletion |
| `skill_save` / `skill_find` / `skill_list` / `skill_get` / `skill_delete` | Keep a skill an agent learned, find it later by meaning, list a vault's skills, read its body, delete it ([agent skills](#agent-skills)) |
| `vault_share_code` / `vault_join_shared` / `vault_peers` / `vault_peer_sync` | Share a vault with another Eunomia install by a one-time code, join one, see each link's sync status, sync now ([peer vaults](peer-vaults.md)) |
| `vault_check` / `vault_repair` | Check a vault for broken or leftover data (memories whose entity is gone, relations with a missing end, misfiled memories, a change-log counter behind); repair removes the dangling rows and fixes the counter (owner only). Also `GET`/`POST /api/vaults/{vault}/integrity` |
| `changes_since` | A vault's numbered change log after a cursor: what was created, updated, merged or deleted, by whom, with no content ([change log](#change-log)) |
| `secret_request` / `secret_request_status` | Ask the user for a password or API key through a masked pop-up in the Eunomia web app instead of the chat; you get a handle, never the value ([secret requests](#secret-requests)) |
| `secret_transform` | Make a new held secret from others (base64, a Bearer or Basic header, an HMAC, a JWT) and get its handle |
| `http_request` | Send one HTTPS request with handles in its headers, query or body, to a host the user approved |

Destructive tools are flagged `destructiveHint` so clients can ask first.

Secrets are never stored: `memory_write`, `memory_write_batch`, `memory_update`, `skill_save` and the REST
memory routes replace API keys, tokens, private keys, JWTs and passwords in URLs with a marker such as
`[redacted: github token]`, still save the rest, and return `redacted: [{"kind", "count"}]` so the agent
knows what was dropped.

Secrets saved before redaction existed are redacted once after the upgrade, by a background job in each
org (`redact_backfill`). It rewrites memories (facts, observations and superseded facts), skill
descriptions and bodies, and audit-log argument summaries, and drops the vectors made from the old text,
so the embed job makes new ones. It runs once per org, resumes where it stopped after a crash, and runs
again when the detectors gain new shapes. When it finishes, the server logs `secret backfill finished`
with how many rows of each kind changed, and the instance audit log gets a `redact.backfill` event with
the same counts. It never logs the text. A rewrite leaves a memory's `version` and `updated_at` as they
were, but adds an `updated` row (actor `system`) to the vault's change log for each rewritten memory or
skill, so a client following the vault (for example `create-eunomia watch`) fetches the redacted text.
Change-log rows hold no content, so there is nothing in them to redact. Uploaded
documents, synced connector records, chat messages, entity names, aliases and summaries, relation labels,
sync error messages and extension webhook deliveries are not redacted on write, so the backfill leaves
them alone too.

### Typed properties

A vault's ontology can give a kind typed properties, and only an approved one takes values. Say a cafe
vault keeps its menu as `product` entities and the user says "a latte is $4.50 now":

1. `ontology_get` shows `product` with a property `price` (`money`, unit `AUD`, status `approved`).
   If there is none, propose it and keep the fact as a memory meanwhile:
   `ontology_propose` `{"properties": [{"kind": "product", "name": "price", "datatype": "money", "unit": "AUD"}]}`.
   A vault owner approves it on the Memory spaces page (Kinds and links).
2. `entity_set_property` `{"entity_id": "<latte id>", "property": "price", "value": 4.5}` stores it. The
   value is a number, not `"$4.50"`; pass `as_of` (an ISO date) when the price held from another day, and
   `source_record_id` when it came from a synced record.
3. `entities_get` on the latte now returns `properties: [{"property": "price", "datatype": "money",
   "value": 4.5, "unit": "AUD", "as_of": "...", "source": null}]`. An older price stays as history; the
   newest `as_of` is the current value.

What is refused, with an error that says why: a property that does not exist or is only proposed (propose
it and wait), a value that does not fit the datatype (a money value with a currency sign, an enum value not
in its list, a date that is not `YYYY-MM-DD`), and a property rejected with no merge target. A property
rejected into another (say `cost` into `price`) writes to that one, and the result's `property_mapped`
says so. Relations are grounded the same way: a `code_relate` whose link type the owners rejected is
refused, and the error names the approved link types between the two kinds.

### Auto-mapping

`ontology_automap` builds the ontology from what Eunomia already holds. Say a cafe uploads its supplier
price list as a CSV (product, supplier, price AUD, unit, lead_time_days, status):

1. `document_upload` the file, then `ontology_automap` `{"document_id": "<its id>"}`. Eunomia profiles each
   column (type, empty share, uniqueness, top values, enum values, date format, min and max). With a
   server model it returns `mode: "model"`: one kind (`supply_item`), a property per column with the
   column it comes from in `source_map`, and links such as `supply_item supplied_by organisation` from
   the supplier column. Each link was checked first: `links[]` says how many of the column's values match
   an entity of the target kind (or a record of the target type, for an id field), and a link that matches
   nothing is dropped. Everything is `proposed`.
2. Without a server model it returns `mode: "profile_only"` with `profiles` and `key_matches` (id fields
   whose values are another record type's ids, with the share). Propose yourself with `ontology_propose`,
   giving each property a `source_map` such as `[{"source": "document", "record_type": "<document id>",
   "path": "price AUD"}]`, so the import knows which column feeds it.
3. A vault owner approves the kind and the properties in the app (Kinds and links).
4. `document_import_rows` `{"document_id": "<id>", "kind": "supply_item", "name_column": "product"}`
   writes each row as an entity found or created by its name, with a value for each column that feeds an
   approved property (pass `column_map` `{"price AUD": "price"}` to choose yourself). It is refused
   before the kind and at least one property are approved. It answers with `created`, `updated`,
   `values`, the `columns` used or skipped (and why), and `bad` cells with their line and column (a
   `$32.50` in a money column, a status not in the enum); the rest is still imported. Run it again after
   a new revision of the file: values are updated in place and no entity is made twice.
5. A metric over the imported values (`avg_price` on `supply_item`) then answers "what does a supply item
   cost on average?" through `metric_eval`.

For synced records, `ontology_automap` `{"source": "stripe"}` profiles each record type of that source in
the vault (payload key paths, nested objects to four levels, at most 60 fields and 12 record types). The
in-app Chat does not run `document_import_rows`: it writes many entities at once from a file, so it stays a
button on the document and a tool for agents.

### Certified metrics

A metric is a number the server computes from a kind's typed properties. Only an approved one runs by
name, and every result carries the entities it came from, so you never add up values yourself. Say the
cafe vault has the approved `price` (money, AUD) on `product` from above, and the user asks "what do our
products cost on average?":

1. `ontology_get` shows `product` with a metric `avg_price` (`expr` `{"agg": "avg", "property": "price"}`,
   `expr_words` "Average price", status `approved`). If there is none, propose it:
   `ontology_propose` `{"metrics": [{"kind": "product", "name": "avg_price", "description": "What a product
   costs on average", "expr": {"agg": "avg", "property": "price"}}]}`. A vault owner approves it on the
   Memory spaces page (Kinds and links), after approving `price`.
2. `metric_eval` `{"metric": "avg_price"}` runs it:

```json
{"metric": "avg_price", "kind": "product", "value": 4.666667, "unit": "AUD", "as_of": "2026-11-08T09:30:00Z",
 "n_entities": 3, "entity_ids": ["entity:latte", "entity:mocha", "entity:tea"], "truncated": false,
 "status": "approved", "warnings": [], "expr_words": "Average price"}
```

3. Answer with the value and unit, say it came from 3 products and cite their ids. To draw it, call
   `show_view` `{"view": "metric", "metric": "avg_price"}`: the server runs the metric again and shows its
   own number, never one you send.

An expression is `agg` (`sum`, `avg`, `min` and `max` over a number, integer, money or percent property;
`count` of entities, or of those with a `property`; `count_distinct` of a property's values; `ratio` of a
`numerator` over a `denominator`, each with its own agg, property and filters), `filters`
(`[{"property": "supplier", "op": "eq", "value": "Riverside Roasters"}]`, ops `eq`, `ne`, `gt`, `gte`, `lt`,
`lte` and `in`, at most 10; `ne` also picks an entity with no value for the property, so "status is not
delivered" counts one with no status), and for a series `time_property` (a date property) and `grain` (`day`, `week`,
`month`, `quarter` or `year`). Every property must be on the kind and every value must fit its datatype;
anything else, including query text, is refused when proposed.

Instead of a `property`, `sum`, `avg`, `min` and `max` (and a ratio's sides) take a `row`: a value worked out
for each entity, `{"op": "mul", "left": "stock", "right": "cost"}`, where `op` is `mul`, `sub`, `add` or `div`
and each side is a numeric property, a number or another row (at most 3 deep). Total stock value is
`{"agg": "sum", "row": {"op": "mul", "left": "stock", "right": "cost"}}` ("Sum of stock × cost"); gross margin is
a ratio of `{"agg": "sum", "row": {"op": "sub", "left": "price", "right": "cost"}}` over
`{"agg": "sum", "property": "price"}`. Units are checked when proposed: money times a number is money, money
minus money of one currency is money, money over money is a share, and money times money is refused. An entity
whose row divides by 0 is left out and counted in `warnings`. A what-if's changed values flow through rows too.

`metric_eval` also takes extra `filters` (ANDed with the metric's own), `as_of` (each entity's latest value
by then; a bare date includes its day), `group_by_grain` (a `series` of `{period, value, n}` instead of one
value) and `kind` (when two kinds have a metric of that name). A proposed metric is refused unless you pass
`allow_unapproved: true`; it then runs with `status: "proposed"` and a warning, so never present that
number as checked. A rejected metric with a merge target runs the target (`metric_mapped` says so). Read
`warnings`: an enum filter value listed nowhere ("Lage") returns no value and names the nearest listed
ones ("Large"), and a text value no entity holds names the nearest stored ones, so fix the spelling
instead of reporting 0.

### What if

`scenario_run` answers "what happens if I do this?" with numbers the server computes, and saves nothing.
You decide what changes and which metrics matter; the server checks each change as `entity_set_property`
checks a value, applies the changes in memory, and runs each metric on the real values and on the changed
ones with the same code as `metric_eval`.

"What if we raise the latte price by 30 cents?" in the cafe vault above:

1. `ontology_get` shows the approved `price` on `product` and the approved metric `avg_price`.
2. `entities_search` `{"query": "Latte"}` finds the latte.
3. `scenario_run`:

```json
{"changes": [{"shift": {"kind": "product", "property": "price", "delta": 0.3, "entity_ids": ["entity:latte"]}}],
 "metrics": ["avg_price"]}
```

returns

```json
{"changes": ["Latte price up 0.30 AUD"],
 "baseline": [{"metric": "avg_price", "kind": "product", "value": 4.666667, "unit": "AUD", "n_entities": 3, "status": "approved", "expr_words": "Average price"}],
 "scenario": [{"metric": "avg_price", "kind": "product", "value": 4.766667, "unit": "AUD", "n_entities": 3, "status": "approved", "expr_words": "Average price", "delta": 0.1, "delta_pct": 2.14}],
 "changed": [{"entity_id": "entity:latte", "name": "Latte", "kind": "product", "property": "price", "before": 5.0, "after": 5.3, "unit": "AUD"}],
 "n_changed": 1, "removed": [], "reached": [], "reached_truncated": false, "warnings": [], "as_of": "2026-11-08T09:30:00Z"}
```

4. Draw it with `show_view` `{"view": "scenario", "headline": "What if the latte costs 30 cents more?",
   "scenario": <the same arguments>}`, and say nothing was saved.

Each of `metrics` is an approved metric's name, or `{"metric": "avg_price", "kind": "product"}` when two kinds
have a metric of that name.

The four changes, applied in order (at most 20):

- `{"set": {"entity_id", "property", "value", "unit"?}}`: a hypothetical value for one entity.
- `{"scale": {"kind", "property", "factor", "entity_ids"?, "filters"?, "related_to"?}}`: the property of
  every entity of the kind that the pickers allow, times `factor` (1.1 is up 10%). "What if Riverside
  Roasters puts prices up 10%?" is `{"scale": {"kind": "product", "property": "price", "factor": 1.1,
  "related_to": {"entity_id": "<Riverside Roasters>", "label": "supplied_by"}}}`; `filters` are a metric's
  filters, such as `[{"property": "status", "op": "eq", "value": "Active"}]`.
- `{"shift": {...the same, with "delta"}}`: plus `delta` in the property's unit (0.3 is 30 cents in AUD).
- `{"remove": {"entity_id"}}`: the entity counts in no metric.

Only an approved property changes, a value must fit its datatype, and scale and shift take only number,
integer, money and percent properties. One scale or shift changes at most 500 entities; past that the rest
keep their real values and a warning says so. Only approved metrics run (at most 10), unless you pass
`allow_unapproved`. A call has a time budget of 20 seconds.

`reached` lists the entities within `depth` relations (default 2, at most 3) of the changed and removed
entities and of `impact_from` (up to 5 ids): each with its kind, depth, the relation it came through
(`via`, "Truck 7 carries Shipment 88"), the start it came `from` and the relation ids on the way. It is
plain data, as `entities_neighbourhood` is: you decide what the change affects and why.

"What if Truck 7 never arrives?" in the logistics vault below is `{"changes": [{"remove": {"entity_id":
"<Truck 7>"}}], "metrics": ["late_shipments"]}`. Truck 7 leaves every metric, and `reached` lists Shipment
88, the orders it holds and the customers who placed them. Whether a late-shipments count moves depends on
what the metric reads: removing the truck changes no shipment's status, so say that rather than invent a
number, and add a `set` of the shipment's `status` to `late` if that is the question.

### Coverage and readiness

Before answering a number or a what-if, call `ontology_coverage` with the user's question as asked (and
`vault_id` for the vault it is about). It picks the question's key phrases (what is measured, the kinds of
things involved, how they relate; never particular names, dates or numbers) and matches each to an approved
kind, property, metric or link type:

1. by name or synonym, word for word (`distance` 0);
2. with a model, by meaning: the nearest approved term by embedding, within cosine distance 0.6;
3. by a name or synonym inside a longer phrase ("oat milk price" names `price`).

Proposed and rejected terms never count. Without a model, the phrases are the runs of words between the
question words, matched by name and synonym only (`mode: "words"`).

```json
{"coverage": 0.5, "matched": [{"phrase": "selling price", "term": "property", "name": "price", "of_kind": "product", "distance": 0.407}],
 "uncovered": ["gross margin"], "suggestion": "Not covered by an approved kind, property, metric or link: \"gross margin\". ...", "mode": "model"}
```

When `uncovered` is not empty, the vault cannot answer that part yet: say so, do not compute or guess, and
suggest what to propose (`ontology_propose` a property and a metric on the kind it names) for an owner to
approve. The in-app Chat does this by itself.

`ontology_readiness` is the vault-level view of the same thing: counts of kinds, properties, metrics and
link types by status, `strict`, and `blockers` such as "3 properties are proposed and need an owner",
"metric avg_price on product uses a rejected property, price" or "no approved metric on kind order". The
same summary sits at the top of Kinds and links in the app (`GET /api/vaults/<id>/ontology/readiness`).

`docs/evals/ontology-tools.json` lists questions with the tool order an agent should follow (coverage
before `metric_eval`, `scenario_run` for a what-if); a test checks every tool it names is registered.

**Only approved kinds and links.** An owner can turn this on for a vault in Kinds and links (browser
session only; `POST /api/vaults/<id>/ontology/strict` `{"on": true}`). Then a write under a custom kind that
is not approved, or a relation under a link type that is not approved, is refused with `ontology.rejected`,
naming the approved kinds or links to use instead, and the kind or link type is filed as a proposal so the
owner sees what was wanted. Built-in kinds always pass. Extraction leaves refused items out, as it does for
rejected ones. It is off by default, so nothing changes for a vault until its owner turns it on.

### What a change affects

Nothing in Eunomia hardcodes what affects what. Your agent works it out from the vault's ontology
(its kinds and link types, with their descriptions and review status), the relations and the facts,
then shows the answer as a view. Say a logistics vault has `driver drives truck`, `truck carries
shipment` and `shipment fulfils order`, and the user says "Truck 7 is late, what does that affect?":

1. `entities_search` `{"query": "Truck 7"}` finds the truck's id.
2. `ontology_get` gives the link types. A description such as "a late truck delays what it carries"
   tells you which way a delay travels.
3. `entities_neighbourhood` `{"entity_id": "<truck id>"}` returns the entities within two relations,
   both directions: the driver, Shipment 88, Order 12. Each node has its latest facts, each edge its id,
   label and stored direction (`from` `label` `to`), and `link_types` the review of each type present.
4. You decide: the shipment is late because the truck carries it, the order because the shipment
   fulfils it; the driver is not delayed by the truck.
5. `show_view` draws it:

```json
{"view": "impact", "start_id": "<truck id>", "headline": "Truck 7 is two hours late.",
  "affected": [
    {"entity_id": "<shipment id>", "path": ["<carries edge id>"], "why": "It is on the truck."},
    {"entity_id": "<order id>", "path": ["<carries edge id>", "<fulfils edge id>"], "why": "Its only shipment is late."}
  ]}
```

Every id is checked: each entity and relation must be in a vault you can read, each relation's ends in the
same vault, each affected entity listed once, and each `path` a chain of relations from `start_id` to its
entity (either stored direction) that uses no relation twice and never comes back to an entity it passed.
A wrong id comes back as an error naming it; fix it and call again. `headline` and `why` are trimmed and capped, and secrets in
them are replaced as in a memory. The result is the view with names and kinds filled in, plus `text`:

```
Truck 7 is two hours late.
Starting from Truck 7 (truck).
1. Shipment 88 (shipment), via Truck 7 carries Shipment 88. It is on the truck.
2. Order 12 (order), via Truck 7 carries Shipment 88, then Shipment 88 fulfils Order 12. Its only shipment is late.
```

Over MCP the tool's `content` is only that `text`, so Claude Code shows readable prose rather than JSON.
The resolved view, as JSON with every id, is in the result's `structuredContent`. The in-app Chat follows the same steps with the same
tools and draws the view as cards above its reply. A `subgraph` view (`title`, `entity_ids`,
`relation_ids`) shows how some entities connect instead.

### Secret requests

Never ask the user to paste a password, API key or token into the chat. Ask Eunomia for it instead
([secret requests](secret-requests.md)):

1. `secret_request` with a stable `name` (`stripe_api_key`), a `label`, a `description` of why, and
   `target.hosts` it will be sent to (`api.stripe.com`). You get a `request_id`, a `fill_url` and the
   `handle` the secret will have, `{"$secret": "eu://<vault>/<name>"}`. Never a value.
2. The user sees a pop-up in the Eunomia web app (or opens `fill_url`), pastes the value there and saves.
   MCP clients that support URL elicitation (protocol 2025-11-25) are sent the page directly.
3. `secret_request_status` tells you when it is filled.
4. Use the handle in `http_request` (in a header by default), or derive another with `secret_transform`,
   for example `{"op": "template", "inputs": {"key": "eu://.../stripe_api_key"}, "params": {"template": "Bearer {{key}}"}}`.
   Responses come back with the value scrubbed. You never see it, and there is no tool that shows it.

### Agent skills

An agent can save a skill it learned (the same shape as a `SKILL.md`: a name, a description of when to use
it, and a Markdown body) and any agent can find it later:

- `skill_save` takes `name` (lowercase letters, digits and hyphens, at most 64 characters), `description`
  (at most 1024 characters) and `body` (Markdown, at most 64 KiB), and keeps it in your personal vault, or
  in `vault_id` (an id or a vault name). Saving the same name in the same vault replaces the skill. It
  needs write access to the vault. It embeds the name and description right away when an embedding
  provider is configured (the result says `embedded: true`). Without one it still saves, and the skill is
  found by keyword only, until it is saved again.
- `skill_find` takes a `query` in plain words (and optionally `vault_id` and `limit`, at most 50). It
  searches every vault you can read, by meaning when embeddings are on, plus a keyword match on name and
  description. It returns each skill's `id`, `name`, `description`, `vault` and `updated_at`, not the body.
- `skill_list` lists every skill of one vault (your personal vault, or `vault_id`; a token restricted to
  one vault lists that vault), sorted by name, with the same fields as `skill_find`. It pages: `limit`
  (default 100, at most 500) and `after`, the `next` of the previous call; `next` is `null` on the last
  page. It is read-only and needs only `memory:read`. `create-eunomia sync` uses it for the skills that
  are only on the server ([projects](projects.md#skills-sync)).
- `skill_get` returns the full body, by `id` or by `name`. If several vaults you can read hold that name,
  it answers with an error naming the ids; pass `vault_id` or an id.
- `skill_delete` removes one skill by id.

A vault you are not a member of, another org, or a token restricted to another vault never shows its
skills: they read as not found.

These are different from the built-in [memory skill](#the-memory-skill), the one set of instructions that
tells agents when to recall and what to remember. Agent skills are many, are written by agents, and live
in a vault like any other memory.

Known gap: `vault_clone` and `vault_merge` do not copy skills yet, and there is no page for them in the
web app.

### Change log

Every write that changes a vault (a memory or entity created, updated, merged or deleted, a relation
created, a skill saved or deleted, a document uploaded or deleted, what a vault clone or merge copies; the
full list and what is not logged are in [projects](projects.md#watching-changes)) adds one numbered row
to that vault's change log. `changes_since`
reads it: `after` (the `next` of your previous call, 0 to start), `limit` (at most 1000, default 100) and
`vault_id` (your personal vault by default). Each row has `seq`, `op`, `kind`, the item's `id`, the `actor`
(a user, token or OAuth client id, or `system`) and `at`. Rows never hold content: fetch the item to see it.
The log keeps the last 90 days and at most 100,000 rows per vault by default. A cursor below what is kept,
or past the newest change (after a restore), answers `changes.resync`: re-read what you need and start
again from `after=0` ([retention](projects.md#retention)).
The same log is `GET /api/vaults/{vault}/changes` over REST and a WebSocket for live updates
([projects](projects.md#watching-changes)).

### Changes show up in the web app by themselves

What an agent does over MCP shows in every open Eunomia tab within a few seconds, without a reload: a vault
created, renamed, cloned, merged or deleted, a member invited, removed or promoted, ownership handed over,
an invitation accepted or declined, and any memory, entity, relation or document written, edited, merged
or deleted. The same goes for changes made in chat, in another tab, or by another member of a shared vault.

Each tab opens one WebSocket, `GET /api/events/ws` (session cookie from an allowed origin, or a Bearer
token with read access). It sends JSON messages with a `type`: `hello` (each readable vault's last
change-log seq, on every connect), `change` (a [change-log](#change-log) row of any vault you can read),
`vault` (something happened to a vault itself: `op` is `created`, `renamed`, `deleted`, `cloned`,
`merged`, `member_invited`, `member_removed`, `role_changed`, `ownership_transferred`,
`invitation_accepted`, `invitation_declined` or `left`), `seq` (a vault's log moved on: refetch it),
`vaults` (the set of vaults you can read changed), `resync` (refetch everything) and `heartbeat` (every
10 seconds). Messages carry ids, never content. After a reconnect, `hello` tells the page which vaults
changed while it was away, so it refetches only those, plus the vault list.

The web app refetches only the views a message affects. While the socket is down, the vault list, the
graph, the memory map and the documents list check every 12 seconds instead (never in a hidden tab), and
the pages show **Up to date** or **Checking every 12 s** next to a **Refresh** button.

Limits: vault events are sent from the server process that made the change. With several backend
replicas, a page connected to another replica learns of a change to what a vault holds within about 10
seconds (its periodic check of the change logs), and of a vault-level change within about 30 seconds (its
membership recheck).

### What you can do in the app and what agents can do

Every tool that changes a vault has a matching action in the web app, except where noted.

| Tool | In the web app |
|---|---|
| `vault_create` / `vault_rename` / `vault_delete` / `vault_clone` / `vault_merge` | Memory spaces page |
| `vault_invite` / `vault_remove_member` / `vault_set_role` / `vault_leave` | Memory spaces page, a space's panel |
| `vault_transfer_ownership` | A space's panel: **Hand over this space** (owners), or the hand-over icon on a member |
| `vault_invitations` / `vault_accept_invitation` / `vault_decline_invitation` | Memory spaces page: **Join**, or the decline button next to it |
| `memory_write` / `memory_update` / `memory_delete` | People & topics: a person or topic's panel (**Add memory**, the pencil to edit, the cross to delete) |
| `entity_update` / `entity_merge` / `entity_delete` | People & topics: a person or topic's panel |
| `consolidate_observations` | People & topics: **Tidy up** in a person or topic's panel (one at a time; needs an AI model) |
| `document_*` | Documents page |
| `memory_write_batch` | None: bulk import is for agents and scripts. Upload a document instead |
| `code_entity_upsert` / `code_relate` | Code map shows them; only coding agents write them |
| `skill_*` | None yet (see [agent skills](#agent-skills)) |

The other direction has one deliberate gap: the built-in **chat** may only read and call `memory_write`
and `memory_update`. It cannot delete, merge, share, invite or change vaults, even when you ask it to,
because a chat reply should never be able to destroy or expose data (`CHAT_WRITE_TOOLS` in
`backend/src/chat/service.rs`). Do those from the pages above, or from an agent you connected.

### Importing a lot at once

Writing hundreds of facts one call at a time (say, 900 pages of notes) runs
into the per-token rate limit (`RATE_LIMIT_TOKEN_PER_MIN`, 600 by default):
the extra calls get `429 rate.limited` with `Retry-After`. Use
`memory_write_batch` instead. It takes up to 100 facts per call, writes each
exactly like `memory_write`, and returns one result per fact, so a bad item
fails alone. A batch costs one request per 10 facts, so 900 facts are 9 calls
and 90 requests, well inside a minute's limit. A CSV import (`document_import_rows`)
costs one request per 500 rows, counted once the file is read, so a 5,000-row
file is 10 requests. If a client still gets a 429,
it should wait `Retry-After` seconds and resend that batch.

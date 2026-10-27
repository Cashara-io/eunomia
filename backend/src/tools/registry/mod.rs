//! The one tool registry consumed by both the MCP server and the REST
//! surface. Merges: generic cache tools
//! (`tools::generic`) + per-domain tools (`entities`, `vaults`, `cache`),
//! wired up in [`register_all`].
//!
//! Layout: this file is the dispatch and audit core (`call`, `run_tool`, the audit row, the read-only
//! list, the shared argument helpers). The tools themselves register from one file per group:
//! `records`, `memory`, `entities`, `code`, `vaults`, `docs`; `admin` holds the REST-only audit read.
//!
//! Async end to end: handlers need to be `async fn` for axum concurrency, and the same shape serves MCP.
//!
//! Usage: every call, read-only or not, is also counted in `tool_usage` (`crate::usage`, one atomic
//! upsert in a spawned task, so it adds no latency and cannot fail the call).
//!
//! Audit trail: every call through [`call`] for a tool NOT in
//! [`READ_ONLY_TOOLS`] writes one `audit_log` row (owner, tool name, a
//! redacted/truncated summary of the args, outcome, timestamp) -- this is
//! the single choke point every tool call (REST and MCP) already goes
//! through, so hooking it in here means no per-tool instrumentation. A
//! read-only tool returning an `{"error": ...}` value (rather than failing)
//! still counts as "ok" for logging purposes since it's excluded from
//! logging entirely either way.

use std::collections::HashMap;
use std::future::IntoFuture;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use serde_json::{json, Value};
use surrealdb::types::RecordId;
use crate::rid::RecordIdExt;
use tracing::{field::Empty, Instrument};

use crate::audit::{self, Event};
use crate::authz;
use crate::pool::{ControlDb, OrgDb};
use crate::vaults::service::{list_my_vaults, VaultWithRole};
use crate::models_user::User;
use crate::scopes;
use crate::store;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::state::{AppState, OrgState};

mod admin;
mod code;
mod docs;
mod documents;
mod entities;
mod memory;
mod records;
mod skills;
mod vaults;

pub use admin::list_audit;

/// A registered tool handler: owner-scoped, takes the raw JSON args object,
/// returns a JSON result (an `{"error": ...}` value on a handled failure, or
/// an `Err` for anything that should surface as a 500 / propagate).
pub type ToolFn =
    Arc<dyn for<'a> Fn(&'a OrgState, &'a RecordId, Value) -> BoxFuture<'a, AppResult<Value>> + Send + Sync>;

pub type BoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

#[derive(Clone)]
pub struct ToolSpec {
    pub read_only: bool,
    pub schema: Value,
    pub handler: ToolFn,
}

/// Tools that only read data -- excluded from the audit log. Everything
/// else registered (now or later) is assumed mutating and gets logged; this
/// is a small explicit allowlist rather than a generic classifier, since the
/// tool count is small and known.
pub const READ_ONLY_TOOLS: &[&str] = &[
    "docs",
    "search",
    "get",
    "list",
    "links",
    "recall",
    "reflect",
    "entities_search",
    "entities_get",
    "entities_graph",
    "entity_types",
    "vault_list",
    "vault_members",
    "document_list",
    "document_get",
    "document_download",
    "document_export",
    "skill_find",
    "skill_get",
    "changes_since",
];

/// Read-only tools that still get an audit row: they disclose an original file or its vectors. The
/// row records the document id, never its content.
pub const DISCLOSURE_TOOLS: &[&str] = &["document_download", "document_export"];

/// Most facts one `memory_write_batch` call takes.
pub const MAX_BATCH_ITEMS: usize = 100;
/// Facts in a batch covered by the request's own rate-limit charge; each further group of this many
/// costs one more request ([`extra_cost`]).
pub const BATCH_ITEMS_PER_REQUEST: usize = 10;

/// Rate-limit units a call costs beyond the one request the gate already charged: a batch of facts
/// costs one request per [`BATCH_ITEMS_PER_REQUEST`] of them, so a bulk import is ten times cheaper
/// than one call per fact yet cannot slip past the limit.
pub fn extra_cost(name: &str, args: &Value) -> usize {
    match name {
        "memory_write_batch" => args["items"].as_array().map_or(0, |a| a.len().min(MAX_BATCH_ITEMS).div_ceil(BATCH_ITEMS_PER_REQUEST).saturating_sub(1)),
        _ => 0,
    }
}

pub fn is_read_only(name: &str) -> bool {
    READ_ONLY_TOOLS.contains(&name)
}

/// Tools that irreversibly remove data or access -- flagged to MCP clients
/// (`destructiveHint`) so they can ask before running them.
pub fn is_destructive(name: &str) -> bool {
    matches!(name, "memory_delete" | "entity_delete" | "entity_merge" | "vault_delete" | "vault_remove_member" | "vault_leave" | "document_delete" | "skill_delete")
}

/// Tools over the user's raw synced source records, which belong to no vault:
/// a vault-restricted token may not use them.
fn reads_source_records(name: &str) -> bool {
    matches!(name, "search" | "get" | "list" | "links")
}

/// What each tool does, for the model choosing between them (chat agent and
/// MCP clients alike). Every registered tool must have one -- see tests.
pub fn description(name: &str) -> &'static str {
    match name {
        "docs" => "Read Eunomia's own documentation: how to install, connect agents, every tool, vaults/memories/recall concepts, deployment. No arguments lists the docs; `topic` returns one in full.",
        "search" => "Search the user's synced source records (transactions, meetings, messages, ...) by keyword. Supports filtering by source, record type and time range. Returns record ids usable with `get` and `links`.",
        "get" => "Fetch one synced source record by id, with its full content. For a Pocket recording (or one of its transcript chunks) this includes the whole stored recording: full speaker-labelled transcript, summary, action items, speakers and tags.",
        "list" => "List synced source records, optionally filtered by type and field values, sorted and paginated. Use for browsing rather than searching.",
        "links" => "List the records linked to a given record (e.g. a meeting's attendees, a transaction's merchant), optionally only one relation type.",
        "recall" => "Retrieve the memories most relevant to a question from the user's memory graph, ranked by combining semantic, keyword, graph and recency signals within a token budget. Searches the personal vault and every org vault the user belongs to unless `vault_id` is given. Finds facts by meaning as well as by words, and follows one hop of relations from the entities you name (hits of kind `relation`). Each hit carries its vault, its date (`occurred_at`: when a memory happened if its writer gave `occurred_at`, else when it was last written) and a 0-1 score relative to the best hit -- check the date before trusting a time-sensitive fact. Superseded facts are left out unless `include_history` is true. Call it at the start of any task touching people, projects, past decisions or code you have worked on.",
        "reflect" => "Answer a question by synthesizing from recalled memories (the personal vault and every org vault the user belongs to, unless `vault_id` is given), with numbered citations; newer memories win conflicts. Answers only from what is already remembered; use `recall` to inspect the raw memories instead.",
        "entities_search" => "Find entities (people, organisations, locations, repositories, files, symbols, and any custom kind) by name or alias, then by the text of their facts (a username or handle only ever written in a fact still finds its entity). Searches the personal vault and every org vault the user belongs to unless `vault_id` is given. Returns entity ids for `entities_get`, `memory_write` and `code_relate`.",
        "entity_types" => "List the entity kinds: the six built-in ones (person, organisation, location, repository, file, symbol) and the custom kinds agents have created (e.g. `db`, `table`, `decision`), each with its entity count. Check it before inventing a new kind, so the same thing keeps one kind name. Reads the personal vault and every org vault the user belongs to unless `vault_id` is given.",
        "entities_get" => "Get one entity with its memories (facts and consolidated observations) and its relations to other entities.",
        "entities_graph" => "Get the entity relationship graph (nodes and edges), optionally restricted to some entity kinds or a vault.",
        "code_entity_upsert" => "Find-or-create a code entity -- a repository, file or symbol -- matched case-insensitively by name, with a short summary. `parent_id` links a file to its repository or a symbol to its file. Returns the entity id.",
        "code_relate" => "Record a labelled relation between two entities, e.g. symbol `calls` symbol, file `imports` file.",
        "memory_write" => "Remember a fact about a person, organisation, location, code entity or anything else (created if new; \"Jane Doe (Acme/JD)\" resolves to an existing \"Jane Doe\", adding the parts as aliases). `subject_kind` is a built-in kind or your own lowercase kind name (`db`, `table`, `field`, `defect`, `decision`, `task`, ...): a new name creates that kind in the vault (at most 50 custom kinds per vault; `entity_types` lists them, reuse one when it fits). Type `world` for objective facts, `experience` for things that happened, `observation` for stable patterns. Writes to the personal vault unless `vault_id` is given. Earlier facts about the subject this one makes no longer true are marked superseded in the background (left out of recall from then on). When a new entity was created, the result lists `possible_duplicates`: similar existing entities to `entity_merge`. Save durable facts, decisions, preferences and corrections as they come up, and before finishing a task a short summary of what was learned (`experience`).",
        "memory_write_batch" => "Remember many facts in one call (up to 100), each exactly as `memory_write` would (same fields per item; a batch-level `vault_id` applies to items without their own). Use it to import or back-fill: a page, a document's notes or a whole export, instead of one call per fact. Returns one result per item in input order (`ok`, `memory_id`, or `error` and `code`); a failed item does not stop the rest. Costs one request against the rate limit per 10 facts.",
        "consolidate_observations" => "Fold an entity's new raw facts into its consolidated observation (or do it for every entity with new facts when `subject_id` is omitted).",
        "memory_update" => "Edit one memory by id: change its `text`, and/or switch a raw fact between `world` and `experience`. To revise an entity's consolidated belief, `memory_write` with type `observation` instead.",
        "entity_update" => "Edit an entity's `name`, `aliases` (replaces the list) and/or `summary`.",
        "memory_delete" => "Delete one memory by id. Irreversible.",
        "entity_delete" => "Delete an entity and its memories and relations. Irreversible.",
        "entity_merge" => "Merge a duplicate entity (`loser_id`) into another of the same kind (`winner_id`): memories and relations move to the winner, the loser's name becomes an alias, and the loser is deleted.",
        "vault_create" => "Create a vault: a separate scope for entities and memories, for whatever the user wants kept apart (a project or service, a client, a team, a homelab, ...). Kind `org` = any non-personal vault. The caller owns it. Every tool's `vault_id` accepts the vault's name, e.g. `vault_id: \"Acme\"`.",
        "vault_list" => "List the vaults the user belongs to (personal plus any they've made or joined), with their role in each. Writes go to the personal vault unless `vault_id` (an id or the vault's name) is given; `recall`, `reflect` and `entities_search` search the personal vault plus every org vault unless it is.",
        "vault_clone" => "Copy a vault's entities and memories into a new vault owned by the caller.",
        "vault_merge" => "Merge two vaults you belong to into a NEW vault: both are copied into it (the originals are never changed), and entities with the same kind and name are folded together -- aliases unioned, identical facts kept once, relations deduplicated.",
        "vault_invite" => "Invite a registered user by email to a vault (owner only). They must accept the invitation before getting access.",
        "vault_members" => "List a vault's members and their roles.",
        "vault_remove_member" => "Remove a member from a vault, or withdraw a pending invitation (owner only). Cannot remove the last owner.",
        "vault_leave" => "Leave a vault. The last owner can't leave while others remain.",
        "vault_rename" => "Rename a vault (owner only).",
        "vault_delete" => "Delete a vault and all memberships (owner only). Irreversible.",
        "document_upload" => "Store a document (plain text, Markdown, JSON, CSV, or a text-based PDF as content_base64) in the user's personal vault: the original is kept for download and its text is chunked and indexed in the background, so `search`, `recall` and `get` find its passages (each hit names the document) and entities are extracted from it like synced data. Status starts `indexing`; `document_get` shows when it is `ready`. Pass `document_id` to replace a document with a new version.",
        "document_list" => "List the user's uploaded documents, newest first, with type, size, SHA-256, indexing status and chunk count.",
        "document_get" => "One uploaded document: its metadata, the extracted text, and each chunk's id (usable with `get`) and character range.",
        "document_download" => "How to download an uploaded document's original bytes: an authenticated HTTP GET (the bytes never pass through the tool), with the size and SHA-256 to verify them.",
        "document_export" => "An uploaded document's export manifest: every chunk with its text, location and whether it has an embedding, plus the embedding model and dimension. Vectors are left out unless `include_vectors` is true; `vectors_url` serves the full manifest over HTTP.",
        "document_delete" => "Delete an uploaded document: its stored file, its chunks (they leave search, recall and get at once) and the memories extracted from them. Irreversible.",
        "skill_save" => "Save a skill you learned (a name, a description of when to use it, and a Markdown body) in the user's personal vault, or in `vault_id`. Saving a name again replaces that skill. It is embedded so `skill_find` finds it by meaning. These are agent skills kept in Eunomia, not the built-in memory skill.",
        "skill_find" => "Find saved skills by what you need to do: by meaning when embeddings are on, and by keyword in a skill's name or description. Searches every vault you can read, or one `vault_id`. Returns id, name, description, vault and updated_at, not the body: `skill_get` reads it.",
        "skill_get" => "One saved skill with its full Markdown body, by id (from `skill_find`) or by name (an error names the ids if several vaults hold that name).",
        "skill_delete" => "Delete one saved skill by id. Irreversible.",
        "changes_since" => "A vault's change log after a cursor: one numbered row per write (seq, op created/updated/deleted/merged, kind memory/entity/relation/skill/document, the item's id, who wrote it, when), oldest first, with no content (fetch the item to see it). Pass the returned `next` as `after` to read on. Your personal vault unless `vault_id` is given.",
        _ => "",
    }
}

// args keys that look like secrets -- redacted rather than written to the
// audit log's args_summary.
const SECRET_KEY_HINTS: &[&str] = &["password", "token", "secret", "credential", "api_key", "apikey"];
const ARGS_SUMMARY_MAX: usize = 500;

// a document's or skill's content, kept out of the audit summary (its size stands in for it)
const CONTENT_KEYS: &[&str] = &["text", "content_base64", "body"];

fn summarize_args(tool_name: &str, args: &Value) -> String {
    let mut redacted = serde_json::Map::new();
    if let Some(obj) = args.as_object() {
        for (k, v) in obj {
            let lower = k.to_lowercase();
            if (tool_name.starts_with("document_") || tool_name.starts_with("skill_")) && CONTENT_KEYS.contains(&k.as_str()) {
                redacted.insert(k.clone(), json!(format!("<{} chars>", v.as_str().map_or(0, |s| s.chars().count()))));
            } else if SECRET_KEY_HINTS.iter().any(|hint| lower.contains(hint)) {
                redacted.insert(k.clone(), json!("***"));
            } else {
                redacted.insert(k.clone(), v.clone());
            }
        }
    }
    // the args hold the raw text a memory write redacts, so the audit row redacts it too
    let mut summary = crate::redact::redact(&Value::Object(redacted).to_string(), &mut Vec::new());
    if summary.chars().count() > ARGS_SUMMARY_MAX {
        summary = summary.chars().take(ARGS_SUMMARY_MAX).collect::<String>() + "\u{2026}";
    }
    summary
}

/// The owner-facing `audit_log` row (what `GET /api/audit` shows) plus the
/// append-only `audit_event` row with the actor, the outcome code and the trace id.
pub(crate) async fn record_audit(state: &OrgState, owner: &RecordId, tool_name: &str, args: &Value, outcome: &str, code: &str) {
    let summary = summarize_args(tool_name, args);
    let detail = control_detail(args);
    // the two rows go to different databases: write them side by side, not one after the other
    let org_row = store::cache::RECORD_AUDIT
        .on(&state.db)
        .bind(("owner", owner.clone()))
        .bind(("tool_name", tool_name.to_string()))
        .bind(("args_summary", summary))
        .bind(("outcome", outcome.to_string()));
    // the control database is shared across orgs: no tenant text, only the argument names and a hash
    let _ = tokio::join!(org_row.into_future(), record_event(&state.control, owner, tool_name, code, &detail));
}

/// What the cross-org `audit_event` keeps of a call's arguments: their keys and a hash, never values.
fn control_detail(args: &Value) -> String {
    let keys = args.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>().join(",")).unwrap_or_default();
    format!("keys={keys} hash={}", crate::models_user::hash_token(&args.to_string()))
}

async fn record_event(db: &ControlDb, owner: &RecordId, tool_name: &str, code: &str, detail: &str) {
    let actor = authz::current().map(|c| c.actor).unwrap_or(authz::Actor { kind: "user", id: owner.to_string() });
    let action = format!("tool.{tool_name}");
    audit::record(db, Event { user: Some(owner), actor: &actor, action: &action, target: "", outcome: code, detail }).await;
}

/// The process-wide tool registry: a `OnceLock<HashMap<..>>` filled once, explicitly, by `register_all` below.
static REGISTRY: OnceLock<HashMap<&'static str, ToolSpec>> = OnceLock::new();

/// Used by anything adding tools before first use (called once from `register_all`).
pub fn register(registry: &mut HashMap<&'static str, ToolSpec>, name: &'static str, schema: Value, handler: ToolFn) {
    registry.insert(name, ToolSpec { read_only: is_read_only(name), schema, handler });
}

/// Builds the full tool map by delegating to `register_all`.
fn build_registry() -> HashMap<&'static str, ToolSpec> {
    let mut registry = HashMap::new();
    register_all(&mut registry);
    registry
}

/// Arguments of the tools that take just a record id (`get`, `entities_get`).
#[derive(serde::Deserialize)]
struct IdArgs {
    id: String,
}

/// A JSON-decode failure for a tool's `args` object: bad agent input is an `{"error": ...}` value, not a 500.
fn bad_args(e: serde_json::Error) -> Value {
    json!({ "error": format!("invalid arguments: {e}") })
}

fn bad_id(field: &str, value: &str) -> Value {
    json!({ "error": format!("invalid {field}: {value:?}") })
}

/// Parses a tool argument string into a `RecordId`, yielding an `{"error":
/// ...}` value (not an `Err`) on failure: bad agent input never
/// raises, it comes back as a value.
fn parse_rid(field: &str, value: &str) -> Result<RecordId, Value> {
    crate::rid::parse(value).map_err(|_| bad_id(field, value))
}

fn parse_opt_rid(field: &str, value: &Option<String>) -> Result<Option<RecordId>, Value> {
    match value {
        None => Ok(None),
        Some(v) => parse_rid(field, v).map(Some),
    }
}

/// The vaults a read names: `vault_id` and every one of `vault_ids` (names were already resolved to
/// ids). Empty: the default scope.
fn parse_vault_ids(vault_id: &Option<String>, vault_ids: &Option<Vec<String>>) -> Result<Vec<RecordId>, Value> {
    vault_id.iter().chain(vault_ids.iter().flatten()).map(|v| parse_rid("vault_ids", v)).collect()
}

/// The `vault_ids` property of a read tool's schema.
/// An entity kind: a built-in one, or the agent's own lowercase name (validated by
/// `entities::service::kind_table`), so it is a pattern rather than an enum.
fn kind_schema() -> Value {
    json!({
        "type": "string",
        "pattern": "^[a-z][a-z0-9_]{0,31}$",
        "description": format!(
            "built-in: {}; or your own lowercase kind name, e.g. db, table, field, defect, decision, task (`entity_types` lists the ones in use)",
            crate::entities::service::KINDS.join(", ")
        ),
    })
}

fn vault_ids_schema() -> Value {
    json!({"type": "array", "items": {"type": "string"}, "description": "read these vaults together (ids or names); with `vault_id`, both are read"})
}

/// Converts a typed service-layer result into the tool-call `Value` shape:
/// `Ok` serializes to JSON, `Err` becomes `{"error": <message>}`, so a
/// handled service error never raises past the registry.
fn to_tool_value<T: serde::Serialize>(result: AppResult<T>) -> Value {
    match result {
        Ok(v) => serde_json::to_value(v).unwrap_or_else(|e| json!({ "error": e.to_string() })),
        Err(e) => e.to_tool_value(),
    }
}

/// Registers every tool: the four generic cache tools, the
/// entity-memory tools, `recall`/`reflect`, and vault management. Per-source
/// tools (`sources::registry`) aren't wired in yet -- that module doesn't
/// exist on the Rust side.
fn register_all(registry: &mut HashMap<&'static str, ToolSpec>) {
    docs::register_all(registry);
    documents::register_all(registry);
    records::register_all(registry);
    memory::register_all(registry);
    entities::register_all(registry);
    code::register_all(registry);
    vaults::register_all(registry);
    skills::register_all(registry);
}

pub fn all_tools() -> &'static HashMap<&'static str, ToolSpec> {
    REGISTRY.get_or_init(build_registry)
}

/// Calls a registered tool by name, applying the audit-log side effect for
/// anything not in [`READ_ONLY_TOOLS`]. Returns `{"error": "unknown tool
/// ..."}`` for a name that isn't registered, rather than a 404 -- the REST router is the one that turns an unknown
/// name into a 404 before ever calling this.
pub async fn call(app: &AppState, user: &User, name: &str, mut args: Value) -> AppResult<Value> {
    let owner = &user.id;
    let tools = all_tools();
    let Some(spec) = tools.get(name) else {
        return Ok(AppError::coded(ErrorCode::ToolNotFound, format!("unknown tool {name}")).to_tool_value());
    };

    let needed = authz::require_scope(scopes::for_tool(name, is_read_only(name))).and_then(|()| {
        if reads_source_records(name) { authz::require_unrestricted() } else { Ok(()) }
    });
    if let Err(e) = needed {
        record_event(&app.control, owner, name, e.code.as_str(), "").await;
        count_call(app, user, name, true, 0);
        return Err(e);
    }
    let state = &app.org(&user.org).await?;
    if let Err(e) = resolve_vault_names(&state.db, owner, &mut args).await {
        return Ok(json!({ "error": e }));
    }

    let vault = args.get("vault_id").and_then(Value::as_str).unwrap_or("personal").to_string();
    let span = tracing::info_span!("tool.call", tool = name, vault = %vault, outcome = Empty, duration_ms = Empty);
    let started = Instant::now();
    let source = Default::default();
    let captured = args.clone();
    // Boxed: the match over every tool is a large future, past a 2 MB stack in debug builds.
    let mut result = crate::error::TOOL_SOURCE.scope(Arc::clone(&source), Box::pin(run_tool(state, owner, name, spec, args))).instrument(span.clone()).await;

    // Every failed tool value carries a stable `code` and the request's `trace_id`.
    if let Ok(Value::Object(map)) = &mut result
        && map.contains_key("error")
    {
        map.entry("code").or_insert_with(|| ErrorCode::ValidationInvalid.as_str().into());
        map.entry("trace_id").or_insert_with(|| crate::telemetry::current_trace_id().into());
    }
    let outcome = match &result {
        Ok(v) if v.get("error").is_some() => v.get("code").and_then(Value::as_str).unwrap_or("error"),
        Ok(_) => "ok",
        Err(e) => e.code.as_str(),
    };
    span.record("outcome", outcome);
    span.record("duration_ms", started.elapsed().as_secs_f64() * 1000.0);
    span.in_scope(|| tracing::info!(outcome, "tool call"));
    let source = source.lock().unwrap().take();
    let errored = !matches!(&result, Ok(v) if v.get("error").is_none());
    count_call(app, user, name, errored, started.elapsed().as_millis() as u64);
    record_failure(state, owner, name, captured, &result, source).await;
    result
}

/// Counts the call in the usage table (`crate::usage`), read-only or not. Spawned: it never waits.
fn count_call(app: &AppState, user: &User, tool: &str, errored: bool, ms: u64) {
    crate::usage::record(&app.control, crate::usage::Sample { org: user.org.to_string(), user: user.id.clone(), tool: tool.to_string(), errored, ms });
}

const VAULT_ARGS: &[&str] = &["vault_id", "vault_id_a", "vault_id_b"];
/// Arguments that are a list of vaults.
const VAULT_LIST_ARGS: &[&str] = &["vault_ids"];

/// A vault given by name ("Acme", "personal") -> its id, among the caller's
/// vaults, case-insensitively. Ids (`vault:...`) pass through unchanged.
fn match_vault(given: &str, vaults: &[VaultWithRole]) -> Result<String, String> {
    if given.starts_with("vault:") {
        return Ok(given.to_string());
    }
    let want = given.trim().to_lowercase();
    let hits: Vec<&VaultWithRole> = vaults.iter().filter(|v| v.name.to_lowercase() == want || (v.kind == "personal" && want == "personal")).collect();
    match hits.as_slice() {
        [one] => Ok(one.id.clone()),
        [] => Err(format!("no vault named {given:?}: vault_list shows yours, vault_create makes one")),
        many => Err(format!("several vaults are named {given:?}; pass one of these ids: {}", many.iter().map(|v| v.id.as_str()).collect::<Vec<_>>().join(", "))),
    }
}

/// Lets every tool take a vault by name wherever it takes a vault id, so an
/// agent can keep separate areas (projects, clients, ...) apart without tracking ids.
/// `list_my_vaults` already honours a vault-restricted token, so a name outside it never resolves.
async fn resolve_vault_names(db: &OrgDb, owner: &RecordId, args: &mut Value) -> Result<(), String> {
    let Some(obj) = args.as_object_mut() else { return Ok(()) };
    let by_name = |v: &Value| v.as_str().is_some_and(|v| !v.starts_with("vault:"));
    if !VAULT_ARGS.iter().any(|k| obj.get(*k).is_some_and(by_name))
        && !VAULT_LIST_ARGS.iter().any(|k| obj.get(*k).and_then(Value::as_array).is_some_and(|a| a.iter().any(by_name)))
    {
        return Ok(());
    }
    let vaults = list_my_vaults(db, owner).await.map_err(|e| e.message)?;
    for key in VAULT_ARGS {
        if let Some(given) = obj.get(*key).and_then(Value::as_str) {
            let id = match_vault(given, &vaults)?;
            obj.insert(key.to_string(), Value::String(id));
        }
    }
    for key in VAULT_LIST_ARGS {
        if let Some(list) = obj.get_mut(*key).and_then(Value::as_array_mut) {
            for item in list.iter_mut() {
                if let Some(given) = item.as_str() {
                    *item = Value::String(match_vault(given, &vaults)?);
                }
            }
        }
    }
    Ok(())
}

/// A capsule for any failure but bad input, so `eunomia replay` can reproduce it.
async fn record_failure(state: &OrgState, owner: &RecordId, name: &str, args: Value, result: &AppResult<Value>, source: Option<String>) {
    let (code, text) = match result {
        Ok(v) if v.get("error").is_some() => {
            let code = v.get("code").and_then(Value::as_str).and_then(|c| ErrorCode::ALL.iter().find(|e| e.as_str() == c));
            (code.copied().unwrap_or(ErrorCode::ValidationInvalid), v["error"].as_str().unwrap_or_default().to_string())
        }
        Err(e) => (e.code, e.source.clone().unwrap_or_else(|| e.message.clone())),
        Ok(_) => return,
    };
    if code == ErrorCode::ValidationInvalid {
        return;
    }
    let status = code.default_status().as_u16();
    let failure = crate::capsules::Failure {
        org: Some(state.db.org().key()),
        kind: "tool",
        name: name.to_string(),
        user: Some(owner.to_string()),
        args,
        code,
        status,
        source: source.unwrap_or(text),
    };
    crate::capsules::record(&state.control, failure).await;
}

async fn run_tool(state: &OrgState, owner: &RecordId, name: &str, spec: &ToolSpec, args: Value) -> AppResult<Value> {
    if is_read_only(name) && !DISCLOSURE_TOOLS.contains(&name) {
        return (spec.handler)(state, owner, args).await;
    }

    let result = (spec.handler)(state, owner, args.clone()).await;
    let (outcome, code) = match &result {
        Ok(v) if v.get("error").is_some() => ("error", v.get("code").and_then(Value::as_str).unwrap_or("error")),
        Ok(_) => ("ok", "ok"),
        Err(e) => ("error", e.code.as_str()),
    };
    record_audit(state, owner, name, &args, outcome, code).await;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault(id: &str, name: &str, kind: &str) -> VaultWithRole {
        VaultWithRole { id: id.into(), name: name.into(), kind: kind.into(), created_at: None, role: "owner".into() }
    }

    #[test]
    fn vaults_resolve_by_name_case_insensitively() {
        let vs = [vault("vault:p", "Midhun", "personal"), vault("vault:a", "Acme Corp", "org"), vault("vault:b", "Beta", "org")];
        assert_eq!(match_vault("acme corp", &vs).unwrap(), "vault:a");
        assert_eq!(match_vault(" Beta ", &vs).unwrap(), "vault:b");
        assert_eq!(match_vault("personal", &vs).unwrap(), "vault:p");
        assert_eq!(match_vault("vault:zzz", &vs).unwrap(), "vault:zzz"); // ids pass through untouched
        assert!(match_vault("Gamma", &vs).unwrap_err().contains("no vault named"));
        let dup = [vault("vault:1", "Acme", "org"), vault("vault:2", "acme", "org")];
        assert!(match_vault("Acme", &dup).unwrap_err().contains("vault:1, vault:2"));
    }

    #[test]
    fn every_tool_has_the_intended_scope() {
        for name in all_tools().keys() {
            let want = if is_read_only(name) {
                scopes::MEMORY_READ
            } else if name.starts_with("vault_") {
                scopes::VAULTS_ADMIN
            } else {
                scopes::MEMORY_WRITE
            };
            assert_eq!(scopes::for_tool(name, is_read_only(name)), want, "{name}");
        }
    }

    #[test]
    fn read_only_tools_are_exactly_the_expected_set() {
        let expected = [
            "docs",
            "search",
            "get",
            "list",
            "links",
            "recall",
            "reflect",
            "entities_search",
            "entities_get",
            "entities_graph",
            "entity_types",
            "vault_list",
            "vault_members",
            "document_list",
            "document_get",
            "document_download",
            "document_export",
            "skill_find",
            "skill_get",
            "changes_since",
        ];
        assert_eq!(READ_ONLY_TOOLS.len(), expected.len());
        for name in expected {
            assert!(is_read_only(name), "{name} should be read-only");
        }
    }

    #[test]
    fn mutating_tool_names_are_not_read_only() {
        for name in ["vault_create", "vault_delete", "vault_invite", "vault_rename", "unknown_tool"] {
            assert!(!is_read_only(name), "{name} should not be read-only");
        }
    }

    #[test]
    fn every_registered_tool_has_a_description() {
        for name in all_tools().keys() {
            assert!(!description(name).is_empty(), "{name} needs a description in registry::description");
        }
    }

    #[test]
    fn destructive_tools_are_never_read_only() {
        for name in all_tools().keys().filter(|n| is_destructive(n)) {
            assert!(!is_read_only(name), "{name}");
        }
    }

    #[test]
    fn summarize_args_redacts_secret_looking_keys() {
        let args = json!({ "password": "hunter2", "api_key": "abc", "query": "hello" });
        let summary = summarize_args("memory_write", &args);
        assert!(!summary.contains("hunter2"));
        assert!(!summary.contains("abc"));
        assert!(summary.contains("hello"));
        assert!(summary.contains("***"));
    }

    #[test]
    fn summarize_args_truncates_long_summaries() {
        let long_value = "x".repeat(1000);
        let args = json!({ "q": long_value });
        let summary = summarize_args("memory_write", &args);
        assert!(summary.chars().count() <= ARGS_SUMMARY_MAX + 1);
        assert!(summary.ends_with('\u{2026}'));
    }

    #[test]
    fn summarize_args_handles_non_object_args() {
        let summary = summarize_args("search", &Value::Null);
        assert_eq!(summary, "{}");
    }

    #[test]
    fn document_content_never_reaches_the_audit_summary() {
        let args = json!({ "filename": "n.md", "text": "secret plan", "content_base64": "c2VjcmV0" });
        let summary = summarize_args("document_upload", &args);
        assert!(!summary.contains("secret plan") && !summary.contains("c2VjcmV0") && summary.contains("n.md"), "{summary}");
        assert!(summary.contains("<11 chars>"));
    }

    #[test]
    fn skill_body_never_reaches_the_audit_summary() {
        let summary = summarize_args("skill_save", &json!({ "name": "n", "body": "secret steps" }));
        assert!(!summary.contains("secret steps") && summary.contains("<12 chars>"), "{summary}");
    }

    #[test]
    fn registry_has_every_tool_wired_up() {
        let expected = [
            "docs", "search", "get", "list", "links", "recall", "reflect", "entities_search", "entities_get",
            "entities_graph", "entity_types", "code_entity_upsert", "code_relate", "memory_write", "memory_write_batch", "consolidate_observations",
            "memory_update", "entity_update", "memory_delete", "entity_delete", "entity_merge", "vault_create", "vault_list", "vault_clone", "vault_merge",
            "vault_invite", "vault_members", "vault_remove_member", "vault_leave", "vault_rename", "vault_delete",
            "document_upload", "document_list", "document_get", "document_download", "document_export", "document_delete",
            "skill_save", "skill_find", "skill_get", "skill_delete", "changes_since",
        ];
        let tools = all_tools();
        for name in expected {
            assert!(tools.contains_key(name), "{name} should be registered");
        }
        assert_eq!(tools.len(), expected.len());
    }
}

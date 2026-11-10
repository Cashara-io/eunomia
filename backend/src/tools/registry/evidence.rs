//! Evidence provenance for tool results. Paths describe request echoes retained for API compatibility.
//! New echoes belong under `_request`. Never infer provenance by comparing argument values or digits.

use serde_json::Value;

const COMMON: &[&str] = &[
    "error", "warnings", "args", "arguments", "parameters",
    "filters", "reach", "query", "question", "queries", "top_n", "as_of",
    "cursor", "next_cursor", "limit", "offset",
];
const METRIC: &[&str] = &["expr_words", "metric_mapped"];
const SCENARIO: &[&str] = &["changes", "changed/*/after", "baseline/*/expr_words", "scenario/*/expr_words"];
const VIEW: &[&str] = &[
    "text", "headline", "title", "affected/*/why", "spec", "evidence", "pagination/reach",
    "bucket_column", "applied_dates",
];

#[derive(Clone, Copy)]
pub struct Policy { paths: &'static [&'static str], view: bool }

/// Every tool has an explicit policy. Unregistered tools fail closed in the collector.
/// Exclude only fields that can echo this request. Independent receipts remain evidence.
/// Later reads of same-turn writes are tracked separately: Cashara-io/eunomia#215.
pub fn for_tool(name: &str) -> Option<Policy> {
    let paths: &'static [&'static str] = match name {
        "show_view" => VIEW,
        "metric_eval" => METRIC,
        "scenario_run" => SCENARIO,
        "ontology_coverage" => &["matched/*/phrase", "needs_metric/*/phrase", "uncovered", "suggestion"],
        "entities_neighbourhood" => &["depth"],
        "reflect" => &["answer"],
        "docs" | "search" | "get" | "list" | "links" | "recall"
        | "entities_search" | "entities_get" | "entities_graph" | "entity_types"
        | "ontology_get" | "ontology_readiness" | "vault_list" | "vault_members"
        | "vault_invitations" | "vault_get" | "vault_check" | "document_list"
        | "document_get" | "document_download" | "document_export" | "skill_find"
        | "skill_list" | "skill_get" | "changes_since" | "vault_peers" | "secret_request_status" => &[],
        "memory_write" => &["entity/name", "entity/kind", "entity/aliases", "memory/text", "memory/type", "memory/source", "kind_mapped/from"],
        "memory_write_batch" => &["results/*/error", "results/*/kind_mapped/from"],
        "memory_update" => &["text", "type"],
        "code_entity_upsert" => &["name", "kind", "summary"],
        "entity_update" => &["name", "aliases", "summary"],
        "code_relate" | "entity_relate" => &["in", "out", "label", "source"],
        "entity_set_property" => &[ "value/property", "value/value", "value/unit", "value/as_of", "value/source", "property_mapped/from"],
        "consolidate_observations" => &["errors"],
        "ontology_propose" => &[
            "kinds/*/kind", "kinds/*/merge_into", "links/*/from_kind", "links/*/label", "links/*/to_kind",
            "properties/*/kind", "properties/*/name", "properties/*/datatype", "properties/*/merge_into",
            "metrics/*/kind", "metrics/*/name", "metrics/*/merge_into",
        ],
        // These tools profile stored records and propose with the server's model, not caller values.
        "ontology_suggest" => &[],
        "ontology_automap" => &["profiles/*/source", "profiles/*/record_type", "key_matches/*/source", "key_matches/*/record_type", "record_types/*/source", "record_types/*/record_type", "links/*/source", "links/*/record_type"],
        "document_import_rows" => &["document_id", "kind", "columns/*/column", "columns/*/property"],
        "document_upload" => &["filename", "media_type"],
        "skill_save" => &["name"],
        "vault_create" | "vault_clone" | "vault_merge" => &["name", "kind"],
        "vault_rename" => &["name"],
        "vault_invite" => &["vault_id", "user_email", "role"],
        "vault_set_role" => &["email", "role"],
        "vault_transfer_ownership" => &["email"],
        "vault_share_code" => &["role"],
        "vault_join_shared" => &["name"],
        "secret_transform" => &["name", "op", "lineage", "allowed_hosts", "placements", "allowed_tools"],
        "memory_delete" | "entity_delete" | "entity_merge" | "document_delete" | "skill_delete"
        | "vault_delete" | "vault_remove_member" | "vault_leave" | "vault_accept_invitation"
        | "vault_decline_invitation" | "vault_repair" | "vault_peer_sync"
        | "secret_request" | "http_request" => &[],
        _ => return None,
    };
    Some(Policy { paths, view: name == "show_view" })
}

fn matches(pattern: &[&str], path: &[&str]) -> bool {
    match (pattern.split_first(), path.split_first()) {
        (None, None) => true,
        (Some((&"**", rest)), _) => matches(rest, path) || path.split_first().is_some_and(|(_, tail)| matches(pattern, tail)),
        (Some((part, rest)), Some((key, tail))) if *part == "*" || part == key => matches(rest, tail),
        _ => false,
    }
}
impl Policy {
    /// A projection for evidence only. The actual API result is never changed.
    pub fn computed(self, value: &Value) -> Value {
        fn walk(policy: Policy, value: &Value, path: &mut Vec<String>) -> Option<Value> {
            let parts: Vec<_> = path.iter().map(String::as_str).collect();
            // Only the reserved key applies everywhere. Legacy echo paths are relative to
            // a tool result or a resolved view source, never to arbitrary record payloads or rows.
            if parts.contains(&"_request") { return None; }
            let scope = if policy.view && parts.len() >= 3 && parts[..2] == ["data", "sources"] { &parts[3..] } else { &parts[..] };
            let (scope, paths) = if policy.view {
                match scope.split_first() {
                    Some((&"metric", rest)) => (rest, METRIC),
                    Some((&"result", rest)) => (rest, SCENARIO),
                    _ => (scope, policy.paths),
                }
            } else { (scope, policy.paths) };
            if COMMON.iter().chain(paths).any(|p| matches(&p.split('/').collect::<Vec<_>>(), scope)) { return None; }
            Some(match value {
                Value::Object(m) => Value::Object(m.iter().filter_map(|(k, v)| {
                    path.push(k.clone());
                    let v = walk(policy, v, path);
                    path.pop();
                    v.map(|v| (k.clone(), v))
                }).collect()),
                Value::Array(a) => Value::Array(a.iter().enumerate().map(|(i, v)| {
                    path.push(i.to_string());
                    let v = walk(policy, v, path).unwrap_or(Value::Null);
                    path.pop();
                    v
                }).collect()),
                _ => value.clone(),
            })
        }
        walk(self, value, &mut Vec::new()).unwrap_or(Value::Null)
    }
}

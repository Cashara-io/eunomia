//! Statements for model spend, the organisation's own provider and the AI switch (control migration 0016).
//! See `ai/`. Every statement names `$org`: the control database is shared by every organisation, so an
//! organisation's id is always part of the question and never inferred.

use super::ControlStmt;

pub const CONTROL_ALL: &[&ControlStmt] = &[
    &CONFIG_GET,
    &CONFIGS_BY_ORG,
    &CONFIG_SET_PROVIDER,
    &CONFIG_CLEAR_PROVIDER,
    &CONFIG_SET_ENABLED,
    &CONFIG_SET_ALLOWANCE,
    &HOLD_SET,
    &HOLD_CLEAR,
    &USAGE_ADD,
    &SPEND_MONTH,
    &SPEND_MONTH_BY_ORG,
    &SPEND_MONTH_FOR_ORGS,
    &ALLOWANCES,
    &USAGE_MONTH,
    &USAGE_MONTH_CALLS,
    &HELD_ORGS,
    &PRICES_ALL,
    &PRICE_SET,
    &PRICE_DELETE,
];

/// One organisation's AI settings (no row: everything at its default, so the server's key, no allowance, on).
pub const CONFIG_GET: ControlStmt = ControlStmt::new(
    "ai.config_get",
    "SELECT enabled, base_url, api_key_enc, chat_model, embedding_model, allowance_micros, hold_until FROM org_ai WHERE org = $org LIMIT 1",
);

/// The allowance and switch of a page of organisations, for the operator's list.
pub const CONFIGS_BY_ORG: ControlStmt = ControlStmt::new(
    "ai.configs_by_org",
    "SELECT org, enabled, allowance_micros, api_key_enc != '' AS own_key, (api_key_enc != '' OR base_url != '') AS own_provider FROM org_ai WHERE org IN $orgs",
);

/// The organisation's own provider. Creates the row on first use (the fields not named keep their defaults).
pub const CONFIG_SET_PROVIDER: ControlStmt = ControlStmt::new(
    "ai.config_set_provider",
    "UPSERT $id SET org = $org, base_url = $base_url, api_key_enc = $api_key_enc, chat_model = $chat_model, \
     embedding_model = $embedding_model, updated_at = time::now(), updated_by = $by RETURN NONE",
);

pub const CONFIG_CLEAR_PROVIDER: ControlStmt = ControlStmt::new(
    "ai.config_clear_provider",
    "UPSERT $id SET org = $org, base_url = '', api_key_enc = '', chat_model = '', embedding_model = '', updated_at = time::now(), updated_by = $by RETURN NONE",
);

pub const CONFIG_SET_ENABLED: ControlStmt = ControlStmt::new(
    "ai.config_set_enabled",
    "UPSERT $id SET org = $org, enabled = $enabled, updated_at = time::now(), updated_by = $by RETURN NONE",
);

/// The monthly allowance, in micro-dollars; NONE removes it (unlimited).
pub const CONFIG_SET_ALLOWANCE: ControlStmt = ControlStmt::new(
    "ai.config_set_allowance",
    "UPSERT $id SET org = $org, allowance_micros = $allowance, hold_until = NONE, updated_at = time::now() RETURN NONE",
);

/// A worker paused a job for lack of allowance: the claim leaves the organisation's model jobs alone for `$secs`.
pub const HOLD_SET: ControlStmt = ControlStmt::new(
    "ai.hold_set",
    "UPSERT $id SET org = $org, hold_until = time::now() + <duration>$hold RETURN NONE",
);

/// The allowance was raised, a key was added or AI was switched back on: the organisation's jobs may be claimed again at once.
pub const HOLD_CLEAR: ControlStmt = ControlStmt::new("ai.hold_clear", "UPDATE org_ai SET hold_until = NONE WHERE org = $org AND hold_until != NONE RETURN NONE");

/// Adds to one counter row (its id is derived from organisation, day, purpose, model and source).
pub const USAGE_ADD: ControlStmt = ControlStmt::new(
    "ai.usage_add",
    "UPSERT $id SET org = $org, day = $day, month = $month, purpose = $purpose, model = $model, source = $source, \
     calls = (calls ?? 0) + $calls, input_tokens = (input_tokens ?? 0) + $input, output_tokens = (output_tokens ?? 0) + $output, \
     estimated_calls = (estimated_calls ?? 0) + $estimated, cost_micros = (cost_micros ?? 0) + $cost, updated_at = time::now() RETURN NONE",
);

/// What the server's key cost one organisation in a calendar month.
pub const SPEND_MONTH: ControlStmt = ControlStmt::new(
    "ai.spend_month",
    "SELECT math::sum(cost_micros) AS n FROM ai_usage WHERE org = $org AND month = $month AND source = 'server' GROUP ALL",
);

/// The same for every organisation: the operator's total and who has used up what.
pub const SPEND_MONTH_BY_ORG: ControlStmt = ControlStmt::new(
    "ai.spend_month_by_org",
    "SELECT org, math::sum(cost_micros) AS n FROM ai_usage WHERE month = $month AND source = 'server' GROUP BY org",
);

/// The same for a page of organisations.
pub const SPEND_MONTH_FOR_ORGS: ControlStmt = ControlStmt::new(
    "ai.spend_month_for_orgs",
    "SELECT org, math::sum(cost_micros) AS n FROM ai_usage WHERE org IN $orgs AND month = $month AND source = 'server' GROUP BY org",
);

/// Every organisation that has an allowance.
pub const ALLOWANCES: ControlStmt = ControlStmt::new("ai.allowances", "SELECT org, allowance_micros FROM org_ai WHERE allowance_micros != NONE");

/// One organisation's counters for a month, by purpose, model and whose key paid.
pub const USAGE_MONTH: ControlStmt = ControlStmt::new(
    "ai.usage_month",
    "SELECT purpose, model, source, math::sum(calls) AS calls, math::sum(input_tokens) AS input_tokens, math::sum(output_tokens) AS output_tokens, \
     math::sum(estimated_calls) AS estimated_calls, math::sum(cost_micros) AS cost_micros FROM ai_usage WHERE org = $org AND month = $month \
     GROUP BY purpose, model, source",
);

/// One organisation's calls this month by whose key paid.
pub const USAGE_MONTH_CALLS: ControlStmt = ControlStmt::new(
    "ai.usage_month_calls",
    "SELECT source, math::sum(calls) AS calls FROM ai_usage WHERE org = $org AND month = $month GROUP BY source",
);

/// Organisations whose model jobs the claim must leave alone right now (a worker paused one of their jobs).
pub const HELD_ORGS: ControlStmt = ControlStmt::new("ai.held_orgs", "SELECT VALUE org FROM org_ai WHERE hold_until > time::now()");

pub const PRICES_ALL: ControlStmt = ControlStmt::new("ai.prices_all", "SELECT model, input_per_m, output_per_m FROM ai_price");

pub const PRICE_SET: ControlStmt = ControlStmt::new(
    "ai.price_set",
    "UPSERT $id SET model = $model, input_per_m = $input, output_per_m = $output, updated_at = time::now() RETURN NONE",
);

pub const PRICE_DELETE: ControlStmt = ControlStmt::new("ai.price_delete", "DELETE ai_price WHERE model = $model RETURN NONE");

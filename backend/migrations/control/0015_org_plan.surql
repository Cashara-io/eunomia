-- Hosted plans and the operator API (`plan.rs`, `operator.rs`, docs/deployment.md "Running a hosted service").
-- Every field is new and optional: an organisation with none of them set is unmanaged and unlimited, which is every
-- organisation on a self-hosted install, and behaves exactly as before.

-- the plan: `trial`, `team` or `enterprise` (a name the billing service chose; this server only stores it)
DEFINE FIELD OVERWRITE plan ON org TYPE option<string> ASSERT $value = NONE OR $value IN ["trial", "team", "enterprise"];
-- how many seats the organisation has: active members plus pending invitations (suspended members do not count)
DEFINE FIELD OVERWRITE seat_limit ON org TYPE option<int> ASSERT $value = NONE OR $value >= 1;
-- `active` (or unset) or `suspended`: a suspended organisation's members cannot read or write its content
DEFINE FIELD OVERWRITE status ON org TYPE option<string> ASSERT $value = NONE OR $value IN ["active", "suspended"];
DEFINE FIELD OVERWRITE suspended_at ON org TYPE option<datetime>;
DEFINE FIELD OVERWRITE suspended_reason ON org TYPE option<string>;
-- shown to the owner only: this server never suspends on its own, the billing service decides
DEFINE FIELD OVERWRITE trial_ends_at ON org TYPE option<datetime>;
-- opaque, set by the billing service; the organisation's id is derived from it, so a retried create is one organisation
DEFINE FIELD OVERWRITE external_ref ON org TYPE option<string>;
-- written by every seat-consuming change of a limited organisation, so two of them racing for the last seat
-- meet on one row and one commits first (the loser retries and sees the seat taken)
DEFINE FIELD OVERWRITE seat_guard ON org TYPE option<int>;
DEFINE INDEX OVERWRITE org_status_idx ON org FIELDS status;
DEFINE INDEX OVERWRITE org_external_ref_idx ON org FIELDS external_ref;

-- The owner claim: an organisation invitation of kind `owner`, made by the operator API (no inviting account).
DEFINE FIELD OVERWRITE role ON org_invitation TYPE string ASSERT $value IN ["owner", "admin", "member"];
DEFINE FIELD OVERWRITE invited_by ON org_invitation TYPE option<record<user>>;

-- The operator API acts on the instance ledger as `operator`.
DEFINE FIELD OVERWRITE actor_kind ON audit_event TYPE string ASSERT $value IN ["user", "token", "oauth", "anonymous", "system", "operator"];

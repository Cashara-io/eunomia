-- Organisation self-administration (`org_admin.rs`, docs/deployment.md "Who may administer what").
-- An organisation's owner or admin sets a per-organisation "require two-step verification" policy. A member
-- must use MFA when the instance switch (`instance_setting:security`) or their organisation's policy requires
-- it (`mfa::required_for`). All three fields are new and optional: an organisation with none of them set
-- requires nothing of its own, exactly as before. The organisation's display name is the existing `org.name`.
DEFINE FIELD OVERWRITE require_mfa ON org TYPE option<bool>;
DEFINE FIELD OVERWRITE require_mfa_at ON org TYPE option<datetime>;
-- who last changed it (an account of that organisation)
DEFINE FIELD OVERWRITE require_mfa_by ON org TYPE option<record<user>>;

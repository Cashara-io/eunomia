-- Keep organisation-scoped secret resolution pages and counts off the full audit ledger.
DEFINE INDEX OVERWRITE audit_event_secret_idx ON audit_event FIELDS action, target, created_at;

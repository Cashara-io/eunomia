# Release notes

## Unreleased

### What changed for scripts

New name-only API token requests now receive 90 days, Personal-only access and
read/write memory scopes. Renew these tokens before day 90. For broader access,
explicitly send `vault_id: null` and the scopes needed. To opt in to unlimited
lifetime, explicitly send `expires_at: null`. Empty expiry or space strings are
invalid. Extension account APIs require explicit all-space access, and extension
management also needs `connectors`.

New peer share-code requests without `role` now grant read-only access. Send
`role: "edit"` explicitly for collaboration. Existing tokens, share codes and
peer links keep their stored permissions and lifetime. There is no migration,
backfill, revocation or narrowing. Space deletion keeps its existing browser
name-confirmation guard and API contract.

Hosts now record expired, used and malformed peer-join reasons in their admin
audit log. Public refusals stay generic and the peer protocol remains compatible.

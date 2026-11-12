# Release notes

## 2.3.0

### Ontology, chat diagrams and Composio

The Ontology workspace now includes a landscape overview, object directory and
profiles, a connections canvas, owner review and saved activity with receipts.
Chat can display server-resolved ontology diagrams alongside its checked numbers
and charts. The shared type scale applies across these screens.

Composio lets each person sign in, select supported app accounts and sync their
content into the chosen space. Sync progress reports partial results and rate
limits. Disconnect revokes and deletes the saved tokens. Projects, shared spaces,
the change log and CLI tools remain part of the same installation.

This release also improves large CSV imports, preserves ontology identity during
merges, fixes stale memory rebuilds and adds private diagnostic downloads in
Settings. It includes the Next.js security updates already on main.

Update from Settings, Updates. Tenant migrations 0040 to 0042 run at start-up.
There are no required new environment settings. Back up before updating, and use
the pre-update backup if rolling back to 2.2.0.

The chat turn ordering and number scale fixes in open PRs #238 and #236 are not
included in this release.


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

OAuth connections deliberately keep access to every space the signed-in user
belongs to, with the scopes approved at interactive consent. This supports
cross-space recall in ChatGPT and Claude. Consent now states this plainly,
including Personal and team spaces. OAuth access tokens remain 15 minutes and
rotating refresh tokens remain 30 days. These differ from the new Personal-only,
90-day personal access token defaults. Space selection at consent is tracked in
[issue 217](https://github.com/Cashara-io/eunomia/issues/217).

Agent skill installation and the Claude SessionStart hook now read the existing
MCP skill resource with `memory:read`; they no longer need account settings
access. Upgrade fixtures explicitly request the space administration they use.
Peer refusals use the same owner lookup and audit write for unknown, used and
expired codes, with identical public status, body and headers.

Vault-bound tokens now use their bound space for unqualified record reads,
including `list`, `search`, `get` and `links`. The former blanket record-tool
refusal blocked the new default Personal tokens even for their own documents and
synced records. The existing vault filters now enforce those reads. Required
vault arguments also default to the token's binding, and MCP advertises that
argument as optional for bound credentials. Explicit space arguments, scopes,
membership checks and administrative refusals remain enforced.

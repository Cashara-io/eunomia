-- Organisation invitations (`invitations.rs`, `routers/org.rs`, docs/architecture/tenancy.md).
-- An org owner or admin invites an email with a role; the invitee accepts with a single-use token that
-- expires. Only the token's sha256 is stored, never the token.

-- An org admin: manages the org's invitations. Not an instance admin (that is `authz::is_admin`).
DEFINE FIELD OVERWRITE role ON membership TYPE string DEFAULT "member" ASSERT $value IN ["owner", "admin", "member"];

DEFINE TABLE OVERWRITE org_invitation SCHEMAFULL;
DEFINE FIELD OVERWRITE org ON org_invitation TYPE record<org>;
-- normalized (trimmed, lowercased) like `user.email_lc`
DEFINE FIELD OVERWRITE email ON org_invitation TYPE string;
DEFINE FIELD OVERWRITE role ON org_invitation TYPE string ASSERT $value IN ["admin", "member"];
DEFINE FIELD OVERWRITE token_hash ON org_invitation TYPE string;
DEFINE FIELD OVERWRITE invited_by ON org_invitation TYPE record<user>;
DEFINE FIELD OVERWRITE created_at ON org_invitation TYPE datetime DEFAULT time::now();
-- when the current token was made (a resend makes a new one and moves the expiry)
DEFINE FIELD OVERWRITE sent_at ON org_invitation TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE expires_at ON org_invitation TYPE datetime;
DEFINE FIELD OVERWRITE accepted_at ON org_invitation TYPE option<datetime>;
DEFINE FIELD OVERWRITE accepted_by ON org_invitation TYPE option<record<user>>;
DEFINE FIELD OVERWRITE revoked_at ON org_invitation TYPE option<datetime>;
-- `<org>/<email>` while the invitation is open (expired or not), so an org holds one open invitation per
-- email even when two admins race; `closed:<id>` once it is accepted or revoked.
DEFINE FIELD OVERWRITE slot ON org_invitation TYPE string;
DEFINE INDEX OVERWRITE org_invitation_token_unique ON org_invitation FIELDS token_hash UNIQUE;
DEFINE INDEX OVERWRITE org_invitation_slot_unique ON org_invitation FIELDS slot UNIQUE;
DEFINE INDEX OVERWRITE org_invitation_org_idx ON org_invitation FIELDS org, created_at;
DEFINE INDEX OVERWRITE org_invitation_email_idx ON org_invitation FIELDS email;

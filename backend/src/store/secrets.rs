//! Statements for the credential broker (`secrets`, migration 0025). See the module docs in store/mod.rs.
//!
//! Isolation's second layer (docs/architecture/tenancy.md): every reference statement names the caller's
//! vaults (`vault IN $vaults`), and a provider row's id carries the org (`secret_provider:<org>_<kind>`),
//! read only by those ids, so even two orgs in one database could not see each other's rows.
//!
//! No statement here returns `token_enc` except `PROVIDER_TOKEN`, which only the broker and the
//! test-connection route call; the settings list reads the other fields by name.

use super::Stmt;

pub const ALL: &[&Stmt] = &[
    &REF_LIST,
    &REF_BY_REFERENCE,
    &REF_GET,
    &REF_CREATE,
    &REF_UPDATE,
    &REF_DELETE,
    &PROVIDER_LIST,
    &PROVIDER_TOKEN,
    &PROVIDER_PUT,
    &PROVIDER_PUT_KEEP_TOKEN,
    &PROVIDER_DELETE,
];

pub const REF_LIST: Stmt = Stmt::new("secrets.ref_list", "SELECT * FROM credential_ref WHERE vault IN $vaults ORDER BY created_at, id");

pub const REF_BY_REFERENCE: Stmt =
    Stmt::new("secrets.ref_by_reference", "SELECT * FROM credential_ref WHERE reference = $reference AND vault IN $vaults ORDER BY created_at, id");

pub const REF_GET: Stmt = Stmt::new("secrets.ref_get", "SELECT * FROM credential_ref WHERE id = $id AND vault IN $vaults");

pub const REF_CREATE: Stmt = Stmt::new(
    "secrets.ref_create",
    "CREATE credential_ref SET reference = $reference, provider = $provider, vault = $vault, label = $label, \
     allowed_tools = $allowed_tools, allowed_actors = $allowed_actors, created_by = $user RETURN AFTER",
);

pub const REF_UPDATE: Stmt = Stmt::new(
    "secrets.ref_update",
    "UPDATE credential_ref SET vault = $new_vault, label = $label, allowed_tools = $allowed_tools, allowed_actors = $allowed_actors, \
     updated_at = time::now() WHERE id = $id AND vault IN $vaults RETURN AFTER",
);

pub const REF_DELETE: Stmt = Stmt::new("secrets.ref_delete", "DELETE credential_ref WHERE id = $id AND vault IN $vaults RETURN BEFORE");

/// Everything but the token.
pub const PROVIDER_LIST: Stmt = Stmt::new(
    "secrets.provider_list",
    "SELECT kind, host, created_by, created_at, updated_by, updated_at FROM secret_provider WHERE id IN $ids ORDER BY kind",
);

pub const PROVIDER_TOKEN: Stmt = Stmt::new("secrets.provider_token", "SELECT kind, host, token_enc FROM secret_provider WHERE id = $id");

pub const PROVIDER_PUT: Stmt = Stmt::new(
    "secrets.provider_put",
    "UPSERT $id SET kind = $kind, host = $host, token_enc = $token_enc, created_by = created_by ?? $user, \
     updated_by = $user, updated_at = time::now() RETURN NONE",
);

/// A host change with the stored token kept; touches nothing when no row exists (the route checks).
pub const PROVIDER_PUT_KEEP_TOKEN: Stmt = Stmt::new(
    "secrets.provider_put_keep_token",
    "UPDATE $id SET host = $host, updated_by = $user, updated_at = time::now() RETURN VALUE id",
);

pub const PROVIDER_DELETE: Stmt = Stmt::new("secrets.provider_delete", "DELETE $id RETURN BEFORE");

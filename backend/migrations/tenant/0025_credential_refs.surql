-- The credential broker (docs/architecture/specs/broker.md): references to secrets held in the org's own
-- password manager, and that password manager's settings. Never a secret value: a reference names an
-- item, and the one secret Eunomia holds, the provider's bootstrap token, is stored only as enc:v1:
-- ciphertext under ENCRYPTION_KEY (connectors/crypto.rs), which is not in the database.

-- A reference an agent may use as {"$secret": "<reference>"} in a tool call, bound to one vault.
DEFINE TABLE OVERWRITE credential_ref SCHEMAFULL;
-- op://<vault>/<item>/[<section>/]<field> or bws://<secret-id>, as parsed (secrets::reference)
DEFINE FIELD OVERWRITE reference ON credential_ref TYPE string ASSERT string::len($value) > 0 AND string::len($value) <= 1024;
DEFINE FIELD OVERWRITE provider ON credential_ref TYPE string ASSERT $value IN ["onepassword_connect", "onepassword_service_account", "bitwarden_sm"];
DEFINE FIELD OVERWRITE vault ON credential_ref TYPE record<vault>;
DEFINE FIELD OVERWRITE label ON credential_ref TYPE string DEFAULT "";
-- tool names that may resolve it ("*": any tool that takes secrets); empty: none
DEFINE FIELD OVERWRITE allowed_tools ON credential_ref TYPE array<string> DEFAULT [];
-- actor ids (user:..., api_token:..., oauth_client:...); empty: any member of the vault
DEFINE FIELD OVERWRITE allowed_actors ON credential_ref TYPE array<string> DEFAULT [];
DEFINE FIELD OVERWRITE created_by ON credential_ref TYPE record<user>;
DEFINE FIELD OVERWRITE created_at ON credential_ref TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON credential_ref TYPE datetime DEFAULT time::now();
DEFINE INDEX OVERWRITE credential_ref_vault_reference_idx ON credential_ref FIELDS vault, reference UNIQUE;
DEFINE INDEX OVERWRITE credential_ref_reference_idx ON credential_ref FIELDS reference;

-- One row per provider kind (`secret_provider:<kind>`): where it is and its bootstrap token, encrypted.
DEFINE TABLE OVERWRITE secret_provider SCHEMAFULL;
DEFINE FIELD OVERWRITE kind ON secret_provider TYPE string ASSERT $value IN ["onepassword_connect", "onepassword_service_account", "bitwarden_sm"];
-- Connect: the server URL (required); Bitwarden: the server URL (optional, the US cloud when NONE)
DEFINE FIELD OVERWRITE host ON secret_provider TYPE option<string>;
-- AES-256-GCM ciphertext with the enc:v1: marker; a plaintext token is refused by the database too
DEFINE FIELD OVERWRITE token_enc ON secret_provider TYPE string ASSERT string::starts_with($value, "enc:v1:");
DEFINE FIELD OVERWRITE created_by ON secret_provider TYPE record<user>;
DEFINE FIELD OVERWRITE created_at ON secret_provider TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_by ON secret_provider TYPE record<user>;
DEFINE FIELD OVERWRITE updated_at ON secret_provider TYPE datetime DEFAULT time::now();

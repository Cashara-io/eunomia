-- Sign-in through an external OpenID Connect provider.
-- An account made by the provider has no password: `password_hash` becomes optional (NONE = cannot log in
-- with a password, which `models_user::authenticate` treats like an unknown email).
DEFINE FIELD OVERWRITE password_hash ON user TYPE option<string>;

-- One row per (provider, subject) that may sign in as a user. The subject is the provider's stable id for the
-- person; the email is only what the provider said at the last login. A user can have several (one per provider).
DEFINE TABLE OVERWRITE identity SCHEMAFULL;
DEFINE FIELD OVERWRITE user ON identity TYPE record<user>;
DEFINE FIELD OVERWRITE issuer ON identity TYPE string;
DEFINE FIELD OVERWRITE subject ON identity TYPE string;
DEFINE FIELD OVERWRITE email ON identity TYPE option<string>;
DEFINE FIELD OVERWRITE created_at ON identity TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE last_login_at ON identity TYPE option<datetime>;
DEFINE INDEX OVERWRITE identity_subject_unique ON identity FIELDS issuer, subject UNIQUE;
DEFINE INDEX OVERWRITE identity_user_idx ON identity FIELDS user;

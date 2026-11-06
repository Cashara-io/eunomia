Test-only RSA (2048) and EC (P-256) key pairs, generated with `openssl genpkey` for the `jwt_rs256` and
`jwt_es256` transform tests (`src/secrets/transform.rs`). They sign nothing anywhere else and protect
nothing; like `tests/common/oidc_keys.rs`, they are fixtures, not secrets.

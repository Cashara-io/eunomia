# Migrations

Two sets, one runner (`src/migrate.rs`): `control/NNNN_name.surql` run once at boot in the `control` database, and `tenant/NNNN_name.surql` run in each `org_<uuid>` database when it is provisioned and, after an upgrade, by a `migrate_tenant` job per org (see `docs/architecture/tenancy.md`). Each file and its `_migration` ledger row commit in one transaction. A file's sha256 is stored; editing an applied file stops the boot with a checksum error, so never edit one. Add a new file instead.

Rules for new migrations:

- Use `DEFINE ... OVERWRITE`, not `IF NOT EXISTS`, so a changed definition really applies. (0001 and 0002 keep `IF NOT EXISTS` because they must be safe over installs that predate the ledger.)
- Destructive changes go expand then contract: add the new shape and keep writing both in one release, drop the old shape in a later migration.
- Data backfills that can be slow become jobs later, not migration steps. Data fixes that must precede a constraint (see 0002) are a Rust pre-step in `migrate.rs`, keyed by version.
- Take the number from `QUEUE.md` (the lead hands them out in merge order); never pick the next free one yourself.
- Register the file in the `MIGRATIONS` list (tenant) or `CONTROL_MIGRATIONS` list (control) in `src/migrate.rs`. A new table also goes in `store::TENANT_TABLES` or `store::CONTROL_TABLES`.
- Org databases are created `STRICT`: a migration must define every table, field and index it relies on.
- Code supports tenant schema N and N-1: do not ship a migration whose absence breaks the previous release (expand then contract), because orgs are migrated one at a time after the code is deployed.

## SurrealDB 3.x

- The files are 3.x syntax. 0001 and 0005 were rewritten from 2.x syntax before release of the 3.x build; the sha256 the 2.x files recorded are accepted in `LEGACY_CHECKSUMS` (`src/migrate.rs`) because an install upgraded with `surreal v2 export --v3` carries its old ledger. Do not add to that list for new files.
- 0001 has no vector or full-text index. `0008_v3_indexes.surql` removes whatever the export converter produced and defines `HNSW DIMENSION 1536 DIST COSINE TYPE F32` and one `FULLTEXT` index per text field (title, body_text, memory text), all `CONCURRENTLY`, plus an owner index for the exact-scan fallback. Org databases are `STRICT` (provisioning runs `DEFINE DATABASE ... STRICT`); the old single self-host database was not, and is only read by the one-time move.
- `tenant/0009_drop_control_tables.surql` removes the tables that moved to `control` (accounts, credentials, OAuth, jobs, capsules). Never apply it to the legacy single database: the move reads those tables (`LEGACY_TENANT_VERSION` is 8).
- `tests/fixtures/v2_export_converted.surql` is the upgrade fixture; `tests/migrations.rs` documents how it is regenerated.

## Entity identity (0010)

`0010_entity_name_key.surql` adds `name_key` and `alias_keys` (lowercase copies the database computes on every write, so code at schema N-1 keeps them right too) to the six entity tables, with a `UNIQUE (vault, name_key)` index and an `alias_keys` index. Before it, a Rust pre-step in `migrate.rs` (shared with 0002) folds entities in one vault whose names differ only by case into the oldest: memories move over, two observations become one stale one, edges follow, the loser's name and aliases become aliases of the winner. The pre-step then defines the two fields and backfills existing rows (`UPDATE ... SET name = name`) outside the migration's transaction: SurrealDB 3.x builds an index inside a transaction without seeing writes made earlier in that transaction, so a backfill in the same file would leave the UNIQUE index enforcing nothing for existing rows. A duplicate created between the pre-step and the index definition fails the definition; the runner retries and the next boot folds it. Until an org is migrated, code at the previous schema finds no `name_key`, so a concurrent differently-cased write can make a duplicate that the migration then folds.

## Vector index rebuild (0022)

`0022_rebuild_vector_indexes.surql` rebuilds the three HNSW indexes (`memory_embedding_idx`, `skill_embedding_idx`, `cache_record_embedding_idx`) once on SurrealDB 3.3.5, because 3.3.4 and 3.3.5 fixed HNSW bugs where a delete could leave a removed point in the graph and break kNN for good. It changes no data. It is `DEFINE INDEX OVERWRITE` with the unchanged definitions, which on 3.3.5 retires the old index and builds a new one under a fresh id, in the background (`CONCURRENTLY`), so the migration commits at once; until the build finishes recall, `skill_find` and record search fall back to their exact scan. `REBUILD INDEX` was not used: on 3.3.5 it fails with "index is currently building" while a `CONCURRENTLY` build runs, which is the case in a fresh org. A redefinition right after another rebuild can meet a commit conflict while the retired index is reclaimed; the runner retries it. Tests: `rebuild_vector_indexes_in_0022_keeps_rows_and_knn_finds_the_live_ones`, `a_fresh_org_runs_0022_right_after_the_index_definitions` (`tests/migrations.rs`).

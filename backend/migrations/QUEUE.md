# Migration queue

Owned by the lead. One line per reserved migration number (docs/architecture/next-release-plan.md, section 3.2).

- **Numbers are handed out in merge order**, not in the order work starts. A lane asks the lead for a number; the lead adds the line here.
- **The lock:** only the lane whose line is `locked` may have an unmerged migration file in an open PR that is up for merge. Other lanes may draft against their `reserved` number locally, but rebase and renumber if the queue moves.
- The lock passes when the locked PR merges (state `merged`) or is parked (line removed). Only then does the next lane rebase on `main`, take the lock, regenerate the schema snapshot and rerun `update-path`.
- States: `reserved`, `locked`, `merged`. Applied files are immutable, so a number never changes once its file is on `main`.
- `scripts/ci/check-migrations.sh` (the `scripts` CI job) checks that the numbers in `MIGRATIONS` and `CONTROL_MIGRATIONS` are contiguous and unique, that every file is registered under its own number and name, and that a file whose number is listed here is `locked` or `merged` under the same name.

Control set: 0007 indexes secret audit queries; 0008 adds admin and suspension to user records (PR #162). Next free: 0009.

| set | number | name | lane | PR | state |
|---|---|---|---|---|---|
| tenant | 0022 | rebuild_vector_indexes | L1 | #119 | merged |
| tenant | 0023 | changes | L2 Projects | #114 | merged |
| tenant | 0024 | redact_backfill | L3 Broker | #115 | merged |
| tenant | 0025 | connector_kind_ed | Sources | #129 (via #128) | merged |
| tenant | 0026 | sync_check_connectivity | Sources | #129 (via #128) | merged |
| tenant | 0027 | owner_name | Sources | #128 | merged |
| tenant | 0028 | peers | Peer vaults | 2.0.8 | merged |
| tenant | 0029 | pocket_audio | Sources (#117) | 2.0.8 | merged |
| tenant | 0030 | perf_indexes | Performance | 2.0.8 | merged |
| tenant | 0031 | credential_refs | L3 Broker | broker core (#122), 2.0.9 | merged |
| tenant | 0032 | secret_requests | L3 Broker | B4 (#122), 2.0.9 | merged |
| tenant | 0033 | ontology | Project ontology | feat/project-ontology | locked |
| tenant | 0034 | ontology_properties | Project ontology properties | feat/ontology-properties | reserved |
| control | 0007 | secret_audit_index | L3 Broker | #123 (#122), 2.0.9 | merged |
| control | 0008 | user_admin_suspend | Admin (#144) | #162, 2.0.9 | merged |

# Migration queue

Owned by the lead. One line per reserved migration number (docs/architecture/next-release-plan.md, section 3.2).

- **Numbers are handed out in merge order**, not in the order work starts. A lane asks the lead for a number; the lead adds the line here.
- **The lock:** only the lane whose line is `locked` may have an unmerged migration file in an open PR that is up for merge. Other lanes may draft against their `reserved` number locally, but rebase and renumber if the queue moves.
- The lock passes when the locked PR merges (state `merged`) or is parked (line removed). Only then does the next lane rebase on `main`, take the lock, regenerate the schema snapshot and rerun `update-path`.
- States: `reserved`, `locked`, `merged`. Applied files are immutable, so a number never changes once its file is on `main`.
- `scripts/ci/check-migrations.sh` (the `scripts` CI job) checks that the numbers in `MIGRATIONS` and `CONTROL_MIGRATIONS` are contiguous and unique, that every file is registered under its own number and name, and that a file whose number is listed here is `locked` or `merged` under the same name.

Control set: no reservations (control stays at 0006).

| set | number | name | lane | PR | state |
|---|---|---|---|---|---|
| tenant | 0022 | rebuild_vector_indexes | L1 | #119 | locked |
| tenant | 0023 | changes | L2 Projects | #114 (carried in the Phase 2 Projects PR) | locked |
| tenant | 0024 | redact_backfill | L3 Broker | #115 (carried in the Phase 2 broker stack) | locked |
| tenant | 0025 | credential_refs | L3 Broker | broker core | locked |

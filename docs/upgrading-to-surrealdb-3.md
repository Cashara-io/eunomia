# Upgrading a self-hosted install to SurrealDB 3

**Any 1.x install can update straight to 2.0.** It does not matter which release you are on (v1.2.x, v1.3.x, v1.4.x) or whether you took the 1.4.2 bridge release first. 2.0 moves your data at `docker compose up` time: its compose file has a one-shot `surreal-upgrade` service that runs before SurrealDB 3.3 starts. An old release's "Update now" only checks out the new tag and runs `docker compose up -d`, so that is enough. The same happens when you run `docker compose up -d` by hand, or re-run the installer. The 1.4.2 bridge release is still the smoothest route (its updater does the move before restarting anything, and puts the old release back if the move fails), but it is no longer required.

Where the move runs, in order of preference, all the same script (`scripts/upgrade-surreal-v3.sh`):

- the updater of 1.4.2 or later, before it pulls or restarts anything (the "What happens" steps below);
- the installer, when you re-run it over a 1.x install;
- the `surreal-upgrade` service, at any `docker compose up` (an older updater, or by hand).

Whichever runs first records `SURREAL_DATA_VOLUME=eunomia-surreal-data-v3` in `.env`; after that the others do nothing. 2.0 keeps its data on `eunomia-surreal-data-v3`, and SurrealDB 3.3 never opens the old `eunomia-surreal-data` volume.

Recovering by hand, if a move failed and you fixed the cause: `docker compose up -d` from the repo root (the service tries again), or `bash scripts/upgrade-surreal-v3.sh` then `docker compose up -d`. Set `EUNOMIA_SURREAL_OLD_IMAGE` if your old install did not run `surrealdb/surrealdb:v2.3`. Releasers: see [releasing.md](releasing.md).

Cupel moves from SurrealDB 2.x to SurrealDB 3.3. Released installs run `surrealdb/surrealdb:v2.3` (v1.2.2 and v1.3.0), so that is the real starting point. SurrealDB 3 cannot read 2.x data files, and it refuses to open a database that a newer version has written to. So the data is copied across, never upgraded in place. The script `scripts/upgrade-surreal-v3.sh` does the copy and checks it. You normally do not run it yourself: "Update now" in Settings, the installer or the `surreal-upgrade` service runs it for you.

## What happens

1. The updater checks out the new release. If its `docker-compose.yml` pins a `surrealdb/surrealdb:v3.x` image and the running database is 2.x, it runs the upgrade script before it pulls or restarts anything else.
2. The backend stops. From here nothing writes to the database. The app is down until the script finishes.
3. An encrypted backup, named `pre-v3-<time>.surql.enc` in the `eunomia-backups` volume. Nothing has to exist beforehand: no released install has a `backup` service. The script copies the `surreal` binary out of the RUNNING server's image into a throwaway volume, pulls (or builds) the new release's backup image, and runs `docker compose run --rm --no-deps backup now pre-v3` with `SURREAL_BIN` pointing at that binary. So a 2.3 server is exported by the 2.3 CLI, never by the 3.x CLI inside the backup image. Nightly rotation never deletes it. Make sure `BACKUP_ENCRYPTION_KEY` is set in `.env` (the updater adds one if missing).
4. The old server stops and its volume is copied to a scratch volume. A 2.7 server (`surrealdb/surrealdb:v2.7.0`) is started on the COPY and exported with `surreal export --v3`. The old volume is never opened by a newer version, because [SurrealDB's upgrade docs](https://surrealdb.com/docs/manage/self-hosted/upgrades-and-patching) say some minor upgrades "can only be reverted" from an export, "including 2.6 to 2.7", so an in-place 2.3 to 2.7 bump would break rollback. The unencrypted export is held in a temporary directory and shredded when the script ends; the scratch container and volume are removed when it ends.
5. The export is adjusted for two index kinds 3.x removed: the `MTREE` embedding index becomes `HNSW` (same name, same dimension, cosine, F32), and the two-field full-text index on `cache_record (title, body_text)` becomes one index per field (`cache_record_fts_idx_title`, `cache_record_fts_idx_body_text`).
6. The 2.7 scratch server stops. SurrealDB 3.3 starts on a NEW volume, `eunomia-surreal-data-v3`, and the export is imported. For this step only the server runs WITHOUT the 60 second query and transaction limits that the normal `docker-compose.yml` sets (the script layers `docker-compose.import.yml` over it, which differs only by dropping those two flags). The export is one `INSERT` per table and the HNSW index is defined before the data, so a table with tens of thousands of 1536-dimension embeddings cannot load inside 60 seconds. HNSW indexes are built during the import. The export is not reordered to build indexes after the data: that is not verified against a real 3.3.1 server, and without the time limit it is not needed for correctness.
7. When the import finishes the 3.x server is recreated from the plain, hardened `docker-compose.yml` (timeouts back on) on the same volume. The script then compares the record count of every table, and the `_migration` ledger rows, between 2.x and 3.x, through that hardened server. If they match it writes `SURREAL_DATA_VOLUME=eunomia-surreal-data-v3` into `.env` (so every later `docker compose up` keeps using the new volume) and the updater carries on with the normal pull and restart.
8. If anything fails or does not match, the new volume is deleted, the old 2.x image starts again on the old volume, the backend restarts, the checkout is put back on the previous release, and Settings shows the error. Nothing was changed. You can click "Update now" again after fixing the cause. The script log is `update-status/upgrade.log`.

### When an older updater (1.4.1 or earlier) takes 2.0 directly

Its `docker compose up -d` first replaces the old containers (so the old SurrealDB is stopped cleanly and nothing writes), then starts `surreal-upgrade`, which runs the same script in a container (it holds the docker socket, like the updater). The script finds the stopped 2.x data volume and does steps 3 to 7 above: the encrypted `pre-v3` backup from a temporary 2.3 server on a COPY, the 2.7 export from another copy, the import on `eunomia-surreal-data-v3` (on one-off containers of the `surrealdb` service, with the same flags) and the count check. Then SurrealDB 3.3 and the new backend start, and the backend moves the data into its org database on first boot. Watch it with `docker compose logs -f surreal-upgrade`; the log is also appended to `update-status/upgrade.log`. Expect the app to be offline for the move plus the backend's first boot.

- If `ENCRYPTION_KEY` is empty (v1.2 updaters never set one), the service first writes a key and `ENCRYPTION_KEY_LEGACY_EMPTY=1` to `.env`, stops this `up` and starts the stack again by itself 20 seconds later, because the backend of the first `up` was configured before the key existed. Settings may show "restart failed" until the updater next reports (it clears itself).
- If the move fails, nothing is changed on the old volume and the partial `-v3` data is emptied, but the old release does not come back by itself (the old containers were already replaced): the app stays down, with the reason in `docker compose logs surreal-upgrade`. Fix the cause and run `docker compose up -d` (it tries again), or go back to your old release as in "Rolling back" step 2, with your old tag.
- If `eunomia-surreal-data-v3` already holds data but `.env` does not say which volume is in use, it refuses to touch either and says how to choose.

Running it by hand (same result): `bash scripts/upgrade-surreal-v3.sh` from the repo root, on a checkout whose compose file already pins 3.x. It exits 0 immediately if the stack already runs 3.x, so it is safe to re-run.

## How long it takes and how much disk it needs

Time grows with data size: a small install (thousands of records) should take a minute or two. The slow part is loading the embeddings and building the HNSW index during import, and that is NOT measured yet on a large install: expect tens of thousands of embeddings to take minutes, and hundreds of thousands to take much longer (possibly an hour or more). The import is not cut off by the normal 60 second limits (see step 6). The app is offline for the whole time. `UPGRADE_TEST_LARGE=1 bash scripts/tests/upgrade-surreal-v3.test.sh` loads 50,000 1536-dimension rows first, so you can time it on your own hardware.

Disk: plan for about 4 times the size of the current `eunomia-surreal-data` volume. That is the old volume (kept), the scratch copy (removed at the end), the new volume, and the temporary export (plus the encrypted backup, which is smaller than the export). Check free space with `docker system df` before you start.

## Rolling back

SurrealDB 3.3 refuses downgrades, so rollback means going back to the old volume, which is never modified or deleted:

1. If the script failed, it already did this for you.
2. After a successful upgrade, to go back anyway: check out the previous release tag (`git checkout v1.4.2`, which pins `surrealdb/surrealdb:v2.3`), set `EUNOMIA_IMAGE_TAG` in `.env` to that tag (otherwise the 2.0.0 backend image, built on the SurrealDB 3 SDK, starts against SurrealDB 2.3 and fails), remove the `SURREAL_DATA_VOLUME=` line from `.env`, and run `docker compose up -d`. You lose anything written after the upgrade. To keep it, restore the post-upgrade state from a backup into 3.x instead.
3. If the old volume is already gone, do step 2 (it starts 2.3 on a new, empty volume), then restore `pre-v3-*.surql.enc` into it: `docker compose exec backup eunomia-backup restore pre-v3-<time>.surql.enc --wipe`. On that checkout the `backup` image is the 1.4.2 one, whose CLI is 2.3; this release's `backup` image (3.x CLI) cannot talk to a 2.x server.

## Deleting the old volume

Keep `eunomia-surreal-data` for at least one release after the upgrade (rollback needs it). When you are happy with 3.x and have a recent backup:

```
docker compose stop
docker volume ls | grep surreal-data      # old: <project>_eunomia-surreal-data
docker volume rm <project>_eunomia-surreal-data
docker compose up -d
```

`<project>` is the compose project name (the checkout folder name, `eunomia` by default). Never remove the `-v3` volume: it holds the live data.

## Notes

- The updater hook lives in `scripts/auto-update.sh`, and the running copy of that script is the one from the OLD release (the updater reads it before it checks out the new one), so it only protects installs already on 1.4.2 or later. The `surreal-upgrade` service covers the rest, because an old updater's `docker compose up` runs it. `scripts/ci/check-release-order.sh` refuses a 3.x tag that has neither.
- What is proven and what is not (be honest about this before trusting a real install):
  - Proven by shell tests with mocks (no Docker): the updater only runs the upgrade before pulling, undoes the checkout on failure, and injects `ENCRYPTION_KEY`/`JWT_SECRET` only after it succeeds; the backup script refuses a half-written export and its nightly loop survives a failed run.
  - Proven with real containers (`scripts/tests/upgrade-surreal-v3.test.sh`, run in CI): the whole of `scripts/upgrade-surreal-v3.sh`, including case B (the real v1.2.2 compose file: `surrealdb:v2.3`, no `backup` service), case D (the v1.2.2 stack, then a plain `docker compose up` of this release: the `surreal-upgrade` service moves the data, and a second `up` does nothing) and case E (a fresh install: nothing to move).
  - Proven by end-to-end runs with real backends (v1.4.1 and v1.2.2 installs with seeded data updated straight to 2.0 by their own updaters, and 1.4.1 through the 1.4.2 bridge): a 2.7 server opens a copy of a volume written by 2.3, `export --v3` of that data imports into 3.3 with every table's count equal, a 2.3 `surreal` binary runs inside the Debian backup image, and login and recall work afterwards.
  - Not measured: how long the move takes on a large install (see above).
- Restores into the running 3.x install have the same time limit problem. Use `backend/scripts/restore.sh <file> [--wipe]` (see [deployment](deployment.md)): it stops the backend, runs the restore on a server started with `docker-compose.import.yml`, and starts the hardened server and the backend again. `docker compose exec backup eunomia-backup restore` talks to the hardened server directly, so a big database can hit the 60 second transaction limit there.
- Proven by a mock-docker test (no Docker needed): `scripts/tests/upgrade-surreal-v3-import.test.sh` checks that the import starts with the override, the plain compose file is started after it, and the new volume is recorded last. Whether the unrestricted server really imports a large dump is Docker-only (the `UPGRADE_TEST_LARGE` case).
- Test: `bash scripts/tests/upgrade-surreal-v3.test.sh` runs the whole flow against real containers (project `fw-upgrade`): a 2.7 install, a forced mismatch that must roll back, the real upgrade, case B from the released v1.2.2 compose file, case D (skip the bridge: a plain `docker compose up`) and case E (fresh install). CI runs it on every pull request.

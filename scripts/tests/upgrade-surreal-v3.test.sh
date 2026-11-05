#!/usr/bin/env bash
# End-to-end test of scripts/upgrade-surreal-v3.sh with real SurrealDB containers.
# Throwaway names only: compose project fw-upgrade, host port 8242 (8241-8249 are
# reserved for this slice). Never touches the real `eunomia` project.
#   1. 2.7.0 on a RocksDB volume, legacy schema + data in every table
#   2. a forced count mismatch must roll back to the 2.x volume with data intact
#   3. the real upgrade must land on 3.3 with equal counts, KNN and full-text working
#   D. skip-bridge: a released v1.2.2 stack, then the new compose file and a plain `docker compose up`
#      (all an old release's updater does): the one-shot surreal-upgrade service must move the data
# UPGRADE_TEST_LARGE=1 (or a row count) adds that many 1536-d cache_record rows (default 50000)
# before the upgrade: the import must finish well past the hardened server's 60 s limits. Slow
# (many minutes, a few GB of disk), so it is off by default and not run in CI.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$HERE/../.."
OLD_IMG=surrealdb/surrealdb:v2.7.0 NEW_IMG=surrealdb/surrealdb:v3.3.5
NET=fw-upgrade_default
W="$(mktemp -d)"
export EUNOMIA_DIR="$W" COMPOSE_PROJECT_NAME=fw-upgrade BACKEND_PORT=8242 BACKUP_ENCRYPTION_KEY=test-key-not-secret
PASS=0; FAIL=0
check() { local d="$1" out; shift; if out="$("$@" 2>&1)"; then PASS=$((PASS + 1)); echo "ok   $d"; else FAIL=$((FAIL + 1)); echo "FAIL $d"; [ -z "$out" ] || echo "$out" | sed 's/^/       /'; fi; }
# down -v skips volumes no service references any more (the old data volume after the switch)
cleanup() {
  (cd "$W" && docker compose down -v --remove-orphans >/dev/null 2>&1)
  docker volume ls -q --filter label=com.docker.compose.project=fw-upgrade | xargs docker volume rm >/dev/null 2>&1
  rm -rf "$W"
}
trap cleanup EXIT

sq() { docker run --rm -i --network "$NET" "$1" sql --endpoint http://surrealdb:8000 --user root --pass root \
         --ns eunomia --db eunomia --hide-welcome --json 2>/dev/null; }
last() { grep . | tail -1; }
count() { echo "RETURN (SELECT count() FROM \`$2\` GROUP ALL)[0].count ?? 0;" | sq "$1" | last | tr -d '[]'; }
running_image() { docker inspect -f '{{.Config.Image}}' "$(cd "$W" && docker compose ps -q surrealdb)"; }

# --- the "installed" release: the real compose file pinned to 2.7.0, plus a stand-in backend ---
mkdir -p "$W/backend/scripts/backup"
cp "$ROOT"/backend/scripts/backup/* "$W/backend/scripts/backup/"
sed -i.bak "s#^FROM surrealdb/surrealdb:.*AS surreal#FROM $OLD_IMG AS surreal#" "$W/backend/scripts/backup/Dockerfile"
cp "$ROOT/scripts/upgrade-surreal-v3.sh" "$W/upgrade.sh"
mkdir -p "$W/scripts" && cp "$ROOT/scripts/upgrade-surreal-v3.sh" "$ROOT/scripts/surreal-upgrade-service.sh" "$W/scripts/"
cp "$ROOT/docker-compose.import.yml" "$W/"
# the "installed" release: like a 1.x one, its data is on eunomia-surreal-data and it has no surreal-upgrade step
python3 - "$ROOT/docker-compose.yml" "$OLD_IMG" > "$W/compose.old" <<'PYEOF'
import re, sys
s = open(sys.argv[1]).read()
s = re.sub(r"surrealdb/surrealdb:v[0-9.]+", sys.argv[2], s)
s = s.replace("${SURREAL_DATA_VOLUME:-eunomia-surreal-data-v3}", "eunomia-surreal-data")
s = re.sub(r"\n    depends_on:\n      surreal-upgrade:\n        condition: service_completed_successfully", "", s)
s = re.sub(r"\n  surreal-upgrade:\n(    .*\n|      .*\n)+", "\n", s)
# 2.x refuses --allow-experimental=files (3.x file buckets)
s = re.sub(r"\n      - --allow-experimental=files", "", s)
print(s, end="")
PYEOF
grep -qE '^  surreal-upgrade:|service_completed_successfully' "$W/compose.old" && { echo "compose.old still has surreal-upgrade"; exit 1; }
sed "s#surrealdb/surrealdb:v[0-9.]*#$NEW_IMG#" "$ROOT/docker-compose.yml" > "$W/compose.new"
cat > "$W/standin.yml" <<'YML'
services:
  backend:
    image: busybox:1.36
    entrypoint: ["sleep", "3600"]
    # `up --wait` fails a service with no healthcheck on current Compose: a trivial one
    healthcheck: {test: ["CMD", "true"], interval: 1s, retries: 1}
YML
cp "$W/compose.old" "$W/docker-compose.yml"
printf 'JWT_SECRET=x\nENCRYPTION_KEY=y\nBACKUP_ENCRYPTION_KEY=%s\n' "$BACKUP_ENCRYPTION_KEY" > "$W/.env"
export COMPOSE_FILE="docker-compose.yml:standin.yml"
docker pull -q busybox:1.36 >/dev/null; docker pull -q docker:27-cli >/dev/null
# `--pull never` below: fetch both SurrealDB images up front (a fresh CI runner has neither)
docker pull -q "$OLD_IMG" >/dev/null && docker pull -q "$NEW_IMG" >/dev/null || { echo "could not pull $OLD_IMG / $NEW_IMG"; exit 1; }
cd "$W" || exit 1
docker compose build -q backup >/dev/null || { echo "backup image build failed"; exit 1; }
docker compose up -d --wait --pull never surrealdb backup backend >/dev/null || { echo "stack did not start"; exit 1; }

# --- legacy schema, ledger, and rows in every table ---
{
  cat "$ROOT/backend/tests/legacy_schema.txt"
  echo "DEFINE TABLE IF NOT EXISTS _migration SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS version ON _migration TYPE int;
DEFINE FIELD IF NOT EXISTS name ON _migration TYPE string;
DEFINE FIELD IF NOT EXISTS checksum ON _migration TYPE string;
DEFINE FIELD IF NOT EXISTS applied_at ON _migration TYPE datetime DEFAULT time::now();
DEFINE INDEX IF NOT EXISTS _migration_version_unique ON _migration FIELDS version UNIQUE;
CREATE _migration:1 SET version = 1, name = 'baseline', checksum = 'aaa';
CREATE _migration:2 SET version = 2, name = 'entity_name_unique', checksum = 'bbb';"
  awk 'BEGIN { srand(7)
    for (i = 1; i <= 3; i++) {
      printf "CREATE user:u%d SET email = \"u%d@example.com\", password_hash = \"h\";\n", i, i
      printf "CREATE vault:v%d SET name = \"vault %d\";\n", i, i
      printf "CREATE vault_member:m%d SET vault = vault:v%d, user = user:u%d;\n", i, i, i
      printf "CREATE api_token:t%d SET owner = user:u%d, token_hash = \"th%d\";\n", i, i, i
      printf "CREATE app_settings:s%d SET owner = user:u%d;\n", i, i
      printf "CREATE chat_thread:ct%d SET owner = user:u1;\n", i
    }
    for (i = 1; i <= 2; i++) { printf "CREATE session:se%d SET owner = user:u1, sid = \"sid%d\";\n", i, i
                               printf "CREATE sync_status:ss%d SET owner = user:u%d;\n", i, i }
    split("person 8 organisation 6 location 5 repository 4 file 6 symbol 7", e, " ")
    for (k = 1; k <= 11; k += 2) for (i = 1; i <= e[k + 1]; i++)
      printf "CREATE %s:%s%d SET owner = user:u1, vault = vault:v1, name = \"%s %d\";\n", e[k], substr(e[k], 1, 2), i, e[k], i
    for (i = 1; i <= 30; i++) {
      split("person:pe organisation:or location:lo repository:re file:fi symbol:sy", s, " "); split(s[i % 6 + 1], p, ":")
      printf "CREATE memory:m%d SET owner = user:u1, vault = vault:v1, subject = %s:%s%d, text = \"memory number %d about marmalade\";\n", i, p[1], p[2], (i % 4) + 1, i
    }
    for (i = 1; i <= 10; i++) {
      printf "RELATE person:pe%d->relates_to->organisation:or%d SET label = \"works at %d\";\n", (i % 8) + 1, (i % 6) + 1, i
      printf "RELATE cache_record:r%d->linked_to->cache_record:r%d SET rel = \"next\", origin = \"agent\";\n", i, i + 1
    }
    for (i = 1; i <= 60; i++) {
      printf "CREATE cache_record:r%d SET owner = user:u1, source = \"s\", type = \"t\", external_id = \"e%d\", title = \"%s record %d\", body_text = \"the quick brown fox %d\", content_hash = \"h%d\", ingested_at = time::now(), updated_at = time::now(), embedding = [", i, i, (i <= 5 ? "needle" : "hay"), i, i, i
      for (d = 1; d <= 1536; d++) printf "%s%.5f", (d > 1 ? "," : ""), rand() * 2 - 1
      print "];"
    }
    for (i = 1; i <= 12; i++) printf "CREATE chat_message:cm%d SET owner = user:u1, thread_id = chat_thread:ct%d, role = \"user\", content = \"hello %d\";\n", i, (i % 3) + 1, i
    for (i = 1; i <= 5; i++) printf "CREATE audit_log:a%d SET owner = user:u1, tool_name = \"recall\", outcome = \"ok\";\n", i
    for (i = 1; i <= 4; i++) printf "CREATE embed_cache:ec%d SET text_hmac = \"hm%d\", vector = [0.1, 0.2, %d];\n", i, i, i
  }'
  for k in github slack notion linear; do echo "CREATE connector SET owner = user:u1, kind = \"$k\";"; done
} > "$W/load.surql"
errs="$(sq "$OLD_IMG" < "$W/load.surql" | grep -c '"[A-Za-z ]*\(error\|Incorrect\|failed\)')"
check "legacy data loaded into 2.7 without errors" test "$errs" = 0

if [ -n "${UPGRADE_TEST_LARGE:-}" ]; then
  N="$UPGRADE_TEST_LARGE"; [ "$N" -gt 1 ] 2>/dev/null || N=50000
  echo "=== large case: loading $N cache_record rows of 1536 floats ==="
  # batches of 100 rows per INSERT; ids start at 1000 so they never meet r1..r60
  awk -v n="$N" 'BEGIN { srand(11)
    for (b = 0; b * 100 < n; b++) {
      printf "INSERT INTO cache_record ["
      for (i = 1; i <= 100 && b * 100 + i <= n; i++) {
        k = 1000 + b * 100 + i
        printf "%s{ id: cache_record:r%d, owner: user:u1, source: \"s\", type: \"t\", external_id: \"big%d\", title: \"big %d\", body_text: \"filler %d\", content_hash: \"hb%d\", ingested_at: time::now(), updated_at: time::now(), embedding: [", (i > 1 ? "," : ""), k, k, k, k, k
        for (d = 1; d <= 1536; d++) printf "%s%.4f", (d > 1 ? "," : ""), rand() * 2 - 1
        printf "] }"
      }
      print "];"
    }}' | sq "$OLD_IMG" >/dev/null
  EXPECT_BIG=$((60 + N))
fi
EXPECT="_migration:2 api_token:3 app_settings:3 audit_log:5 cache_record:${EXPECT_BIG:-60} chat_message:12 chat_thread:3 connector:4 embed_cache:4 file:6 linked_to:10 location:5 memory:30 organisation:6 person:8 relates_to:10 repository:4 session:2 symbol:7 sync_status:2 user:3 vault:3 vault_member:3"
counts_ok() { local img="$1" bad=0 p; for p in $EXPECT; do [ "$(count "$img" "${p%%:*}")" = "${p##*:}" ] || { echo "count mismatch on ${p%%:*}: want ${p##*:}, got $(count "$img" "${p%%:*}")"; bad=1; }; done; return $bad; }
check "2.7 holds the expected rows in all $(echo $EXPECT | wc -w | tr -d " ") tables" counts_ok "$OLD_IMG"
check "the installed server is 2.7" test "$(running_image)" = "$OLD_IMG"

# --- "update": the new release's compose file now pins 3.3 ---
cp "$W/compose.new" "$W/docker-compose.yml"

echo "=== run 1: forced count mismatch, must roll back ==="
UPGRADE_TEST_SQL='DELETE person:pe1;' bash "$W/upgrade.sh"; rc=$?
echo "=== run 1 exit code $rc ==="
check "mismatch: script exits non-zero" test "$rc" -ne 0
check "mismatch: old 2.7 image is running again" test "$(running_image)" = "$OLD_IMG"
check "mismatch: old volume data intact (all tables)" counts_ok "$OLD_IMG"
check "mismatch: the new volume was removed" test -z "$(docker volume ls -q --filter name=fw-upgrade_eunomia-surreal-data-v3)"
check "mismatch: .env not switched" bash -c "! grep -q SURREAL_DATA_VOLUME '$W/.env'"
check "mismatch: backend stand-in restarted" test "$(docker inspect -f '{{.State.Running}}' "$(docker compose ps -q backend)")" = true
check "mismatch: encrypted pre-v3 backup was written" bash -c "docker compose exec -T backup eunomia-backup list | grep -q 'pre-v3-.*surql.enc'"

echo "=== run 2: the real upgrade ==="
bash "$W/upgrade.sh"; rc=$?
echo "=== run 2 exit code $rc ==="
check "upgrade: script exits 0" test "$rc" -eq 0
check "upgrade: 3.3 is running" test "$(running_image)" = "$NEW_IMG"
check "upgrade: 3.3 holds the same rows in all tables (incl. _migration)" counts_ok "$NEW_IMG"
check "upgrade: .env points at the v3 volume" grep -q '^SURREAL_DATA_VOLUME=eunomia-surreal-data-v3$' "$W/.env"
check "upgrade: old volume still exists" test -n "$(docker volume ls -q --filter name=fw-upgrade_eunomia-surreal-data$)"
check "upgrade: ledger rows came across" bash -c "echo 'SELECT version FROM _migration ORDER BY version;' | docker run --rm -i --network $NET $NEW_IMG sql --endpoint http://surrealdb:8000 --user root --pass root --ns eunomia --db eunomia --hide-welcome --json 2>/dev/null | grep -q '\"version\":1.*\"version\":2'"

check "upgrade: the 2.x MTREE index arrived as HNSW" bash -c "echo 'INFO FOR TABLE cache_record;' | docker run --rm -i --network $NET $NEW_IMG sql --endpoint http://surrealdb:8000 --user root --pass root --ns eunomia --db eunomia --hide-welcome --json 2>/dev/null | grep -q 'HNSW DIMENSION 1536'"

# the index definitions the backend uses on 3.x (idempotent OVERWRITE), then the real queries
echo 'DEFINE INDEX OVERWRITE cache_record_embedding_idx ON cache_record FIELDS embedding HNSW DIMENSION 1536 DIST COSINE TYPE F32;
DEFINE INDEX OVERWRITE cache_record_title_ft ON cache_record FIELDS title FULLTEXT ANALYZER cache_text_analyzer BM25;
DEFINE INDEX OVERWRITE cache_record_body_ft ON cache_record FIELDS body_text FULLTEXT ANALYZER cache_text_analyzer BM25;' | sq "$NEW_IMG" >/dev/null
knn="$(echo 'LET $v = (SELECT VALUE embedding FROM ONLY cache_record:r7); RETURN array::len(SELECT id FROM cache_record WHERE embedding <|3,40|> $v);' | sq "$NEW_IMG" | last | sed 's/.*,//' | tr -d '[]')"
echo "KNN rows: $knn"
check "3.3 HNSW KNN select returns rows" test "${knn:-0}" -ge 1
ft="$(echo "RETURN array::len(SELECT id FROM cache_record WHERE title @@ 'needle');" | sq "$NEW_IMG" | last | tr -d '[]')"
echo "full-text rows (cache_record.title): $ft"
check "3.3 full-text match on cache_record returns the 5 needle rows" test "${ft:-0}" = 5
mf="$(echo "RETURN array::len(SELECT id FROM memory WHERE text @@ 'marmalade');" | sq "$NEW_IMG" | last | tr -d '[]')"
echo "full-text rows (memory.text): $mf"
check "3.3 full-text match on memory returns all 30 rows" test "${mf:-0}" = 30

echo "=== run 3: idempotent re-run ==="
bash "$W/upgrade.sh"; rc=$?
check "re-run on 3.x exits 0 and changes nothing" test "$rc" -eq 0 -a "$(running_image)" = "$NEW_IMG"

# --- case C: surrealdb DOWN (an old updater pulled 3.x over 2.x data and the server exited). NOT YET RUN: needs Docker. ---
echo "=== case C: recover with surrealdb stopped ==="
docker compose down -v --remove-orphans >/dev/null 2>&1
docker volume ls -q --filter label=com.docker.compose.project=fw-upgrade | xargs docker volume rm >/dev/null 2>&1
printf 'JWT_SECRET=x\nENCRYPTION_KEY=y\nBACKUP_ENCRYPTION_KEY=%s\n' "$BACKUP_ENCRYPTION_KEY" > "$W/.env"
cp "$W/compose.old" "$W/docker-compose.yml"
docker compose up -d --wait --pull never surrealdb backup backend >/dev/null || { echo "case C stack did not start"; FAIL=$((FAIL + 1)); }
printf 'DEFINE TABLE person SCHEMALESS;\nCREATE person:a SET name = "a";\nCREATE person:b SET name = "b";\n' | sq "$OLD_IMG" >/dev/null
docker compose stop surrealdb backend >/dev/null
cp "$W/compose.new" "$W/docker-compose.yml"
EUNOMIA_SURREAL_OLD_IMAGE="$OLD_IMG" bash "$W/upgrade.sh"; rc=$?
echo "=== case C exit code $rc ==="
check "case C: script exits 0 with surrealdb stopped" test "$rc" -eq 0
check "case C: 3.3 runs with the same rows" test "$(running_image)" = "$NEW_IMG" -a "$(count "$NEW_IMG" person)" = 2
check "case C: a pre-v3 backup was written from the old server" bash -c "docker compose run --rm --no-deps -T backup list | grep -q 'pre-v3-.*surql.enc'"
check "case C: the temporary server and copy are gone" test -z "$(docker ps -aq --filter name=fw-upgrade-v2scratch; docker volume ls -q --filter name=v2copy)"

# --- case B: the REAL released install. v1.2.2 ships surrealdb:v2.3 and has NO `backup` service. ---
# Needs Docker and the v1.2.2 tag in the checkout (CI: fetch-depth 0). Starts over from nothing.
echo "=== case B: upgrade from the released v1.2.2 compose file ==="
REL="$(git -C "$ROOT" show v1.2.2:docker-compose.yml 2>/dev/null)"
if [ -z "$REL" ]; then echo "SKIP case B: tag v1.2.2 not available"; else
  docker compose down -v --remove-orphans >/dev/null 2>&1
  docker volume ls -q --filter label=com.docker.compose.project=fw-upgrade | xargs docker volume rm >/dev/null 2>&1
  rm -f "$W/.env.bak"; printf 'JWT_SECRET=x\nENCRYPTION_KEY=y\nBACKUP_ENCRYPTION_KEY=%s\nEUNOMIA_IMAGE_TAG=fw-local\n' "$BACKUP_ENCRYPTION_KEY" > "$W/.env"
  cp "$ROOT/backend/scripts/backup/Dockerfile" "$W/backend/scripts/backup/Dockerfile"   # pristine: 3.x CLI inside
  printf '%s\n' "$REL" > "$W/docker-compose.yml"
  check "case B: the released compose file has no backup service" bash -c "! docker compose config --services | grep -qx backup"
  check "case B: the released compose file pins surrealdb:v2.3" bash -c "docker compose config --images | grep -qx 'surrealdb/surrealdb:v2.3'"
  docker pull -q surrealdb/surrealdb:v2.3 >/dev/null   # `--pull never`: the released pin, not otherwise fetched
  docker compose up -d --wait --pull never surrealdb backend >/dev/null || { echo "case B stack did not start"; FAIL=$((FAIL + 1)); }
  OLD23=surrealdb/surrealdb:v2.3
  printf 'DEFINE TABLE person SCHEMALESS;\nCREATE person:a SET name = "a";\nCREATE person:b SET name = "b";\nCREATE person:c SET name = "c";\n' | sq "$OLD23" >/dev/null
  check "case B: 2.3 holds the rows" test "$(count "$OLD23" person)" = 3
  # the "update": new release's compose file (has a backup service, pins 3.x); the .env tag selects a local build
  cp "$W/compose.new" "$W/docker-compose.yml"
  bash "$W/upgrade.sh"; rc=$?
  echo "=== case B exit code $rc ==="
  check "case B: script exits 0 with no pre-existing backup service" test "$rc" -eq 0
  check "case B: 3.3 is running with the same rows" test "$(running_image)" = "$NEW_IMG" -a "$(count "$NEW_IMG" person)" = 3
  check "case B: an encrypted pre-v3 backup was written" bash -c "docker compose run --rm --no-deps -T backup list | grep -q 'pre-v3-.*surql.enc'"
  check "case B: the backup restores (integrity check passes) with the key" bash -c "f=\$(docker compose run --rm --no-deps -T backup list | grep -o 'pre-v3-[^ ]*surql.enc' | head -1); docker compose run --rm --no-deps -T --entrypoint sh backup -c \"head -n1 /backups/\$f | grep -q '^EUNOMIA-BK2 '\""
  check "case B: scratch container and volumes are gone" test -z "$(docker ps -aq --filter name=fw-upgrade-v2scratch; docker volume ls -q --filter name=v2copy --filter name=oldbin)"
fi

# --- case D: skip-bridge. The released v1.2.2 stack, then what its updater does with a 3.x release:
# check out the new compose file and `docker compose up`. No host-side script runs. ---
echo "=== case D: v1.2.2 stack, then a plain docker compose up of the new release ==="
if [ -z "$REL" ]; then echo "SKIP case D: tag v1.2.2 not available"; else
  docker compose down -v --remove-orphans >/dev/null 2>&1
  docker volume ls -q --filter label=com.docker.compose.project=fw-upgrade | xargs docker volume rm >/dev/null 2>&1
  printf 'JWT_SECRET=x\nENCRYPTION_KEY=y-long-enough-key\nEUNOMIA_IMAGE_TAG=fw-local\n' > "$W/.env"   # v1.2 updaters write no backup key
  printf '%s\n' "$REL" > "$W/docker-compose.yml"
  docker compose up -d --wait --pull never surrealdb backend >/dev/null || { echo "case D stack did not start"; FAIL=$((FAIL + 1)); }
  printf 'DEFINE TABLE person SCHEMALESS;\nCREATE person:a SET name = "a";\nCREATE person:b SET name = "b";\nDEFINE TABLE memory SCHEMALESS;\nCREATE memory:m1 SET text = "kept\\nacross { lines";\nCREATE memory:m2 SET text = "a 2.3 export\\nkeeps raw ] [ newlines";\n' | sq "$OLD23" >/dev/null   # multi-line values: 2.3 exports them raw
  cp "$W/compose.new" "$W/docker-compose.yml"
  docker compose up -d --wait --pull never surrealdb backup backend; rc=$?
  echo "=== case D up exit code $rc ==="
  docker compose logs surreal-upgrade 2>&1 | grep -E 'nothing to do|OK:|ERROR|counts' | sed 's/^/       /'
  check "case D: up succeeds (the one-shot moved the data)" test "$rc" -eq 0
  check "case D: 3.3 runs with the same rows" test "$(running_image)" = "$NEW_IMG" -a "$(count "$NEW_IMG" person)" = 2 -a "$(count "$NEW_IMG" memory)" = 2
  check "case D: .env records the v3 volume" grep -q '^SURREAL_DATA_VOLUME=eunomia-surreal-data-v3$' "$W/.env"
  check "case D: no one-off server left behind" test -z "$(docker ps -aq --filter name=fw-upgrade-v3oneoff)"
  check "case D: 3.3 runs on the v3 volume" bash -c "docker inspect -f '{{range .Mounts}}{{.Name}} {{end}}' \$(docker compose ps -q surrealdb) | grep -q 'fw-upgrade_eunomia-surreal-data-v3'"
  check "case D: an encrypted pre-v3 backup was written" bash -c "docker compose run --rm --no-deps -T backup list | grep -q 'pre-v3-.*surql.enc'"
  check "case D: old volume still holds the 2.x data" bash -c "docker run --rm -v fw-upgrade_eunomia-surreal-data:/d:ro busybox:1.36 ls /d/eunomia.db | grep -q ."
  docker compose up -d --wait --pull never surrealdb backup backend >/dev/null 2>&1; rc=$?
  check "case D: a second up is a no-op" bash -c "[ $rc -eq 0 ] && docker compose logs surreal-upgrade 2>&1 | tail -1 | grep -q 'nothing to do'"
  check "case D: still the same rows after the second up" test "$(count "$NEW_IMG" person)" = 2
fi

# --- case E: a fresh install of this release (no 2.x volume): the one-shot does nothing ---
echo "=== case E: fresh install ==="
docker compose down -v --remove-orphans >/dev/null 2>&1
docker volume ls -q --filter label=com.docker.compose.project=fw-upgrade | xargs docker volume rm >/dev/null 2>&1
printf 'JWT_SECRET=x\nENCRYPTION_KEY=y-long-enough-key\nBACKUP_ENCRYPTION_KEY=%s\nEUNOMIA_IMAGE_TAG=fw-local\n' "$BACKUP_ENCRYPTION_KEY" > "$W/.env"
cp "$W/compose.new" "$W/docker-compose.yml"
docker compose up -d --wait --pull never surrealdb backup backend >/dev/null 2>&1; rc=$?
check "case E: fresh up succeeds" test "$rc" -eq 0
check "case E: the one-shot found nothing to move" bash -c "docker compose logs surreal-upgrade 2>&1 | grep -q 'no 2.x data volume: nothing to do'"
check "case E: 3.3 on the v3 volume, no 2.x volume created" bash -c "[ '$(running_image)' = '$NEW_IMG' ] && [ -z \"\$(docker volume ls -q --filter name=fw-upgrade_eunomia-surreal-data\$)\" ]"
check "case E: .env not touched" bash -c "! grep -q SURREAL_DATA_VOLUME '$W/.env'"

echo "upgrade-surreal-v3: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]

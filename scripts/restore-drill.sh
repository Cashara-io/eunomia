#!/usr/bin/env bash
# Restore drill: proves backups can actually be restored. Uses a throwaway
# SurrealDB (container fw-backup-db, host port 8211) and the real backup image,
# never your live stack. Seeds records, backs up, wipes, restores, compares
# counts. Exits non-zero on any mismatch. Always cleans up.
#   scripts/restore-drill.sh
set -euo pipefail
cd "$(dirname "$0")/.."

DB=fw-backup-db NET=fw-backup-net IMG=fw-backup-img VOL=fw-backup-drill-vol
KEY="drill-$(openssl rand -hex 8)"
cleanup() { docker rm -f "$DB" >/dev/null 2>&1 || true; docker network rm "$NET" >/dev/null 2>&1 || true
            docker volume rm "$VOL" >/dev/null 2>&1 || true; }
trap cleanup EXIT
cleanup

docker build -q -t "$IMG" backend/scripts/backup >/dev/null
docker network create "$NET" >/dev/null
docker volume create "$VOL" >/dev/null
docker run -d --name "$DB" --network "$NET" -p 127.0.0.1:8211:8000 --user root surrealdb/surrealdb:v3.3.5 \
  start --user root --pass root memory >/dev/null

# Run the backup image against the throwaway DB.
bk() { docker run --rm -i --network "$NET" -v "$VOL:/backups" -e BACKUP_ENCRYPTION_KEY="$KEY" \
         -e SURREAL_ENDPOINT="http://$DB:8000" -e SURREAL_NS=drill -e SURREAL_DB=drill "$@"; }
sql() { echo "$1" | bk --entrypoint surreal "$IMG" sql --endpoint "http://$DB:8000" --user root --pass root \
         --ns drill --db drill --hide-welcome --json; }
count() { sql "SELECT count() FROM $1 GROUP ALL;" | grep -o '"count":[0-9]*' | head -1 | cut -d: -f2; }

for i in $(seq 30); do sql "INFO FOR DB;" >/dev/null 2>&1 && break; sleep 1; done

sql "DEFINE TABLE person SCHEMALESS; DEFINE TABLE memory SCHEMALESS;
     CREATE person:a SET name='Ada'; CREATE person:b SET name='Bo'; CREATE person:c SET name='Cy';
     CREATE memory:1 SET text='one'; CREATE memory:2 SET text='two';" >/dev/null
p0=$(count person); m0=$(count memory)
echo "seeded: person=$p0 memory=$m0"

bk "$IMG" now manual
file=$(bk --entrypoint sh "$IMG" -c 'ls -1 /backups | tail -1')

sql "REMOVE DATABASE drill;" >/dev/null
p1=$(count person || true); echo "after wipe: person=${p1:-0}"
[ "${p1:-0}" = 0 ] || { echo "FAIL: wipe did not empty the database" >&2; exit 1; }

bk "$IMG" restore "$file"
p2=$(count person); m2=$(count memory)
echo "restored: person=$p2 memory=$m2"

if [ "$p0" = "$p2" ] && [ "$m0" = "$m2" ] && [ "$p0" = 3 ] && [ "$m0" = 2 ]; then
  echo "PASS: restore drill matched ($p2 person, $m2 memory)"
else
  echo "FAIL: counts differ (before person=$p0 memory=$m0, after person=$p2 memory=$m2)" >&2; exit 1
fi

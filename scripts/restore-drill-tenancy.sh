#!/usr/bin/env bash
# Restore drill for the tenancy layout: one backup is a DIRECTORY holding a file per database
# (`control` plus `org_<32 hex>`). Seeds both, backs up with the real backup image, removes both
# databases, restores the directory, compares per-database per-table counts. Throwaway SurrealDB
# container fw-backup-db (host port 8211), never your live stack.
#   scripts/restore-drill-tenancy.sh
set -euo pipefail
cd "$(dirname "$0")/.."

DB=fw-backup-db NET=fw-backup-net IMG=fw-backup-img VOL=fw-backup-drill-vol
KEY="drill-$(openssl rand -hex 8)"
ORG="org_$(openssl rand -hex 16)"
cleanup() { docker rm -f "$DB" >/dev/null 2>&1 || true; docker network rm "$NET" >/dev/null 2>&1 || true
            docker volume rm "$VOL" >/dev/null 2>&1 || true; }
trap cleanup EXIT
cleanup

docker build -q -t "$IMG" backend/scripts/backup >/dev/null
docker network create "$NET" >/dev/null
docker volume create "$VOL" >/dev/null
docker run -d --name "$DB" --network "$NET" -p 127.0.0.1:8211:8000 --user root surrealdb/surrealdb:v3.3.5 \
  start --user root --pass root memory >/dev/null

# Namespace eunomia; the default database "eunomia" is absent on purpose.
bk() { docker run --rm -i --network "$NET" -v "$VOL:/backups" -e BACKUP_ENCRYPTION_KEY="$KEY" \
         -e SURREAL_ENDPOINT="http://$DB:8000" -e SURREAL_NS=eunomia "$@"; }
sql() { # sql DB 'statements'
  echo "$2" | bk --entrypoint surreal "$IMG" sql --endpoint "http://$DB:8000" --user root --pass root \
    --ns eunomia --db "$1" --hide-welcome --json; }
count() { sql "$1" "SELECT count() FROM $2 GROUP ALL;" | grep -o '"count":[0-9]*' | head -1 | cut -d: -f2; }
# one line per database and table: "<db>.<table>=<rows>"
snapshot() { local t; for t in org user; do echo "control.$t=$(count control $t || echo 0)"; done
             for t in memory entity; do echo "$ORG.$t=$(count "$ORG" $t || echo 0)"; done; }

for i in $(seq 30); do sql control "INFO FOR DB;" >/dev/null 2>&1 && break; sleep 1; done

sql control "DEFINE TABLE org SCHEMALESS; DEFINE TABLE user SCHEMALESS;
  CREATE org:a SET name='Acme'; CREATE user:u1 SET email='a@x'; CREATE user:u2 SET email='b@x';" >/dev/null
sql "$ORG" "DEFINE TABLE memory SCHEMALESS; DEFINE TABLE entity SCHEMALESS;
  CREATE memory:1 SET text='one'; CREATE memory:2 SET text='two'; CREATE memory:3 SET text='three';
  CREATE entity:e1 SET name='Ada';" >/dev/null
before="$(snapshot)"; echo "seeded:"; echo "$before"

bk "$IMG" now manual
name=$(bk --entrypoint sh "$IMG" -c 'ls -1 /backups | tail -1')
files=$(bk --entrypoint sh "$IMG" -c "ls -1 /backups/$name | tr '\n' ' '")
echo "backup $name holds: $files"
case "$files" in *control.surql.enc*"$ORG.surql.enc"*) ;; *) echo "FAIL: backup is not a per-database directory with control and $ORG" >&2; exit 1 ;; esac

sql control "REMOVE DATABASE control;" >/dev/null; sql "$ORG" "REMOVE DATABASE \`$ORG\`;" >/dev/null
wiped="$(snapshot)"; echo "after wipe:"; echo "$wiped"
echo "$wiped" | grep -qv '=0$' && { echo "FAIL: wipe did not empty the databases" >&2; exit 1; }

bk "$IMG" restore "$name"
after="$(snapshot)"; echo "restored:"; echo "$after"
if [ "$before" = "$after" ]; then echo "PASS: tenancy restore drill matched"; else echo "FAIL: counts differ after restore" >&2; exit 1; fi

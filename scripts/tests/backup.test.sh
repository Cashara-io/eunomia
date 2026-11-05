#!/usr/bin/env bash
# Tests backend/scripts/backup/eunomia-backup.sh against a `surreal` shim: a pre-tenancy install is
# backed up as one file, a tenancy install (a `control` database exists) as a directory with one file
# per database, restores replay every file into its own database, and rotation keeps the newest N of both.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPT="$HERE/../../backend/scripts/backup/eunomia-backup.sh"
PASS=0; FAIL=0
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
check() { local d="$1"; shift; if "$@" >/dev/null 2>&1; then PASS=$((PASS + 1)); else FAIL=$((FAIL + 1)); echo "FAIL: $d"; fi; }

mkdir -p "$TMP/bin" "$TMP/backups" "$TMP/tmpd"; export TMPDIR="$TMP/tmpd"
cat > "$TMP/bin/surreal" <<'SHIM'
#!/bin/sh
# pop FILE: the first line of FILE (removed), or fail when it is empty: per-call answers queued by a test
pop() { [ -s "$1" ] || return 1; head -n1 "$1"; sed -i.bak 1d "$1" && rm -f "$1.bak"; }
cmd="$1"; shift
db=""; file=""; ep=""
while [ $# -gt 0 ]; do case "$1" in --db) db="$2"; shift ;; --endpoint) ep="$2"; shift ;; --ns|--user|--pass|--log) shift ;; --*) ;; *) file="$1" ;; esac; shift; done
case "$cmd" in
  export) if [ -f "$SHIM_FAIL" ] && grep -q "$db" "$SHIM_FAIL"; then
            rm -f "$SHIM_FAIL"; echo "OPTION IMPORT; -- dump of $db -- padding padding padding padding padding padding"; printf 'CREATE half'
            [ "$SHIM_MODE" = truncate ] && exit 0; exit 1; fi
          echo "OPTION IMPORT; -- dump of $db -- padding padding padding padding padding padding padding padding;"
          echo "DEFINE TABLE t TYPE ANY SCHEMALESS PERMISSIONS NONE;"; echo "-- TABLE DATA: t"; echo "-- ------"; echo
          # MOCK_STUB: schema only, as after a server that died mid-export; MOCK_EMPTY: a database with no rows.
          # Otherwise 5 rows (or the next line of $SHIM_ROWS), the first with braces and brackets in its values
          # and id, which must not count as rows.
          [ -n "${MOCK_STUB:-}${MOCK_EMPTY:-}" ] && exit 0
          # MOCK_MULTILINE: a 2.x export, whose strings keep raw newlines (and a comment with a quote):
          # 5 rows, with brackets, braces and quotes inside multi-line values
          if [ -n "${MOCK_MULTILINE:-}" ]; then
            echo "-- the server's own note: don't count this"
            printf '%s\n' "INSERT [ { id: t:1, text: \"line one" "} { fake ] [ 'quote" "line three\" }, { id: t:2, s: 'a" "{ b }' }," "{ id: t:⟨x" "y⟩ }, { id: t:4, n: [ { deep: 1 } ] }, { id: t:5 } ];"
            exit 0
          fi
          n="$(pop "$SHIM_ROWS" || echo 5)"; [ "$n" -gt 0 ] || exit 0; rows="{ id: t:\`a}{\`, nested: { a: [ { b: 1 } ] }, s: 'it''s }, { id: t:9 }', q: \"[{\\\"}\" }"
          i=2; while [ "$i" -le "$n" ]; do rows="$rows, { id: t:$i }"; i=$((i + 1)); done
          echo "INSERT [ $rows ];" ;;
  # A server started with --deny-all (the real compose file) refuses isready over http(s):// with
  # "Method not allowed" while ws(s):// passes, so the script must ask over the websocket form.
  isready) case "$ep" in http://*|https://*) echo "ERROR surrealdb_server::cli: Method not allowed" >&2; exit 1 ;; esac
           [ -z "${MOCK_DEAD:-}" ] ;;
  import) echo "import $db: $(cat "$file")" >> "$SHIM_LOG" ;;
  sql) in="$(cat)"; echo "sql: $in" >> "$SHIM_LOG"
       case "$in" in
         "INFO FOR DB;") echo '[{"tables":{"t":"DEFINE TABLE t TYPE ANY SCHEMALESS PERMISSIONS NONE"}}]' ;;
         *"SELECT count() FROM"*) c="$(pop "$SHIM_COUNTS")" || { [ -n "${MOCK_EMPTY:-}" ] && c=0 || c=5; }
            echo "[[{\"count\":$c}]]" ;;
         "INFO FOR NS;") [ -z "${MOCK_INFO_FAIL:-}" ] || { echo "connection refused" >&2; exit 1; }
            [ -z "${MOCK_INFO_GARBAGE:-}" ] || { echo '[{"error":"namespace does not exist"}]'; exit 0; }
            if [ -n "${MOCK_TENANCY:-}" ]; then
              echo '[{"databases":{"control":"DEFINE DATABASE control","eunomia":"DEFINE DATABASE eunomia","org_0123456789abcdef0123456789abcdef":"DEFINE DATABASE org_0123456789abcdef0123456789abcdef"}}]'
            else echo '[{"databases":{"eunomia":"DEFINE DATABASE eunomia"}}]'; fi ;;
       esac ;;
esac
SHIM
chmod +x "$TMP/bin/surreal"
export PATH="$TMP/bin:$PATH" SHIM_LOG="$TMP/shim.log" SHIM_FAIL="$TMP/shim.fail" SHIM_ROWS="$TMP/shim.rows" SHIM_COUNTS="$TMP/shim.counts" BACKUP_DIR="$TMP/backups" BACKUP_ENCRYPTION_KEY=testkey SURREAL_NS=eunomia SURREAL_DB=eunomia
bk() { sh "$SCRIPT" "$@"; }
# decrypt a new-format file by restoring it through the script (the shim logs the import)
dump_of() { : > "$SHIM_LOG"; sh "$SCRIPT" restore "$1" >/dev/null 2>&1; cat "$SHIM_LOG"; }

# 0. a 2.x export whose strings span lines is counted right (190 rows read as 1 broke an upgrade)
eval "$(sed -n '/^dump_counts() {/,/^}/p' "$SCRIPT")"
MOCK_MULTILINE=1 surreal export --db eunomia - > "$TMP/multi.surql"
check "multi-line 2.x export: every row counted" test "$(dump_counts "$TMP/multi.surql")" = "t 5"
check "multi-line 2.x export: the backup verifies" sh -c "MOCK_MULTILINE=1 sh '$SCRIPT' now ml"
check "multi-line 2.x export: a missing row is still refused" sh -c "printf '6\\n6\\n' > '$SHIM_COUNTS'; ! MOCK_MULTILINE=1 sh '$SCRIPT' now ml2"
rm -f "$TMP"/backups/ml*; : > "$SHIM_COUNTS"

# 1. a pre-tenancy install: one file, as before
bk now manual >/dev/null
f="$(ls "$TMP"/backups/manual-*.surql.enc | head -1)"
check "single-database install writes one file" test -f "$f"
check "that file holds the database export" bash -c "grep -q 'dump of eunomia' <<< \"\$(sh '$SCRIPT' restore '$f' >/dev/null 2>&1; cat '$SHIM_LOG')\""
check "new files carry the authenticated header" sh -c "head -n1 '$f' | grep -q '^EUNOMIA-BK2 '"
check "round trip restores the exact dump" sh -c ": > '$SHIM_LOG'; sh '$SCRIPT' restore '$f' && grep -q 'import eunomia: OPTION IMPORT; -- dump of eunomia' '$SHIM_LOG'"
cp "$f" "$TMP/tampered.surql.enc"; printf 'X' | dd of="$TMP/tampered.surql.enc" bs=1 seek=60 conv=notrunc 2>/dev/null
check "a tampered file is refused before import" sh -c ": > '$SHIM_LOG'; sh '$SCRIPT' restore '$TMP/tampered.surql.enc' 2>&1 | grep -q 'integrity check' && ! grep -q import '$SHIM_LOG'"
printf 'legacy dump of eunomia\n' | openssl enc -aes-256-cbc -pbkdf2 -salt -pass env:BACKUP_ENCRYPTION_KEY > "$TMP/legacy.surql.enc"
check "an old-format file still restores" sh -c ": > '$SHIM_LOG'; sh '$SCRIPT' restore '$TMP/legacy.surql.enc' && grep -q 'legacy dump of eunomia' '$SHIM_LOG'"

# 2. a tenancy install: a directory with a file per database (control, each org, the old database)
rm -rf "$TMP/backups"/*; sleep 1
MOCK_TENANCY=1 bk now manual >/dev/null
d="$(ls -d "$TMP"/backups/manual-* | head -1)"
check "tenancy install writes a directory" test -d "$d"
check "control, the org and the old database are each backed up" test "$(ls "$d" | sort | tr '\n' ' ')" = "control.surql.enc eunomia.surql.enc org_0123456789abcdef0123456789abcdef.surql.enc "
check "no half-written .part is left" test -z "$(ls -d "$TMP"/backups/*.part 2>/dev/null)"

# 3. restoring the directory imports every database into its own database
: > "$SHIM_LOG"
bk restore "$(basename "$d")" --wipe >/dev/null
check "each database is wiped, created and imported" sh -c "grep -c 'import ' '$SHIM_LOG' | grep -qx 3 && grep -q 'REMOVE DATABASE IF EXISTS .control.' '$SHIM_LOG' && grep -q 'import control: OPTION IMPORT; -- dump of control' '$SHIM_LOG'"
check "a wrong key refuses clearly" sh -c "BACKUP_ENCRYPTION_KEY=nope sh '$SCRIPT' restore '$(basename "$d")' 2>&1 | grep -q 'integrity check'"

# 3a. uploaded documents (the file bucket folder) go into a tenancy backup as documents.tar.enc and come back
rm -rf "$TMP/backups"/*; sleep 1
mkdir -p "$TMP/docs/org_x/abc/1"; printf 'original bytes\n' > "$TMP/docs/org_x/abc/1/notes.md"
DOCUMENTS_DIR="$TMP/docs" MOCK_TENANCY=1 bk now manual >/dev/null
d="$(ls -d "$TMP"/backups/manual-* | head -1)"
check "the documents folder is archived, encrypted" sh -c "test -f '$d/documents.tar.enc' && head -n1 '$d/documents.tar.enc' | grep -q '^EUNOMIA-BK2 ' && ! grep -q 'original bytes' '$d/documents.tar.enc'"
rm -rf "$TMP/docs"/*; mkdir -p "$TMP/docs/stale"
DOCUMENTS_DIR="$TMP/docs" bk restore "$(basename "$d")" --wipe >/dev/null 2>&1
check "restore --wipe brings the documents back and drops what was not in the backup" sh -c "test \"\$(cat '$TMP/docs/org_x/abc/1/notes.md')\" = 'original bytes' && test ! -e '$TMP/docs/stale'"
rm -rf "$TMP/docs"
DOCUMENTS_DIR="$TMP/docs" bk restore "$(basename "$d")" >/dev/null 2>&1
check "restore creates a missing documents folder (a fresh data volume)" test -f "$TMP/docs/org_x/abc/1/notes.md"
rm -rf "$TMP/docs"; rm -rf "$TMP/backups"/*; sleep 1
DOCUMENTS_DIR="$TMP/docs" MOCK_TENANCY=1 bk now manual >/dev/null
check "no documents folder, no archive" test -z "$(ls "$TMP"/backups/manual-*/documents.tar.enc 2>/dev/null)"

# 3b. no BACKUP_ENCRYPTION_KEY: a key is generated once into the backup volume and reused
rm -rf "$TMP/backups"/*
out1="$(unset BACKUP_ENCRYPTION_KEY; bk now manual 2>&1)"
case "$out1" in *"COPY IT SOMEWHERE SAFE"*) PASS=$((PASS + 1)) ;; *) FAIL=$((FAIL + 1)); echo "FAIL: first start generates a key and says so";; esac
check "the key file exists and is private" test -s "$TMP/backups/.backup-key" -a "$(ls -l "$TMP/backups/.backup-key" | cut -c1-10)" = "-rw-------"
sleep 1
out2="$(unset BACKUP_ENCRYPTION_KEY; bk now manual 2>&1)"
case "$out2" in *"COPY IT"*) FAIL=$((FAIL + 1)); echo "FAIL: the notice is printed only once";; *) PASS=$((PASS + 1));; esac
g="$(ls "$TMP"/backups/manual-*.surql.enc | head -1)"
check "generated-key backups restore with the key file" sh -c "unset BACKUP_ENCRYPTION_KEY; sh '$SCRIPT' restore '$g'"

# 4. rotation keeps the newest N, files and directories alike
rm -rf "$TMP/backups"/*
for s in 1 2 3 4; do mkdir "$TMP/backups/daily-2026010${s}T000000Z"; done; touch "$TMP/backups/daily-20260105T000000Z.surql.enc"
KEEP_DAILY=2 MOCK_TENANCY=1 bk now daily >/dev/null
check "only the newest two daily backups stay" test "$(ls -1d "$TMP"/backups/daily-* | wc -l | tr -d ' ')" = 2

# 5. an export that dies midway (non-zero exit, no closing statement) never becomes a backup
rm -rf "$TMP/backups"/*
echo eunomia > "$SHIM_FAIL"; SHIM_MODE=die bk now manual >/dev/null 2>&1; rc=$?
check "a failed export makes the command fail" test "$rc" -ne 0
check "a failed export leaves no backup file" test -z "$(ls -A "$TMP/backups" | grep -v '^\.backup-key$')"
echo eunomia > "$SHIM_FAIL"; SHIM_MODE=truncate bk now manual >/dev/null 2>&1; rc=$?
check "an export that exits 0 but is cut off is refused too" test "$rc" -ne 0 -a -z "$(ls "$TMP/backups" | grep -v '^\.backup-key$')"
check "no plaintext temp dump is left behind" test -z "$(ls -A "$TMP/tmpd")"
rm -rf "$TMP/backups"/*; echo control > "$SHIM_FAIL"; SHIM_MODE=die MOCK_TENANCY=1 bk now manual >/dev/null 2>&1
check "a tenancy backup failing on one database leaves no directory" test -z "$(ls "$TMP/backups")"

# 5a. a backup must prove it holds the data: the server counts rows around the export, the dump must contain them
rm -rf "$TMP/backups"/*
MOCK_STUB=1 bk now manual >"$TMP/stub.out" 2>&1; rc=$?
check "a schema-only export while the server reports rows is refused" test "$rc" -ne 0 -a -z "$(ls "$TMP/backups" | grep -v '^\.backup-key$')"
check "the refusal says which table lost its rows" grep -q 'does not hold the rows the server counted: t(5 rows counted, 0 in the export)' "$TMP/stub.out"
MOCK_TENANCY=1 MOCK_STUB=1 bk now manual >/dev/null 2>&1; rc=$?
check "a tenancy backup with a stub database leaves no directory" test "$rc" -ne 0 -a -z "$(ls "$TMP/backups" | grep -v '^\.backup-key$')"
MOCK_DEAD=1 bk now manual >/dev/null 2>&1; rc=$?
check "a server that stopped answering after the export is refused" test "$rc" -ne 0 -a -z "$(ls "$TMP/backups" | grep -v '^\.backup-key$')"
MOCK_EMPTY=1 MOCK_STUB=1 bk now manual >/dev/null 2>&1; rc=$?
check "a database with no rows is a valid backup" test "$rc" -eq 0 -a -n "$(ls "$TMP"/backups/manual-*.surql.enc 2>/dev/null)"
rm -rf "$TMP/backups"/*; sleep 1
bk now manual >/dev/null 2>&1; rc=$?
check "a normal export with rows is accepted (braces inside values and ids are not rows)" test "$rc" -eq 0 -a -n "$(ls "$TMP"/backups/manual-*.surql.enc 2>/dev/null)"

# 5c. presence is not enough: the dump must hold as many rows as the server counted, per table
rm -rf "$TMP/backups"/*
echo 4 > "$SHIM_ROWS"; bk now manual >"$TMP/short.out" 2>&1; rc=$?
check "an export holding fewer rows than the server counted is refused" test "$rc" -ne 0 -a -z "$(ls "$TMP/backups" | grep -v '^\.backup-key$')"
check "the refusal gives both numbers" grep -q 't(5 rows counted, 4 in the export)' "$TMP/short.out"
echo 6 > "$SHIM_ROWS"; bk now manual >/dev/null 2>&1; rc=$?
check "an export holding more rows than the server counted is refused" test "$rc" -ne 0 -a -z "$(ls "$TMP/backups" | grep -v '^\.backup-key$')"
printf '4\n5\n' > "$SHIM_ROWS"; bk now manual >"$TMP/stable.out" 2>&1; rc=$?
check "with the counts unchanged around the export, a short dump is not retried" sh -c "test $rc -ne 0 && ! grep -q 'exporting it again' '$TMP/stable.out'"
: > "$SHIM_ROWS"
# rows written while the export runs: the counts are taken before and after it
printf '5\n7\n' > "$SHIM_COUNTS"; echo 6 > "$SHIM_ROWS"; bk now manual >/dev/null 2>&1; rc=$?
check "rows written during the export: a dump between the two counts is accepted" test "$rc" -eq 0 -a -n "$(ls "$TMP"/backups/manual-*.surql.enc 2>/dev/null)"
rm -rf "$TMP/backups"/*; sleep 1
printf '5\n7\n7\n7\n' > "$SHIM_COUNTS"; printf '4\n7\n' > "$SHIM_ROWS"; bk now manual >"$TMP/retry.out" 2>&1; rc=$?
check "a dump outside counts that moved is exported again, and kept once it matches" sh -c "test $rc -eq 0 && grep -q 'exporting it again (attempt 2 of 3)' '$TMP/retry.out' && ls '$TMP'/backups/manual-*.surql.enc"
rm -rf "$TMP/backups"/*
printf '1\n2\n3\n4\n5\n6\n' > "$SHIM_COUNTS"; printf '0\n0\n0\n' > "$SHIM_ROWS"; bk now manual >"$TMP/moving.out" 2>&1; rc=$?
check "a database that keeps changing gives up after 3 exports, writing nothing" sh -c "test $rc -ne 0 && grep -q 'attempt 3 of 3' '$TMP/moving.out' && ! grep -q 'attempt 4' '$TMP/moving.out' && test -z \"\$(ls '$TMP/backups' | grep -v '^\.backup-key$')\""
: > "$SHIM_COUNTS"; : > "$SHIM_ROWS"

# 5b. a failing INFO FOR NS is an error, never a quiet fallback to the stale single database
rm -rf "$TMP/backups"/*
MOCK_TENANCY=1 MOCK_INFO_FAIL=1 bk now manual >/dev/null 2>&1; rc=$?
check "INFO FOR NS failing makes the backup fail" test "$rc" -ne 0
check "INFO FOR NS failing writes no backup" test -z "$(ls "$TMP/backups" | grep -v '^\.backup-key$')"
MOCK_INFO_GARBAGE=1 bk now manual >/dev/null 2>&1; rc=$?
check "an unexpected INFO FOR NS answer fails too" test "$rc" -ne 0 -a -z "$(ls "$TMP/backups" | grep -v '^\.backup-key$')"
bk now manual >/dev/null 2>&1; rc=$?
check "no control database (pre-tenancy) still falls back to one file" test "$rc" -eq 0 -a -n "$(ls "$TMP"/backups/manual-*.surql.enc 2>/dev/null)"

# 6. the nightly loop survives a failed run and the next one succeeds
rm -rf "$TMP/backups"/*
mkdir -p "$TMP/loopbin"
cat > "$TMP/loopbin/sleep" <<'S'
#!/bin/sh
n=$(( $(cat "$SLEEPS" 2>/dev/null || echo 0) + 1 )); echo "$n" > "$SLEEPS"
[ "$n" -lt 3 ] || { kill "$(cat "$LOOPPID")"; sleep 5; }
S
cat > "$TMP/loopbin/date" <<'S'
#!/bin/sh
case "$*" in *-d*) echo 99999999999 ;; *) exec /bin/date "$@" ;; esac
S
chmod +x "$TMP/loopbin/sleep" "$TMP/loopbin/date"
echo eunomia > "$SHIM_FAIL"; SHIM_MODE=die
export SLEEPS="$TMP/sleeps" LOOPPID="$TMP/looppid" SHIM_MODE
# the shim's `sleep 5` after the kill must be the real one: call it by path
sed -i.bak 's#; sleep 5; }#; /bin/sleep 5; }#' "$TMP/loopbin/sleep"; rm -f "$TMP/loopbin/sleep.bak"
PATH="$TMP/loopbin:$PATH" sh -c 'echo $$ > "$LOOPPID"; exec sh "$0" loop' "$SCRIPT" > "$TMP/loop.log" 2>&1 &
wait $! 2>/dev/null
check "the loop logs the failure and keeps going" grep -q 'nightly backup failed' "$TMP/loop.log"
check "the next night's run still produced a backup" test -n "$(ls "$TMP"/backups/daily-* 2>/dev/null)"

echo "backup.test.sh: $PASS passed, $FAIL failed"
[ "$FAIL" = 0 ]

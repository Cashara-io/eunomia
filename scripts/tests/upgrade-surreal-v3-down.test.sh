#!/usr/bin/env bash
# Mock-based test of the "surrealdb is not running" path of scripts/upgrade-surreal-v3.sh
# (an install broken by an old updater that pulled a 3.x release over 2.x data). No Docker needed.
# The real recovery (temp 2.x server on a copy, backup, export, import) is covered only by
# scripts/tests/upgrade-surreal-v3.test.sh with Docker, which has not been run for this path.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PASS=0; FAIL=0
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
check() { local d="$1"; shift; if "$@" >/dev/null 2>&1; then PASS=$((PASS + 1)); else FAIL=$((FAIL + 1)); echo "FAIL: $d"; fi; }
mkdir -p "$T/bin" "$T/repo"; : > "$T/repo/.env"; touch "$T/repo/docker-compose.import.yml"
cat > "$T/bin/docker" <<SHIM
#!/bin/sh
echo "\$@" >> "$T/docker.log"
case "\$*" in
  "compose ps -q backend"*) cat "$T/bid" 2>/dev/null ;;    # the backend, when one exists
  "compose ps -q"*) ;;                                     # nothing else running
  "compose run --rm --no-deps -T -v"*backup*) [ -f "$T/backup_fails" ] && exit 1 ;;
  "compose config --images") echo surrealdb/surrealdb:v3.3.5 ;;
  "volume ls"*) cat "$T/volume" 2>/dev/null ;;
  "network inspect"*) [ -f "$T/net" ] || exit 1 ;;
  "inspect -f {{.State.Running}} proj-v2scratch") cat "$T/scratch_running" 2>/dev/null ;;
esac
exit 0
SHIM
chmod +x "$T/bin/docker"
run() { EUNOMIA_DIR="$T/repo" COMPOSE_PROJECT_NAME=proj PATH="$T/bin:$PATH" bash "$HERE/../upgrade-surreal-v3.sh" 2>&1; }

: > "$T/docker.log"; rm -f "$T/volume"
out="$(run)"; rc=$?
check "down, no data volume: exits 0, nothing to upgrade" test "$rc" -eq 0
check "down, no data volume: says so" grep -q 'nothing to upgrade' <<<"$out"

echo proj_eunomia-surreal-data > "$T/volume"; : > "$T/docker.log"
out="$(run)"; rc=$?
check "down, volume found, no network: fails with a hint" test "$rc" -ne 0
check "down: the original volume is never mounted" bash -c "! grep -q 'proj_eunomia-surreal-data:/data' '$T/docker.log'"
check "down: no container was started" bash -c "! grep -q '^run -d' '$T/docker.log'"

# 3.x data in a stopped install: the 2.x server cannot open the copy, so the script leaves everything alone
touch "$T/net"; rm -f "$T/scratch_running"; : > "$T/docker.log"
out="$(run)"; rc=$?
check "down, 2.x cannot open the data: exits 0" test "$rc" -eq 0
check "down, 2.x cannot open the data: says nothing was changed" grep -q 'Nothing was changed' <<<"$out"
check "down, 2.x cannot open the data: backend untouched" bash -c "! grep -q 'compose stop backend' '$T/docker.log'"
check "down, 2.x cannot open the data: scratch container removed" grep -q 'rm -f proj-v2scratch' "$T/docker.log"

# 2.x data, the temp server on the copy comes up (answering as `surrealdb`), then the backup fails: the
# rollback must remove that temp server BEFORE it restarts the backend, or the backend writes to the copy
echo true > "$T/scratch_running"; echo bid123 > "$T/bid"; touch "$T/backup_fails"; : > "$T/docker.log"
out="$(run)"; rc=$?
check "down, backup fails: exits non-zero" test "$rc" -ne 0
check "down, backup fails: rolls back" grep -q 'ROLLING BACK' <<<"$out"
first_rm="$(grep -n 'rm -f proj-v2scratch' "$T/docker.log" | head -1 | cut -d: -f1)"
backend_start="$(grep -n '^start bid123' "$T/docker.log" | head -1 | cut -d: -f1)"
check "down, backup fails: the backend is restarted" test -n "$backend_start"
check "down, backup fails: the temp server is gone before the backend restarts" test -n "$first_rm" -a "${first_rm:-0}" -lt "${backend_start:-0}"
rm -f "$T/bid" "$T/backup_fails" "$T/scratch_running"

echo SURREAL_DATA_VOLUME=eunomia-surreal-data-v3 > "$T/repo/.env"; : > "$T/docker.log"
out="$(run)"; rc=$?
check "down but already on the v3 volume: exits 0" test "$rc" -eq 0
echo "upgrade-down: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]

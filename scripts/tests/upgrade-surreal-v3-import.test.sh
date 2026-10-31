#!/usr/bin/env bash
# Mock-docker run of the whole "surrealdb is down" path of scripts/upgrade-surreal-v3.sh, to prove the
# ordering the real-container test cannot show cheaply: the import runs on a surrealdb started with
# docker-compose.import.yml (no 60 s timeouts), and the plain hardened compose file is what is started
# before the counts are checked and the new volume is recorded. No Docker needed.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$HERE/../.."
PASS=0; FAIL=0
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
check() { local d="$1"; shift; if "$@" >/dev/null 2>&1; then PASS=$((PASS + 1)); else FAIL=$((FAIL + 1)); echo "FAIL: $d"; fi; }
mkdir -p "$T/bin" "$T/repo"; : > "$T/repo/.env"; touch "$T/repo/docker-compose.import.yml"
printf "OPTION IMPORT;\nDEFINE TABLE person TYPE ANY SCHEMALESS;\nINSERT [ { id: person:a } ];\n" > "$T/export.surql"
cat > "$T/bin/docker" <<SHIM
#!/bin/sh
echo "COMPOSE_FILE=\${COMPOSE_FILE:-} :: \$@" >> "$T/docker.log"
case "\$*" in
  "compose ps -q"*) ;;
  "compose config --images") echo surrealdb/surrealdb:v3.3.5 ;;
  "volume ls"*) echo proj_eunomia-surreal-data ;;
  "inspect -f {{.State.Running}} "*) echo true ;;
  "create"*) echo cid ;;
  "start -a"*) cat "$T/export.surql" ;;
  "run --rm -i"*) cat >/dev/null; echo '[{"person":1}]' ;;
esac
exit 0
SHIM
chmod +x "$T/bin/docker"
cd "$T/repo" || exit 1
out="$(EUNOMIA_DIR="$T/repo" COMPOSE_PROJECT_NAME=proj COMPOSE_FILE=docker-compose.yml PATH="$T/bin:$PATH" bash "$ROOT/scripts/upgrade-surreal-v3.sh" 2>&1)"; rc=$?
check "the upgrade completes against the mock" test "$rc" -eq 0
[ "$rc" -eq 0 ] || echo "$out" | tail -5
L="$T/docker.log"
imp="$(grep -n 'COMPOSE_FILE=.*docker-compose.import.yml :: compose up -d --wait --no-deps surrealdb' "$L" | head -1 | cut -d: -f1)"
import="$(grep -n ' :: create .* import ' "$L" | head -1 | cut -d: -f1)"
plain="$(grep -n 'COMPOSE_FILE=docker-compose.yml :: compose up -d --wait --no-deps surrealdb' "$L" | tail -1 | cut -d: -f1)"
check "3.x is started with the import override before the import" test -n "$imp" -a -n "$import" -a "${imp:-9}" -lt "${import:-0}"
check "the plain hardened compose file is started after the import" test -n "$plain" -a "${plain:-0}" -gt "${import:-9999}"
check "no override file is layered on the hardened start" bash -c "! tail -n +\$((${plain:-1})) '$L' | head -1 | grep -q import.yml"
check "the new volume is recorded only after that" grep -q '^SURREAL_DATA_VOLUME=eunomia-surreal-data-v3$' "$T/repo/.env"

# under the one-shot surreal-upgrade service: the same order, on one-off containers of the service,
# and the container that `docker compose up` created is never replaced (no `compose up` of surrealdb)
: > "$L"; : > "$T/repo/.env"
EUNOMIA_DIR="$T/repo" COMPOSE_PROJECT_NAME=proj COMPOSE_FILE=docker-compose.yml SURREAL_UPGRADE_ONESHOT=1 PATH="$T/bin:$PATH" bash "$ROOT/scripts/upgrade-surreal-v3.sh" >/dev/null 2>&1; rc=$?
check "one-shot: completes" test "$rc" -eq 0
imp="$(grep -n 'COMPOSE_FILE=.*docker-compose.import.yml :: compose run -d --no-deps --use-aliases --name proj-v3oneoff surrealdb' "$L" | head -1 | cut -d: -f1)"
import="$(grep -n ' :: create .* import ' "$L" | head -1 | cut -d: -f1)"
plain="$(grep -n 'COMPOSE_FILE=docker-compose.yml :: compose run -d --no-deps --use-aliases --name proj-v3oneoff surrealdb' "$L" | tail -1 | cut -d: -f1)"
check "one-shot: import override one-off, import, then hardened one-off" test -n "$imp" -a -n "$import" -a -n "$plain" -a "${imp:-9}" -lt "${import:-0}" -a "${plain:-0}" -gt "${import:-9999}"
check "one-shot: never runs compose up/rm on surrealdb" bash -c "! grep -qE ':: compose (up|rm) .*surrealdb' '$L'"
check "one-shot: the one-off is stopped before the volume is recorded" grep -q ':: stop -t 60 proj-v3oneoff' "$L"
check "one-shot: records the new volume" grep -q '^SURREAL_DATA_VOLUME=eunomia-surreal-data-v3$' "$T/repo/.env"

rm "$T/repo/docker-compose.import.yml"; : > "$L"
EUNOMIA_DIR="$T/repo" COMPOSE_PROJECT_NAME=proj PATH="$T/bin:$PATH" bash "$ROOT/scripts/upgrade-surreal-v3.sh" >/dev/null 2>&1; rc=$?
check "a checkout without the override file stops before touching anything" bash -c "[ $rc -ne 0 ] && ! grep -q 'run -d\|stop' '$L'"
echo "upgrade-import: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]

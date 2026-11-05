#!/usr/bin/env bash
# Fails when the SurrealDB version is not the same, to the patch, everywhere it is pinned: the
# backend crate (backend/Cargo.toml and Cargo.lock), the server image (docker-compose.yml), the
# backup CLI (backend/scripts/backup/Dockerfile), the CI claim-db container (ci.yml), the restore
# drills, the upgrade test's target image and run.sh. A crate on one patch and a server on another
# is what nobody tests, so the pins move together (docs/releasing.md).
# Usage: scripts/ci/check-surreal-pins.sh [repo-root]   (default: this checkout). Exit 0 ok, 1 drift.
set -uo pipefail
ROOT="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"
cd "$ROOT" || exit 1
rc=0
fail() { echo "DRIFT: $*"; rc=1; }

crate="$(sed -n 's#^surrealdb *= *{.*version *= *"=\([0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*\)".*#\1#p' backend/Cargo.toml | head -1)"
[ -n "$crate" ] || { echo "DRIFT: backend/Cargo.toml has no exact surrealdb pin (version = \"=X.Y.Z\")"; exit 1; }
lock="$(awk '$0 == "name = \"surrealdb\"" { getline; gsub(/version = |"/, ""); print; exit }' backend/Cargo.lock)"
[ "$lock" = "$crate" ] || fail "backend/Cargo.lock has surrealdb $lock, backend/Cargo.toml pins =$crate"

# Every surrealdb/surrealdb image of the crate's major in FILE must be v$crate, and there must be one.
# Images of another major (the 2.x images the upgrade test starts from) are not pins of this release.
major="${crate%%.*}"
check_file() { # check_file FILE [grep filter for the lines that hold the pin]
  local f="$1" filter="${2:-.}" found=0 v
  [ -f "$f" ] || { fail "$f is missing"; return; }
  for v in $(grep -E "$filter" "$f" | grep -oE "surrealdb/surrealdb:v?$major(\.[0-9A-Za-z-]+)*" | sed 's#.*:v\{0,1\}##'); do
    found=1
    [ "$v" = "$crate" ] || fail "$f pins surrealdb/surrealdb:v$v, backend/Cargo.toml pins =$crate"
  done
  [ "$found" = 1 ] || fail "$f has no surrealdb/surrealdb:v$major.x image (expected v$crate)"
}
check_file docker-compose.yml '^ *image:'
check_file backend/scripts/backup/Dockerfile '^FROM '
check_file .github/workflows/ci.yml
check_file scripts/restore-drill.sh
check_file scripts/restore-drill-tenancy.sh
check_file scripts/tests/upgrade-surreal-v3.test.sh 'NEW_IMG='
check_file run.sh

[ "$rc" = 0 ] && echo "ok: SurrealDB $crate everywhere it is pinned"
exit "$rc"

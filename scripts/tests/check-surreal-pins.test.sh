#!/usr/bin/env bash
# scripts/ci/check-surreal-pins.sh: passes on this checkout, and fails when any one pin drifts.
# Runs on copies of the pinned files in a temp dir. No Docker needed.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"; ROOT="$HERE/../.."; S="$ROOT/scripts/ci/check-surreal-pins.sh"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
PASS=0; FAIL=0; check() { local d="$1"; shift; if "$@" >/dev/null 2>&1; then PASS=$((PASS+1)); else FAIL=$((FAIL+1)); echo "FAIL: $d"; fi; }
FILES="backend/Cargo.toml backend/Cargo.lock docker-compose.yml backend/scripts/backup/Dockerfile .github/workflows/ci.yml
       scripts/restore-drill.sh scripts/restore-drill-tenancy.sh scripts/tests/upgrade-surreal-v3.test.sh run.sh"
fresh() { rm -rf "$T/r"; for f in $FILES; do mkdir -p "$T/r/$(dirname "$f")"; cp "$ROOT/$f" "$T/r/$f"; done; }
pin="$(sed -n 's#^surrealdb *= *{.*version *= *"=\([0-9.]*\)".*#\1#p' "$ROOT/backend/Cargo.toml")"
bump() { echo "${pin%.*}.$(( ${pin##*.} + 1 ))"; }   # the next patch: a version no file holds
other="$(bump)"
drift() { # drift DESCRIPTION FILE SED-EXPRESSION: that one edit must make the check fail and name the file
  fresh; sed -i.bak "$3" "$T/r/$2" && rm -f "$T/r/$2.bak"
  check "$1: refused" bash -c "! bash '$S' '$T/r'"
  check "$1: names $2" bash -c "bash '$S' '$T/r' | grep -q '$2'"
}

check "this checkout passes" bash "$S" "$ROOT"
fresh; check "a copy of the pinned files passes" bash "$S" "$T/r"
drift "compose image" docker-compose.yml "s#surrealdb/surrealdb:v$pin#surrealdb/surrealdb:v$other#"
drift "backup Dockerfile" backend/scripts/backup/Dockerfile "s#surrealdb/surrealdb:v$pin#surrealdb/surrealdb:v$other#"
drift "ci.yml claim-db" .github/workflows/ci.yml "s#surrealdb/surrealdb:v$pin#surrealdb/surrealdb:v$other#"
drift "restore drill" scripts/restore-drill.sh "s#surrealdb/surrealdb:v$pin#surrealdb/surrealdb:v$other#"
drift "tenancy restore drill" scripts/restore-drill-tenancy.sh "s#surrealdb/surrealdb:v$pin#surrealdb/surrealdb:v$other#"
drift "upgrade test target" scripts/tests/upgrade-surreal-v3.test.sh "s#NEW_IMG=surrealdb/surrealdb:v$pin#NEW_IMG=surrealdb/surrealdb:v$other#"
drift "run.sh" run.sh "s#surrealdb/surrealdb:v$pin#surrealdb/surrealdb:v$other#"
drift "compose minor-only pin" docker-compose.yml "s#surrealdb/surrealdb:v$pin#surrealdb/surrealdb:v${pin%.*}#"
drift "crate moved alone" backend/Cargo.toml "s#version = \"=$pin\"#version = \"=$other\"#"
drift "lock not updated" backend/Cargo.lock "/^name = \"surrealdb\"$/{n;s#version = \"$pin\"#version = \"$other\"#;}"
fresh; sed -i.bak "s#version = \"=$pin\"#version = \"$pin\"#" "$T/r/backend/Cargo.toml"
check "a crate pin without = is refused" bash -c "! bash '$S' '$T/r'"
fresh; sed -i.bak 's#^\(OLD_IMG=surrealdb/surrealdb:\)v[0-9.]*#\1v2.6.0#' "$T/r/scripts/tests/upgrade-surreal-v3.test.sh"
check "the 2.x image the upgrade test starts from is not a pin" bash "$S" "$T/r"
fresh; rm "$T/r/scripts/restore-drill.sh"
check "a missing pinned file is refused" bash -c "! bash '$S' '$T/r'"
echo "surreal-pins: $PASS passed, $FAIL failed"; [ "$FAIL" -eq 0 ]

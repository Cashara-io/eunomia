#!/usr/bin/env bash
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"; S="$HERE/../ci/check-update-path-rows.sh"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
PASS=0; FAIL=0; check() { local d="$1"; shift; if "$@" >/dev/null 2>&1; then PASS=$((PASS+1)); else FAIL=$((FAIL+1)); echo "FAIL: $d"; fi; }
yml() {
  { echo "jobs:"; echo "  update-path:"; echo "    strategy:"; echo "      matrix:"; echo "        include:"
    for r in "$@"; do echo "          - from: ${r%%@*}"; [ "$r" = "${r#*@}" ] || echo "            from_ref: ${r#*@}"; done
    echo "    steps:"; echo "      - run: true"; echo "  other:"; echo "    strategy:"; echo "      matrix:"; echo "        include:"
    echo "          - from: v9.9.9"; } > "$T/ci.yml"
}
run() { CI_YML="$T/ci.yml" UPDATE_PATH_TAGS=$'v2.0.7\nv2.0.8\nv2.0.9\nv2.1.0\nv2.1.0-rc1\nlatest' bash "$S" "$@"; }
fails() { ! "$@"; }
says() { local want="$1"; shift; { "$@" || true; } | grep -qF -- "$want"; }
quiet() { local not="$1"; shift; ! { "$@" || true; } | grep -qF -- "$not"; }

yml v2.0.7 v2.0.8 v2.0.9 v2.1.0
check "rows up to the newest release pass" run
yml v2.0.7 v2.0.8
check "a missing newest release fails" fails run
check "the failure names the row to add" says "Add '- from: v2.1.0'" run
yml v2.0.7 v2.0.8 v2.0.9
check "a release of 2.2.0 without a 2.1.0 row fails" fails run v2.2.0
check "a release of 2.1.0 with a 2.0.9 row passes" run v2.1.0
yml v2.0.7 v2.1.0 v2.1.1@origin/release/2.1.1
check "an unpublished row built from a branch passes" run
yml v2.0.7 v2.1.0 v2.1.1
check "an unpublished row with no from_ref fails" fails run
check "a row in another job does not count" quiet v9.9.9 run
yml v2.0.7 v2.0.8 v2.0.9 v2.1.0
awk '/- from: v2.0.8$/ { print "          # - from: v2.0.6" } { print }' "$T/ci.yml" > "$T/c" && mv "$T/c" "$T/ci.yml"
check "a commented-out row is ignored" run
yml v2.0.7 v2.0.10
check "versions sort as numbers (v2.0.10 is newer than v2.0.9)" env CI_YML="$T/ci.yml" UPDATE_PATH_TAGS=$'v2.0.9\nv2.0.10\nv2.0.7' bash "$S"
echo "update-path-rows: $PASS passed, $FAIL failed"; [ "$FAIL" -eq 0 ]

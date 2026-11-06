#!/usr/bin/env bash
# scripts/ci/check-migrations.sh: passes on this checkout, and fails on each numbering mistake.
# Runs on a copy of backend/migrations and backend/src/migrate.rs in a temp dir. No Docker needed.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"; ROOT="$HERE/../.."; S="$ROOT/scripts/ci/check-migrations.sh"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
PASS=0; FAIL=0; check() { local d="$1"; shift; if "$@" >/dev/null 2>&1; then PASS=$((PASS+1)); else FAIL=$((FAIL+1)); echo "FAIL: $d"; fi; }
fresh() { rm -rf "$T/r"; mkdir -p "$T/r/backend/src"; cp -r "$ROOT/backend/migrations" "$T/r/backend/"; cp "$ROOT/backend/src/migrate.rs" "$T/r/backend/src/"; }
refused() { # refused DESCRIPTION WORDS : the check fails and its output contains WORDS
  check "$1: refused" bash -c "! bash '$S' '$T/r'"
  check "$1: says '$2'" bash -c "bash '$S' '$T/r' | grep -q '$2'"
}
M="$T/r/backend/migrations"; RS="$T/r/backend/src/migrate.rs"; Q="$T/r/backend/migrations/QUEUE.md"
last="$(find "$ROOT/backend/migrations/tenant" -name '[0-9]*.surql' | sed 's#.*/0*\([0-9]*\)_.*#\1#' | sort -n | tail -1)"
next="$(printf %04d $((last + 1)))"

check "this checkout passes" bash "$S" "$ROOT"
fresh; check "a copy passes" bash "$S" "$T/r"

fresh; echo "SELECT 1;" > "$M/tenant/${next}_stray.surql"
refused "an unregistered tenant file" "not registered in MIGRATIONS"

fresh; echo "SELECT 1;" > "$M/control/0099_far.surql"
refused "a gap in the control files" "no file numbered 0007"

fresh; echo "SELECT 1;" > "$M/tenant/0021_again.surql"
refused "two tenant files with one number" "two files numbered 0021"

fresh; sed -i.bak 's#(21, "skills",#(21, "skillz",#' "$RS"
refused "an entry whose name is not its file's" "includes tenant/0021_skills.surql, expected tenant/0021_skillz.surql"

fresh; sed -i.bak 's#(20, "extensions",#(19, "extensions",#' "$RS"
refused "an entry out of order" "MIGRATIONS entry 20 is numbered 19"

fresh; rm "$M/control/0006_oidc_identity.surql"
refused "an entry whose file is gone" "does not exist"

# the queue: a reserved number must not land before it is locked, and under its reserved name
fresh; echo "SELECT 1;" > "$M/tenant/${next}_early.surql"
awk -v n="$((10#$next))" -v f="${next}_early.surql" '/^pub const MIGRATIONS/ { on = 1 } on && /^\];/ { print "    (" n ", \"early\", include_str!(\"../migrations/tenant/" f "\"))," ; on = 0 } { print }' "$ROOT/backend/src/migrate.rs" > "$RS"
sed -i.bak "/^| tenant | $next |/d" "$Q"   # the real queue may reserve $next; this case is about the file alone
check "a registered next file not in the queue passes" bash "$S" "$T/r"
printf '| tenant | %s | early | L9 | #999 | reserved |\n' "$next" >> "$Q"
refused "a file for a reserved (not locked) number" "take the lock first"
sed -i.bak "s#| tenant | $next | early | L9 | \#999 | reserved |#| tenant | $next | other | L9 | \#999 | locked |#" "$Q"
refused "a file under another name than the queue's" "reserves $next for other"
sed -i.bak "s#| tenant | $next | other | L9 | \#999 | locked |#| tenant | $next | early | L9 | \#999 | locked |#" "$Q"
check "the locked number under its name passes" bash "$S" "$T/r"

fresh; printf '| tenant | 0023 | twice | L9 | #999 | reserved |\n' >> "$Q"
refused "a number listed twice in the queue" "lists a number twice"
fresh; printf '| tenant | 0030 | odd | L9 | #999 | maybe |\n' >> "$Q"
refused "an unknown queue state" "has state .maybe."

echo "check-migrations: $PASS passed, $FAIL failed"; [ "$FAIL" -eq 0 ]

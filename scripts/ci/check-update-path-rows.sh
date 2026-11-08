#!/usr/bin/env bash
# The update test (CI job `update-path`) must start from the newest release, because that is the
# update people run next. 2.0.9 and 2.1.0 shipped with no row for them, so neither update was tested.
#   bash scripts/ci/check-update-path-rows.sh            every PR: the newest published v* tag has a row
#   bash scripts/ci/check-update-path-rows.sh <tag>      a release: the newest tag below <tag> has a row
# Also refuses a row naming a tag that does not exist, unless the row builds it from a `from_ref`.
# Tags come from `git ls-remote --tags origin`, or from UPDATE_PATH_TAGS (one per line) in tests.
# CI_YML overrides the workflow file (tests).
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
YML="${CI_YML:-$ROOT/.github/workflows/ci.yml}"
RELEASE="${1:-}"
[ -f "$YML" ] || { echo "check-update-path-rows: no $YML"; exit 1; }

if [ -n "${UPDATE_PATH_TAGS+x}" ]; then
  tags="$UPDATE_PATH_TAGS"
else
  tags="$(git -C "$ROOT" ls-remote --tags --refs origin 'v*' | sed 's#.*refs/tags/##')" \
    || { echo "check-update-path-rows: cannot list the tags on origin"; exit 1; }
fi
tags="$(printf '%s\n' "$tags" | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | sort -V)"
[ -n "$tags" ] || { echo "check-update-path-rows: no release tags found"; exit 1; }

# "<tag> <from_ref or ->" for each matrix row of the update-path job
rows="$(awk '
  /^  [A-Za-z0-9_-]+:/ { job = $1 }
  job != "update-path:" { next }
  /^ *#/ { next }
  /- from: / { if (tag != "") print tag, ref; tag = $3; ref = "-"; next }
  /from_ref: / && tag != "" { ref = $2 }
  END { if (tag != "") print tag, ref }
' "$YML")"
[ -n "$rows" ] || { echo "check-update-path-rows: no update-path matrix rows in $YML"; exit 1; }

if [ -n "$RELEASE" ]; then
  need="$(printf '%s\n%s\n' "$tags" "$RELEASE" | sort -uV | grep -B1 -xF "$RELEASE" | head -1)"
  [ "$need" != "$RELEASE" ] || need=""
  [ -n "$need" ] || { echo "check-update-path-rows: no release before $RELEASE, nothing to require"; exit 0; }
else
  need="$(printf '%s\n' "$tags" | tail -1)"
fi

ERR=0
if ! printf '%s\n' "$rows" | awk '{ print $1 }' | grep -qxF "$need"; then
  echo "check-update-path-rows: the update test does not start from $need, the newest release."
  echo "  Add '- from: $need' to the update-path matrix in .github/workflows/ci.yml, and to the rows in docs/testing.md."
  ERR=1
fi
while read -r tag ref; do
  [ "$ref" = "-" ] || continue
  printf '%s\n' "$tags" | grep -qxF "$tag" \
    || { echo "check-update-path-rows: row '$tag' is not a release tag (add from_ref to build it from a branch)"; ERR=1; }
done <<< "$rows"
[ "$ERR" -eq 0 ] && echo "check-update-path-rows: ok, the update test starts from $need"
exit "$ERR"

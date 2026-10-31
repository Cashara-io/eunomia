#!/usr/bin/env bash
# Migration numbering check (backend/migrations/QUEUE.md, docs/architecture/next-release-plan.md 3.2).
#   bash scripts/ci/check-migrations.sh [repo-root]
# For each set (tenant: MIGRATIONS, control: CONTROL_MIGRATIONS in backend/src/migrate.rs):
#   - the files under backend/migrations/<set>/ are numbered 0001.. contiguously, one file per number;
#   - every file is registered once, under its own number and name, in file order, and every entry
#     points at a file that exists;
#   - a file whose number QUEUE.md lists is `locked` or `merged` there, under the same name (a lane
#     must not land a number it only has `reserved`), and QUEUE.md lists each number once.
# Not checked: that every file not yet on main is the `locked` one (that needs main's history).
set -uo pipefail
ROOT="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"
RS="$ROOT/backend/src/migrate.rs"; Q="$ROOT/backend/migrations/QUEUE.md"
ERR=0
err() { echo "check-migrations: $*"; ERR=1; }
[ -f "$RS" ] || { err "no $RS"; exit 1; }
[ -f "$Q" ] || { err "no $Q"; exit 1; }

# QUEUE.md rows: "set number name state"
queue="$(awk -F'|' 'NF >= 8 { for (i = 2; i <= 7; i++) gsub(/^ +| +$/, "", $i); if ($2 == "tenant" || $2 == "control") print $2, $3, $4, $7 }' "$Q")"
dups="$(printf '%s\n' "$queue" | awk 'NF { print $1, $2 }' | sort | uniq -d)"
[ -z "$dups" ] || err "QUEUE.md lists a number twice: $(echo "$dups" | tr '\n' ' ')"
while read -r set num name state; do
  [ -n "${set:-}" ] || continue
  case "$state" in reserved|locked|merged) ;; *) err "QUEUE.md: $set $num has state '$state' (reserved, locked or merged)" ;; esac
  [[ "$num" =~ ^[0-9]{4}$ ]] || err "QUEUE.md: $set number '$num' is not four digits"
done <<< "$queue"

for set in tenant control; do
  const=MIGRATIONS; [ "$set" = control ] && const=CONTROL_MIGRATIONS
  dir="$ROOT/backend/migrations/$set"
  # files: "number name", in number order
  files="$(find "$dir" -maxdepth 1 -name '*.surql' -exec basename {} \; | sort | sed -n 's#^\([0-9]\{4\}\)_\(.*\)\.surql$#\1 \2#p')"
  bad="$(find "$dir" -maxdepth 1 -name '*.surql' -exec basename {} \; | grep -vE '^[0-9]{4}_[a-z0-9_]+\.surql$')"
  [ -z "$bad" ] || err "$set: file names not NNNN_name.surql: $(echo "$bad" | tr '\n' ' ')"
  # registered entries: "number name path", in list order
  entries="$(awk -v c="$const" '$0 ~ "^pub const " c ": " { on = 1; next } on && /^\];/ { on = 0 } on' "$RS" \
    | sed -n 's#^ *(\([0-9]*\), *"\([^"]*\)", *include_str!("\.\./migrations/\([^"]*\)")),.*#\1 \2 \3#p')"
  [ -n "$entries" ] || { err "$set: no entries found in $const"; continue; }

  # files: contiguous from 1, unique
  want=1
  while read -r num name; do
    [ -n "${num:-}" ] || continue
    n=$((10#$num))
    if [ "$n" -lt "$want" ]; then err "$set: two files numbered $num"; continue; fi
    [ "$n" -eq "$want" ] || err "$set: no file numbered $(printf %04d "$want") (next is ${num}_$name)"
    want=$((n + 1))
  done <<< "$files"

  # entries: in order 1..N, each pointing at its own file
  want=1
  while read -r n name path; do
    [ "$n" -eq "$want" ] || err "$set: $const entry $want is numbered $n"
    want=$((n + 1))
    expect="$set/$(printf %04d "$n")_$name.surql"
    [ "$path" = "$expect" ] || err "$set: $const entry $n ($name) includes $path, expected $expect"
    [ -f "$ROOT/backend/migrations/$path" ] || err "$set: $const entry $n includes $path, which does not exist"
  done <<< "$entries"

  # every file registered
  while read -r num name; do
    [ -n "${num:-}" ] || continue
    printf '%s\n' "$entries" | awk -v n="$((10#$num))" -v m="$name" '$1 == n && $2 == m { f = 1 } END { exit !f }' \
      || err "$set: ${num}_$name.surql is not registered in $const"
  done <<< "$files"

  # a file whose number the queue lists is locked or merged there, under its name
  while read -r qset qnum qname qstate; do
    [ "${qset:-}" = "$set" ] || continue
    fname="$(printf '%s\n' "$files" | awk -v n="$qnum" '$1 == n { print $2 }')"
    [ -n "$fname" ] || continue
    [ "$fname" = "$qname" ] || err "$set: file ${qnum}_$fname.surql, but QUEUE.md reserves $qnum for $qname"
    case "$qstate" in locked|merged) ;; *) err "$set: ${qnum}_$fname.surql exists but QUEUE.md has $qnum as $qstate (take the lock first)" ;; esac
  done <<< "$queue"
done

[ "$ERR" -eq 0 ] && echo "check-migrations: ok"
exit "$ERR"

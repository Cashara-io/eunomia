#!/usr/bin/env bash
# Real export sections: schema objects must never count as rows of the preceding table.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
sed '/^case "${1:-}" in/,$d' "$HERE/../../backend/scripts/backup/eunomia-backup.sh" > "$TMP/functions.sh"
cat > "$TMP/export.surql" <<'SQL'
-- TABLE: empty
DEFINE TABLE empty TYPE NORMAL SCHEMAFULL PERMISSIONS NONE;
-- TABLE DATA: empty
-- TABLE: items
DEFINE TABLE items TYPE NORMAL SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD config ON items TYPE option<{ nested: { enabled: bool } }>;
-- TABLE DATA: items
INSERT [{ id: items:one, text: 'A real row with { braces }', nested: { value: 1 } }];
-- TABLE: links
DEFINE TABLE links TYPE RELATION SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD config ON links TYPE option<{ nested: { enabled: bool } }>;
-- TABLE DATA: links
INSERT RELATION [{ id: links:one, in: items:one, out: items:two }];
SQL
actual="$(sh -c '. "$1"; dump_counts "$2"' sh "$TMP/functions.sh" "$TMP/export.surql")"
expected="$(printf 'empty 0\nitems 1\nlinks 1')"
if [ "$actual" != "$expected" ]; then
  printf 'FAIL: export counts must exclude schemas\nExpected:\n%s\nActual:\n%s\n' "$expected" "$actual"
  exit 1
fi
printf 'PASS: empty tables, following schemas, nested row values and relation rows\n'

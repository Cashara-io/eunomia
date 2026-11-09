#!/usr/bin/env bash
# Real marker collector, with bounded Docker fixtures. No running stack required.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/bin" "$TMP/status"
cat > "$TMP/bin/docker" <<'MOCK'
#!/bin/sh
case "$*" in
  'compose ps --all --format json') printf '%s\n' '{"Service":"backend","State":"running","Image":"ghcr.io/cashara-io/eunomia-backend:v2.2.1","Name":"private@example.com"}' ;;
  'compose exec -T surrealdb df -Pk /data') printf 'Filesystem 1024-blocks Used Available Capacity Mounted\nvolume 10000 100 9900 1%% /data\n' ;;
  *) printf '%s\n' '{"timestamp":"2026-11-09T01:00:00Z","level":"WARN","target":"eunomia_backend::sources","line":123,"fields":{"count":5,"error":"password=CANARY_SECRET private@example.com MEMORY_CANARY DOCUMENT_CANARY CHAT_CANARY"}}' 'Bearer CANARY_TOKEN eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJjYW5hcnkifQ.signaturecanary' '-----BEGIN PRIVATE KEY-----' 'CANARY_PEM_BODY' ;;
esac
MOCK
chmod +x "$TMP/bin/docker"
printf '0123456789abcdef0123456789abcdef' > "$TMP/status/diagnostics-requested"
PATH="$TMP/bin:$PATH" python3 "$HERE/../diagnostics.py" "$TMP/status"
python3 - "$TMP/status" <<'PY'
import json, pathlib, sys
root = pathlib.Path(sys.argv[1])
assert not (root / 'diagnostics-requested').exists()
assert (root / 'diagnostics/ready').read_text() == '0123456789abcdef0123456789abcdef'
text = '\n'.join(p.read_text() for p in (root / 'diagnostics').iterdir())
assert all(x not in text for x in ['CANARY', 'private@example.com', 'Bearer', 'PRIVATE KEY', 'eyJhbG'])
assert 'WARN' in text and 'count' in text
ps = json.loads((root / 'diagnostics/compose-ps.json').read_text())
assert ps[0]['service'] == 'backend' and ps[0]['state'] == 'running'
assert json.loads((root / 'diagnostics/collection.json').read_text())['data_volume_free_bytes'] == 9900 * 1024
PY
# A 2.2.0 updater ignores the marker. The backend timeout test proves the actual graceful note.
printf '0123456789abcdef0123456789abcdef' > "$TMP/status/diagnostics-requested"
printf '{"current_version":"v2.2.0"}' > "$TMP/status/status.json"
mkdir -p "$TMP/legacy"
mv "$TMP/status" "$TMP/legacy/update-status"
git -C "$HERE/../.." show v2.2.0:scripts/auto-update.sh > "$TMP/legacy-updater.sh"
git -C "$TMP/legacy" init -q
printf 'EUNOMIA_IMAGE_TAG=v2.2.0\nSECRETS_HELPER_KEY=fixture-only\n' > "$TMP/legacy/.env"
EUNOMIA_DIR="$TMP/legacy" PATH="$TMP/bin:$PATH" bash "$TMP/legacy-updater.sh" >/dev/null 2>&1
test -f "$TMP/legacy/update-status/diagnostics-requested"
# A privileged collector must never read through a marker symlink.
mkdir -p "$TMP/symlink-status"
printf 'abcdef0123456789abcdef0123456789' > "$TMP/private-fixture"
ln -s "$TMP/private-fixture" "$TMP/symlink-status/diagnostics-requested"
PATH="$TMP/bin:$PATH" python3 "$HERE/../diagnostics.py" "$TMP/symlink-status"
test ! -e "$TMP/symlink-status/diagnostics/ready"
test -f "$TMP/private-fixture"
mkdir -p "$TMP/fifo-status"
mkfifo "$TMP/fifo-status/diagnostics-requested"
PATH="$TMP/bin:$PATH" python3 "$HERE/../diagnostics.py" "$TMP/fifo-status"
test ! -e "$TMP/fifo-status/diagnostics/ready"
# Collector must run before auto-update's recent-check early exit, and be installed with the release.
python3 - "$HERE/../auto-update.sh" <<'PY'
from pathlib import Path
import sys
s = Path(sys.argv[1]).read_text()
assert s.index('python3 scripts/diagnostics.py') < s.index('# Nothing requested and checked recently')
PY
python3 - "$HERE/../diagnostics.py" <<'PYTEST'
import importlib.util, sys
spec = importlib.util.spec_from_file_location('diagnostics', sys.argv[1])
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)
for text in ['[]', 'null', '{"fields":null}', '{"fields":[],"level":[]}', '{"fields":{"count":"MEMORY_CANARY"}}']:
    assert 'CANARY' not in m.safe_logs(text)
PYTEST
printf 'PASS: diagnostics marker, privacy canaries, metadata, disk count and throttle ordering\n'

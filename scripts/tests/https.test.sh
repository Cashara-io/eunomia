#!/usr/bin/env bash
# Tests Settings, HTTPS as applied by scripts/auto-update.sh: `docker` and
# `curl` shims record what would have run; update-status/https.json is what
# POST /api/https writes.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PASS=0; FAIL=0
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
check() { local d="$1"; shift; if "$@" >/dev/null 2>&1; then PASS=$((PASS + 1)); else FAIL=$((FAIL + 1)); echo "FAIL: $d"; fi; }
S="$TMP/repo/update-status"
field() { python3 -c 'import json,sys;print(str(json.load(open(sys.argv[1]))[sys.argv[2]]).lower())' "$S/https-status.json" "$1"; }

mkdir -p "$TMP/repo/scripts" "$S" "$TMP/bin"
cp "$HERE/../auto-update.sh" "$TMP/repo/scripts/"
printf 'JWT_SECRET=x\nEUNOMIA_IMAGE_TAG=v1.0.0\n' > "$TMP/repo/.env"
# A fresh release check, so each run only does the HTTPS part.
echo '{}' > "$S/status.json"
printf '#!/bin/sh\necho "$@" >> "%s/docker.log"\n' "$TMP" > "$TMP/bin/docker"
printf '#!/bin/sh\necho "$@" >> "%s/curl.log"\nexit $(cat "%s/curl.rc")\n' "$TMP" "$TMP" > "$TMP/bin/curl"
chmod +x "$TMP/bin/docker" "$TMP/bin/curl"
run() { EUNOMIA_DIR="$TMP/repo" PATH="$TMP/bin:$PATH" bash "$TMP/repo/scripts/auto-update.sh"; }
request() { printf '{\n  "enabled": %s,\n  "domain": "%s",\n  "email": "%s"\n}\n' "$1" "$2" "$3" > "$S/https.json"; }
env_count() { grep -c "^$1" "$TMP/repo/.env"; }

# 1. a valid request: .env updated, caddy started, pending until the cert answers
echo 7 > "$TMP/curl.rc" # connection refused
request true eunomia.example.com me@example.com; run
check "domain written to .env" grep -qx 'EUNOMIA_DOMAIN=eunomia.example.com' "$TMP/repo/.env"
check "email written to .env" grep -qx 'EUNOMIA_ACME_EMAIL=me@example.com' "$TMP/repo/.env"
check "https profile enabled" grep -qx 'COMPOSE_PROFILES=https' "$TMP/repo/.env"
check ".env keeps its other settings" grep -qx 'JWT_SECRET=x' "$TMP/repo/.env"
# Caddy proxies to the frontend: the frontend must believe its headers, and only Caddy may reach it
check "frontend trusts the forwarded headers" grep -qx 'FRONTEND_TRUST_FORWARDED=1' "$TMP/repo/.env"
check "frontend port closed to the network" grep -qx 'FRONTEND_BIND=127.0.0.1' "$TMP/repo/.env"
check "PUBLIC_URL points at the domain" grep -qx 'PUBLIC_URL=https://eunomia.example.com' "$TMP/repo/.env"
check "the backend's trusted proxies are left alone" bash -c "! grep -q '^TRUSTED_PROXIES=' '$TMP/repo/.env'"
check "caddy started with the frontend and backend recreated for the new env" grep -qx 'compose up -d backend frontend caddy' "$TMP/docker.log"
check "request consumed" test ! -e "$S/https.json"
check "no certificate yet: pending" test "$(field state)" = pending
check "pending says what to check" grep -q 'points at this machine' "$S/https-status.json"
check "status names the domain" test "$(field domain)" = eunomia.example.com
check "probed the domain through caddy's port" grep -q 'connect-to eunomia.example.com:443:127.0.0.1:443 https://eunomia.example.com/' "$TMP/curl.log"
check "lock released" test ! -e "$S/.lock"

# 2. the certificate answers: active, and no more probing after that
echo 0 > "$TMP/curl.rc"; run
check "certificate answers: active" test "$(field state)" = active
check "active: message cleared" test "$(field message)" = none
: > "$TMP/curl.log"; run
check "active: not probed again" test ! -s "$TMP/curl.log"

# 3. re-applying the same request is idempotent
: > "$TMP/docker.log"; request true eunomia.example.com me@example.com; run
check "re-apply: one domain line" test "$(env_count EUNOMIA_DOMAIN=)" = 1
check "re-apply: one email line" test "$(env_count EUNOMIA_ACME_EMAIL=)" = 1
check "re-apply: one profile line" test "$(env_count COMPOSE_PROFILES=)" = 1
check "re-apply: one trust line" test "$(env_count FRONTEND_TRUST_FORWARDED=)" = 1
check "re-apply: one bind line" test "$(env_count FRONTEND_BIND=)" = 1
check "re-apply: one public url line" test "$(env_count PUBLIC_URL=)" = 1
check "re-apply: caddy up again (a no-op for compose)" grep -qx 'compose up -d backend frontend caddy' "$TMP/docker.log"
check "re-apply: active again" test "$(field state)" = active

# 4. invalid input is refused and changes nothing
for bad in 'evil.com;rm -rf /' 'x$(id).com' '-lead.example.com' '127.0.0.1' 'localhost' 'a#b.com' 'a&b.com'; do
  cp "$TMP/repo/.env" "$TMP/env.before"; : > "$TMP/docker.log"
  request true "$bad" me@example.com; run
  check "refused domain '$bad': .env unchanged" cmp -s "$TMP/env.before" "$TMP/repo/.env"
  check "refused domain '$bad': docker not run" test ! -s "$TMP/docker.log"
  check "refused domain '$bad': error status" test "$(field state)" = error
done
for bad in 'not-an-email' 'me@localhost' 'me@x.com;reboot' 'a#b@x.com' 'a&b@x.com' 'me @x.com'; do
  cp "$TMP/repo/.env" "$TMP/env.before"; : > "$TMP/docker.log"
  request true eunomia.example.com "$bad"; run
  check "refused email '$bad': .env unchanged" cmp -s "$TMP/env.before" "$TMP/repo/.env"
  check "refused email '$bad': docker not run" test ! -s "$TMP/docker.log"
  check "refused email '$bad': error status" test "$(field state)" = error
done
check "refusal says why" grep -q 'invalid domain or email' "$S/https-status.json"

# 5. disable: profile removed, caddy stopped and removed
: > "$TMP/docker.log"; : > "$TMP/curl.log"; request false "" ""; run
check "disable: profile removed" bash -c "! grep -q '^COMPOSE_PROFILES=' '$TMP/repo/.env'"
check "disable: caddy stopped" grep -qx 'compose rm -sf caddy' "$TMP/docker.log"
check "disable: frontend and backend recreated without the trust env" grep -qx 'compose up -d backend frontend' "$TMP/docker.log"
check "disable: frontend stops trusting forwarded headers" bash -c "! grep -q '^FRONTEND_TRUST_FORWARDED=' '$TMP/repo/.env'"
check "disable: frontend port reopened" bash -c "! grep -q '^FRONTEND_BIND=' '$TMP/repo/.env'"
check "disable: PUBLIC_URL back to the default" bash -c "! grep -q '^PUBLIC_URL=' '$TMP/repo/.env'"
check "disable: domain and email kept for re-enabling" grep -qx 'EUNOMIA_DOMAIN=eunomia.example.com' "$TMP/repo/.env"
check "disable: status off" test "$(field state)" = off
run
check "disabled: never probed" test ! -s "$TMP/curl.log"

# 5b. a PUBLIC_URL that was not set by HTTPS is left alone on disable
echo 'PUBLIC_URL=https://other.example.org' >> "$TMP/repo/.env"; request false "" ""; run
check "disable: foreign PUBLIC_URL kept" grep -qx 'PUBLIC_URL=https://other.example.org' "$TMP/repo/.env"
sed -i.bak '/^PUBLIC_URL=/d' "$TMP/repo/.env" && rm -f "$TMP/repo/.env.bak"

# 6. enabled by the installer (no status yet): probed on the first run
rm -f "$S/https-status.json"; echo 'COMPOSE_PROFILES=https' >> "$TMP/repo/.env"; run
check "installer-enabled: probed and active" test "$(field state)" = active

# 7. the password managers' profile (`secrets`) is kept when HTTPS is switched on and off
sed -i.bak '/^COMPOSE_PROFILES=/d' "$TMP/repo/.env" && rm -f "$TMP/repo/.env.bak"; echo 'COMPOSE_PROFILES=secrets' >> "$TMP/repo/.env"
request true eunomia.example.com me@example.com; run
check "enable with secrets on: both profiles" grep -qx 'COMPOSE_PROFILES=secrets,https' "$TMP/repo/.env"
request false "" ""; run
check "disable with secrets on: secrets kept" grep -qx 'COMPOSE_PROFILES=secrets' "$TMP/repo/.env"

echo "https: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]

#!/usr/bin/env bash
# The backend and the real secrets-helper container on the real compose networks (CI job
# secrets-helper-compose; needs Docker with container-to-container networking):
#   - "Test connection" goes backend -> secrets-helper -> a mock 1Password Connect container and back,
#     with the right token (ok) and a wrong one (denied);
#   - the sidecar publishes no port, is on the secrets and secrets-egress networks only, answers the
#     backend, refuses a call without the internal key, and cannot be reached from the default network;
#   - after the calls nothing was written to its filesystem (docker diff), and the token is in neither
#     its environment nor its logs.
# Own compose project ($P), backend on 127.0.0.1:8261. SECRETS_COMPOSE_KEEP=1 leaves it up.
set -uo pipefail
P="${SECRETS_COMPOSE_PROJECT:-eu-secrets}"
BPORT="${SECRETS_COMPOSE_BACKEND_PORT:-8261}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
W="$(mktemp -d "${TMPDIR:-/tmp}/eu-secrets.XXXXXX")"
TOKEN="connect-token-compose-$(openssl rand -hex 8)"
NODE_IMG="node:24.22.2-alpine3.24"
PASS=0; FAIL=0
log() { printf '[secrets-compose] %s\n' "$*"; }
check() { local d="$1"; shift; if "$@" >/dev/null 2>&1; then PASS=$((PASS + 1)); log "ok: $d"; else FAIL=$((FAIL + 1)); log "FAIL: $d"; fi; }
die() { log "FATAL: $*"; FAIL=$((FAIL + 1)); exit 1; }
for c in docker curl jq openssl; do command -v "$c" >/dev/null || { echo "needs $c" >&2; exit 2; }; done
dc() { (cd "$ROOT" && docker compose -p "$P" --env-file "$W/.env" "$@"); }
cleanup() {
  local rc=$?
  { [ "$FAIL" -gt 0 ] || [ "$rc" -ne 0 ]; } && { dc ps -a; dc logs --no-color --tail 80 backend secrets-helper; docker logs "$P-mock-connect" 2>&1 | tail -20; } 2>/dev/null
  if [ "${SECRETS_COMPOSE_KEEP:-}" != 1 ]; then
    docker rm -f "$P-mock-connect" >/dev/null 2>&1
    dc down -v --remove-orphans >/dev/null 2>&1
    rm -rf "$W"
  fi
  log "$PASS passed, $FAIL failed"
}
trap cleanup EXIT

cat > "$W/.env" <<ENV
JWT_SECRET=$(openssl rand -hex 32)
ENCRYPTION_KEY=$(openssl rand -hex 32)
SECRETS_HELPER_KEY=$(openssl rand -hex 32)
BACKEND_PORT=$BPORT
EMBEDDINGS_BACKEND=stub
RATE_LIMIT_AUTH_PER_MIN=1000
EUNOMIA_IMAGE_TAG=secrets-ci
ENV

log "building backend and secrets-helper"
dc build backend secrets-helper > "$W/build.txt" 2>&1 || { tail -40 "$W/build.txt"; die "build failed"; }
dc up -d --wait surrealdb backend secrets-helper > "$W/up.txt" 2>&1 || { cat "$W/up.txt"; die "the stack did not come up healthy"; }
# the mock provider on the sidecar's egress network, where a customer's Connect server would be reachable
docker run -d --name "$P-mock-connect" --network "${P}_secrets-egress" --network-alias mock-connect -e MOCK_CONNECT_TOKEN="$TOKEN" \
  -v "$ROOT/scripts/tests/fixtures/mock-connect.mjs:/mock.mjs:ro" "$NODE_IMG" node /mock.mjs >/dev/null || die "mock connect did not start"

SH="$(dc ps -q secrets-helper)"
check "secrets-helper is healthy" bash -c "[ \"\$(docker inspect -f '{{.State.Health.Status}}' $SH)\" = healthy ]"
check "secrets-helper publishes no port" bash -c "[ -z \"\$(docker port $SH)\" ]"
check "secrets-helper is on the secrets networks only" bash -c "[ \"\$(docker inspect -f '{{range \$k, \$v := .NetworkSettings.Networks}}{{\$k}} {{end}}' $SH | tr ' ' '\n' | grep . | sort | tr '\n' ' ')\" = '${P}_secrets ${P}_secrets-egress ' ]"
check "secrets-helper runs on a read-only root filesystem as a non-root user" bash -c "[ \"\$(docker inspect -f '{{.HostConfig.ReadonlyRootfs}} {{.Config.User}}' $SH)\" = 'true node' ]"
check "the backend reaches it" dc exec -T backend curl -fsS --max-time 5 http://secrets-helper:8080/healthz
check "a call without the internal key is refused" bash -c "[ \"\$(cd '$ROOT' && docker compose -p '$P' --env-file '$W/.env' exec -T backend curl -s -o /dev/null -w '%{http_code}' -X POST -H 'content-type: application/json' -d '{}' http://secrets-helper:8080/v1/check)\" = 401 ]"
check "the default network cannot reach it" bash -c "! docker run --rm --network '${P}_default' '$NODE_IMG' node -e \"fetch('http://secrets-helper:8080/healthz').then(() => process.exit(0), () => process.exit(1))\""

# an admin configures 1Password Connect and tests the connection, through the API as the browser does
B="http://127.0.0.1:$BPORT"; JAR="$W/jar"
curl -fsS -c "$JAR" -H 'content-type: application/json' -d '{"email":"admin@example.com","password":"correct-horse-battery-staple"}' "$B/api/auth/register" >/dev/null || die "signup failed"
put() { curl -sS -b "$JAR" -X PUT -H 'content-type: application/json' -d "{\"host\":\"http://mock-connect:8080\",\"token\":\"$1\"}" "$B/api/secrets/providers/onepassword_connect"; }
put "$TOKEN" > "$W/put.json"
check "the provider is saved" jq -e '.ok == true' "$W/put.json"
check "the provider list says configured and holds no token" bash -c "curl -fsS -b '$JAR' '$B/api/secrets/providers' | tee '$W/list.json' | jq -e '.[0].configured == true' && ! grep -q '$TOKEN' '$W/list.json'"
curl -sS -b "$JAR" -X POST "$B/api/secrets/providers/onepassword_connect/test" > "$W/test-ok.json"
check "Test connection through the sidecar to the mock Connect: ok" jq -e '.ok == true and .detail == "2 vaults visible"' "$W/test-ok.json"
put "connect-token-wrong-$(openssl rand -hex 4)" >/dev/null
curl -sS -b "$JAR" -X POST "$B/api/secrets/providers/onepassword_connect/test" > "$W/test-bad.json"
check "a wrong token: denied" jq -e '.ok == false and .error == "denied"' "$W/test-bad.json"
check "the error carries no token" bash -c "! grep -q connect-token '$W/test-bad.json'"

check "nothing was written to the sidecar's filesystem" bash -c "[ -z \"\$(docker diff $SH)\" ]"
check "the token is not in the sidecar's environment" bash -c "! docker exec $SH env | grep -q '$TOKEN'"
check "the token is not in the sidecar's or the backend's logs" bash -c "! (cd '$ROOT' && docker compose -p '$P' --env-file '$W/.env' logs --no-color backend secrets-helper) | grep -q '$TOKEN'"
[ "$FAIL" -eq 0 ]

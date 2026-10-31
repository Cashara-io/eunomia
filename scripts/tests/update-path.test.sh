#!/usr/bin/env bash
# The real update path: an install on a released tag takes THIS checkout's build through the old
# release's own updater, with data intact and every service healthy, then rolls back.
#   bash scripts/tests/update-path.test.sh [from-tag]        (default v2.0.7; needs Docker and the tag)
# Steps (docs/testing.md, "Update path"):
#   1. build this checkout's images and tag them $FAKE, a local-only tag that sorts above every
#      release and is never pushed;
#   2. a stand-in GitHub: a local bare repo with the real tags up to <from-tag> plus $FAKE on HEAD;
#   3. install <from-tag> from it with the PUBLISHED ghcr images (`docker compose pull`);
#   4. seed real-shaped data through the API (two users in two orgs, vaults, entities of several
#      kinds, facts, a superseded fact, a deleted fact, relations, skills, a document, a PAT);
#   5. snapshot per-database, per-table row counts, the migration ledgers and a few reads;
#   6. `backup now pre-update`;
#   7. "Update now" as the admin, then the OLD checkout's scripts/auto-update.sh, for real except
#      that a PATH shim turns `docker compose pull` into a no-op ($FAKE exists only locally);
#   8. assert status.json, history.log, every service healthy, the new SurrealDB pin running,
#      ledgers current, counts and reads unchanged, tenant 0022's rebuilt vector indexes present
#      and answering kNN;
#   9. run the updater again, restart the backend, `up -d` again: nothing changes;
#  10. rollback: the old tag on a fresh volume, restore the pre-update backup, counts and reads again.
# Own compose project ($P), ports 8251 (backend) and 3251 (frontend). Artifacts (logs, snapshots) in
# $ROOT/update-path-artifacts on failure. UPDATE_PATH_KEEP=1 leaves the stack and temp dir up.
set -uo pipefail
FROM="${1:-v2.0.7}"
FAKE="${UPDATE_PATH_FAKE_TAG:-v9.9.9-ci}"
P="${UPDATE_PATH_PROJECT:-eu-update}"
BPORT="${UPDATE_PATH_BACKEND_PORT:-8251}" FPORT="${UPDATE_PATH_FRONTEND_PORT:-3251}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
ART="$ROOT/update-path-artifacts"
W="$(mktemp -d "${TMPDIR:-/tmp}/eu-update-path.XYZXXXXXX")"
I="$W/install"
NET="${P}_default"
CURL_IMG="ghcr.io/cashara-io/eunomia-backend:$FROM"   # any image with curl on the compose network
PIN="$(sed -n 's#^ *image: surrealdb/surrealdb:\(v[0-9.]*\)$#\1#p' "$ROOT/docker-compose.yml" | head -1)"
LATEST_TENANT="$(find "$ROOT/backend/migrations/tenant" -name '[0-9]*.surql' | sed 's#.*/0*\([0-9]*\)_.*#\1#' | sort -n | tail -1)"
LATEST_CONTROL="$(find "$ROOT/backend/migrations/control" -name '[0-9]*.surql' | sed 's#.*/0*\([0-9]*\)_.*#\1#' | sort -n | tail -1)"
# Rows that move by themselves (sessions, jobs, audit and usage rows, and the embedding cache that
# recall in the reads below fills):
# compared for presence only, not for an exact count.
VOLATILE="${UPDATE_PATH_VOLATILE:-job job_leader audit_event audit_log tool_usage session failure_capsule _migration_lock rate_limit embed_cache}"
PASS=0; FAIL=0; T0=$SECONDS
log() { printf '[update-path %4ss] %s\n' "$((SECONDS - T0))" "$*"; }
check() { local d="$1"; shift; if "$@" >/dev/null 2>&1; then PASS=$((PASS + 1)); log "ok: $d"; else FAIL=$((FAIL + 1)); log "FAIL: $d"; fi; }
die() { log "FATAL: $*"; FAIL=$((FAIL + 1)); exit 1; }
need() { command -v "$1" >/dev/null || { echo "update-path: needs $1" >&2; exit 2; }; }
need docker; need git; need jq; need curl; need openssl
dc() { (cd "$I" && docker compose "$@"); }

dump() {
  mkdir -p "$ART"
  [ -d "$I" ] && dc ps -a > "$ART/ps.txt" 2>&1
  [ -d "$I" ] && dc logs --no-color --timestamps > "$ART/stack.log" 2>&1
  cp "$W"/*.json "$W"/*.txt "$ART/" 2>/dev/null
  cp -r "$I/update-status" "$ART/update-status" 2>/dev/null
  log "logs and snapshots in $ART"
}
cleanup() {
  local rc=$?
  { [ "$FAIL" -gt 0 ] || [ "$rc" -ne 0 ]; } && dump
  if [ "${UPDATE_PATH_KEEP:-}" = 1 ]; then log "kept: $I (project $P)"; else
    [ -d "$I" ] && dc --profile https down -v --remove-orphans >/dev/null 2>&1
    docker rm -f "$P-oldengine" >/dev/null 2>&1; docker volume rm "$P-oldengine-copy" >/dev/null 2>&1
    rm -rf "$W"
  fi
  log "update-path $FROM -> $FAKE: $PASS passed, $FAIL failed"
}
trap cleanup EXIT

# ---- helpers against the running install ------------------------------------------------------
sql_root() { # sql_root < query : as sql, with no namespace or database selected
  docker run --rm -i --network "$NET" --entrypoint curl "$CURL_IMG" -sS --max-time 60 -u root:root \
    -H 'Accept: application/json' --data-binary @- http://surrealdb:8000/sql
}
sql() { # sql DB < query : JSON array of statement results from SurrealDB's /sql, over the compose network
  docker run --rm -i --network "$NET" --entrypoint curl "$CURL_IMG" -sS --max-time 60 -u root:root \
    -H 'Accept: application/json' -H 'surreal-ns: eunomia' -H "surreal-db: $1" --data-binary @- http://surrealdb:8000/sql
}
org_dbs() { echo 'INFO FOR NS;' | sql control | jq -r '.[0].result.databases | keys[] | select(startswith("org_"))' | sort; }
counts() { # counts: "<db>.<table> <rows>" for control and every org database, sorted
  local db tables q
  for db in control $(org_dbs); do
    tables="$(echo 'INFO FOR DB;' | sql "$db" | jq -r '.[0].result.tables | keys[]')"
    q=""; for t in $tables; do q="$q SELECT count() FROM \`$t\` GROUP ALL;"; done
    [ -n "$q" ] || continue
    echo "$q" | sql "$db" | jq -r --arg db "$db" --argjson t "$(printf '%s\n' $tables | jq -R . | jq -s .)" \
      'to_entries[] | "\($db).\($t[.key]) \(.value.result[0].count // 0)"'
  done | sort
}
stable() { awk -v v=" $VOLATILE " '{ split($1, a, "."); if (index(v, " " a[2] " ") == 0) print }' "$1"; }
ledgers() { # "<db> <rows> <max version>" for every _migration ledger
  local db
  for db in control $(org_dbs); do
    echo 'SELECT count() AS n, math::max(version) AS v FROM _migration GROUP ALL;' | sql "$db" | jq -r --arg db "$db" '"\($db) \(.[0].result[0].n) \(.[0].result[0].v)"'
  done
}
api() { curl -sS --max-time 60 "$@"; }
mcp() { # mcp TOKEN TOOL ARGS-JSON : the tool's result object; fails on a protocol or tool error
  local out
  out="$(api -H "Authorization: Bearer $1" -H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream' \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"$2\",\"arguments\":$3}}" "$BASE/mcp")" || return 1
  out="$(printf '%s' "$out" | sed -n 's/^data: //p;/^{/p' | head -1)"
  printf '%s' "$out" | jq -e '(.result != null) and (.result.isError != true)' >/dev/null 2>&1 || { echo "mcp $2 failed: $out" >&2; return 1; }
  printf '%s' "$out" | jq -c '.result.content[0].text | fromjson'
}
wait_healthy() { # wait_healthy SECONDS : backend and surrealdb healthy, frontend and backup running
  local until=$((SECONDS + $1)) ps
  while [ $SECONDS -lt $until ]; do
    ps="$(dc ps -a --format '{{.Service}} {{.State}} {{.Health}}' 2>/dev/null)"
    if printf '%s\n' "$ps" | grep -q '^surrealdb running healthy' && printf '%s\n' "$ps" | grep -q '^backend running healthy' \
      && printf '%s\n' "$ps" | grep -q '^frontend running' && printf '%s\n' "$ps" | grep -q '^backup running' \
      && curl -fsS -o /dev/null --max-time 5 "$BASE/"; then return 0; fi
    sleep 3
  done
  log "not healthy after $1 s:"; printf '%s\n' "$ps"; return 1
}
wait_jobs() { # wait_jobs SECONDS : no job ready or running in the control queue
  local until=$((SECONDS + $1)) n
  while [ $SECONDS -lt $until ]; do
    n="$(echo "SELECT count() FROM job WHERE status IN ['ready', 'running'] GROUP ALL;" | sql control | jq -r '.[0].result[0].count // 0')"
    [ "$n" = 0 ] && return 0
    sleep 2
  done
  log "jobs still pending after $1 s"; return 1
}
wait_tenants() { # wait_tenants SECONDS : every org's tenant row and ledger at LATEST_TENANT
  local until=$((SECONDS + $1)) ok
  while [ $SECONDS -lt $until ]; do
    ok=1
    [ "$(echo 'SELECT VALUE schema_version FROM tenant;' | sql control | jq -r --argjson l "$LATEST_TENANT" '.[0].result | length > 0 and all(. == $l)')" = true ] || ok=0
    ledgers | awk -v t="$LATEST_TENANT" -v c="$LATEST_CONTROL" '($1 == "control" && $3 != c) || ($1 != "control" && $3 != t) { bad = 1 } END { exit bad }' || ok=0
    [ "$ok" = 1 ] && return 0
    sleep 3
  done
  log "tenants not at $LATEST_TENANT after $1 s"; ledgers; return 1
}
# Tenant 0022 rebuilt these HNSW indexes ("table:index").
VECTOR_INDEXES="memory:memory_embedding_idx skill:skill_embedding_idx cache_record:cache_record_embedding_idx"
ledger_has() { # ledger_has VERSION NAME : every org ledger has that migration
  local db
  for db in $(org_dbs); do
    [ "$(echo "SELECT VALUE name FROM _migration WHERE version = $1;" | sql "$db" | jq -r '.[0].result[0] // empty')" = "$2" ] || { log "   $db: no $1_$2 in the ledger"; return 1; }
  done
}
vector_indexes() { # vector_indexes : every org database defines the three HNSW indexes (INFO FOR TABLE)
  local db ti
  for db in $(org_dbs); do
    for ti in $VECTOR_INDEXES; do
      echo "INFO FOR TABLE ${ti%%:*};" | sql "$db" | jq -e --arg i "${ti#*:}" '.[0].result.indexes[$i] // "" | test("HNSW")' >/dev/null \
        || { log "   $db: no HNSW index ${ti#*:}"; return 1; }
    done
  done
}
wait_vector_builds() { # wait_vector_builds SECONDS : no background build of the three indexes still running
  local until=$((SECONDS + $1)) db ti busy
  while [ $SECONDS -lt $until ]; do
    busy=0
    for db in $(org_dbs); do
      for ti in $VECTOR_INDEXES; do
        [ "$(echo "INFO FOR INDEX ${ti#*:} ON ${ti%%:*};" | sql "$db" | jq -r '.[0].result.building.status // "ready"')" = ready ] || busy=1
      done
    done
    [ "$busy" = 0 ] && return 0
    sleep 2
  done
  log "vector index builds still running after $1 s"; return 1
}
knn_live() { # knn_live : in every org, a kNN query (<|K,EF|>) from an embedded row's own vector returns K rows, that row among them
  local db t n k out seen=0
  for db in $(org_dbs); do
    for t in memory skill; do
      n="$(echo "SELECT count() FROM $t WHERE embedding != NONE GROUP ALL;" | sql "$db" | jq -r '.[0].result[0].count // 0')"
      [ "$n" -gt 0 ] || continue
      k=$(( n < 10 ? n : 10 ))
      out="$(echo "LET \$r = (SELECT id, embedding FROM $t WHERE embedding != NONE ORDER BY id LIMIT 1)[0]; SELECT VALUE id FROM $t WHERE embedding <|$k,64|> \$r.embedding; RETURN \$r.id;" | sql "$db")"
      printf '%s' "$out" | jq -e --argjson k "$k" '.[2].result as $id | .[1].result | (length == $k) and any(. == $id)' >/dev/null \
        || { log "   $db.$t: kNN did not return $k rows including its own: $(printf '%s' "$out" | head -c 400)"; return 1; }
      seen=$((seen + 1))
    done
  done
  [ "$seen" -ge 2 ] || { log "   no embedded memory or skill to search"; return 1; }
}
reads() { # reads FILE : a fixed set of reads, reduced to what must not change (ids, counts, bodies)
  {
    echo "recall-a $(mcp "$TOKEN_A" recall '{"query":"algorithm"}' | jq -c '[.results[].id] | sort')"
    echo "recall-a-team $(mcp "$TOKEN_A" recall "{\"query\":\"compiler\",\"vault_id\":\"$TEAM\"}" | jq -c '[.results[].id] | sort')"
    echo "recall-b $(mcp "$TOKEN_B" recall '{"query":"Postgres"}' | jq -c '[.results[].id] | sort')"
    echo "entity-ada $(mcp "$TOKEN_A" entities_get "{\"id\":\"$ADA\"}" | jq -c '{m: ([.memory[].id] | sort), r: (.relations | length)}')"
    echo "recall-history $(mcp "$TOKEN_A" recall '{"query":"Ada Lovelace lives in London","include_history":true}' | jq -c '[.results[] | [.id, (.status // "")]] | sort')"
    echo "entity-ada-status $(mcp "$TOKEN_A" entities_get "{\"id\":\"$ADA\"}" | jq -c '[.memory[] | [.id, (.status // "")]] | sort')"
    echo "skill $(mcp "$TOKEN_A" skill_get "{\"id\":\"$SKILL\"}" | jq -c '{body, name}')"
    echo "skill-find $(mcp "$TOKEN_A" skill_find '{"query":"roll back a deploy"}' | jq -c '[.skills[].id] | sort')"
    echo "documents $(api -H "Authorization: Bearer $TOKEN_A" "$BASE/api/documents" | jq -c '[.results[].id] | sort')"
    echo "me-a $(api -H "Authorization: Bearer $TOKEN_A" "$BASE/api/auth/me" | jq -c '{email}')"
  } > "$1"
  ! grep -q ' $' "$1"   # every read returned something
}

BASE="http://127.0.0.1:$FPORT"
log "from $FROM to $FAKE (this checkout, SurrealDB $PIN, tenant $LATEST_TENANT, control $LATEST_CONTROL), project $P, temp $W"
git -C "$ROOT" cat-file -e "$FROM^{commit}" 2>/dev/null || die "tag $FROM is not in this clone (CI: actions/checkout with fetch-depth: 0)"
[ -z "$(docker ps -aq --filter "label=com.docker.compose.project=$P")" ] || die "compose project $P already has containers: docker compose -p $P down -v"

# ---- 1. candidate images ----------------------------------------------------------------------
if [ "${UPDATE_PATH_SKIP_BUILD:-}" = 1 ] && docker image inspect "ghcr.io/cashara-io/eunomia-backend:$FAKE" >/dev/null 2>&1; then
  log "1. reusing the $FAKE images (UPDATE_PATH_SKIP_BUILD=1)"
else
  log "1. building this checkout as $FAKE"
  (cd "$ROOT" && EUNOMIA_IMAGE_TAG="$FAKE" docker compose --env-file /dev/null -p "$P-build" build backend frontend backup) > "$W/build.txt" 2>&1 \
    || { tail -40 "$W/build.txt"; die "docker compose build failed"; }
fi
for s in backend frontend backup; do docker image inspect "ghcr.io/cashara-io/eunomia-$s:$FAKE" >/dev/null 2>&1 || die "no image eunomia-$s:$FAKE"; done

# ---- 2. stand-in GitHub -----------------------------------------------------------------------
log "2. local remote with the tags up to $FROM and $FAKE on HEAD"
git init -q --bare "$W/remote.git"
for t in $(git -C "$ROOT" for-each-ref --format='%(refname:short)' 'refs/tags/v*' | sort -V); do
  git -C "$W/remote.git" fetch -q "$ROOT" "refs/tags/$t:refs/tags/$t" || die "could not copy tag $t"
  [ "$t" = "$FROM" ] && break
done
git -C "$W/remote.git" fetch -q "$ROOT" HEAD && git -C "$W/remote.git" update-ref "refs/tags/$FAKE" FETCH_HEAD || die "could not add $FAKE"
[ "$(git -C "$W/remote.git" for-each-ref --format='%(refname:short)' 'refs/tags/v*' | sort -V | tail -1)" = "$FAKE" ] || die "$FAKE does not sort above $FROM"

# ---- 3. install the old release for real -------------------------------------------------------
log "3. installing $FROM with its published images"
git clone -q --branch "$FROM" "file://$W/remote.git" "$I" 2>/dev/null || die "clone of $FROM failed"
cat > "$I/.env" <<EOF
COMPOSE_PROJECT_NAME=$P
EUNOMIA_IMAGE_TAG=$FROM
JWT_SECRET=$(openssl rand -hex 32)
ENCRYPTION_KEY=$(openssl rand -hex 32)
BACKUP_ENCRYPTION_KEY=$(openssl rand -base64 32 | tr -d '\n')
BACKEND_PORT=$BPORT
FRONTEND_PORT=$FPORT
FRONTEND_BIND=127.0.0.1
EMBEDDINGS_BACKEND=stub
EUNOMIA_SIGNUP_ORG=personal
RATE_LIMIT_AUTH_PER_MIN=1000
EOF
mkdir -p "$I/update-status" && chmod 777 "$I/update-status"   # as the e2e job: the backend (uid 10001) writes here
dc pull -q > "$W/pull.txt" 2>&1 || { cat "$W/pull.txt"; die "pulling the published $FROM images failed"; }
# `up -d` gives up when the backend is not healthy within compose's wait; retry it like the
# updater's start_stack does (bounded), then wait for health ourselves.
for try in 1 2 3; do dc up -d >> "$W/up-old.txt" 2>&1 && break; log "   up -d try $try failed, retrying"; sleep 10; done
wait_healthy 300 || die "$FROM did not come up healthy"
check "$FROM runs its published backend image" bash -c "[ \"\$(docker inspect -f '{{.Config.Image}}' \"\$(cd '$I' && docker compose ps -q backend)\")\" = ghcr.io/cashara-io/eunomia-backend:$FROM ]"
OLDPIN="$(docker inspect -f '{{.Config.Image}}' "$(dc ps -q surrealdb)")"
log "   $FROM runs $OLDPIN"

# ---- 4. seed through the API -------------------------------------------------------------------
log "4. seeding"
JAR_A="$W/jar-a"; JAR_B="$W/jar-b"; PW="$(openssl rand -hex 12)"
EMAIL_A="admin-$(date +%s)@example.com"; EMAIL_B="other-$(date +%s)@example.com"
signup() { # signup JAR EMAIL : register (EUNOMIA_SIGNUP_ORG=personal: an org of their own) and onboard
  api -f -c "$1" -H 'Content-Type: application/json' -d "{\"email\":\"$2\",\"password\":\"$PW\"}" "$BASE/api/auth/register" >/dev/null \
    && api -f -b "$1" -X POST "$BASE/api/settings/complete-onboarding" >/dev/null
}
signup "$JAR_A" "$EMAIL_A" || die "signup of the first user (the instance admin) failed"
signup "$JAR_B" "$EMAIL_B" || die "signup of the second user failed"
TOKEN_A="$(api -f -b "$JAR_A" -H 'Content-Type: application/json' -d '{"name":"update-path"}' "$BASE/api/auth/tokens" | jq -r .token)"
TOKEN_B="$(api -f -b "$JAR_B" -H 'Content-Type: application/json' -d '{"name":"update-path"}' "$BASE/api/auth/tokens" | jq -r .token)"
[ -n "$TOKEN_A" ] && [ "$TOKEN_A" != null ] && [ -n "$TOKEN_B" ] && [ "$TOKEN_B" != null ] || die "no PAT"
# Every seed call must work: stop at the first that does not, with its line, logs dumped by the trap.
trap 'log "FATAL: seed step failed at line $LINENO"; FAIL=$((FAIL + 1))' ERR
set -e
ada="$(mcp "$TOKEN_A" memory_write '{"subject_name":"Ada Lovelace","subject_kind":"person","text":"Wrote the first published algorithm."}')"
ADA="$(jq -r .entity.id <<<"$ada")"; ADA_FACT="$(jq -r .memory.id <<<"$ada")"
ada2="$(mcp "$TOKEN_A" memory_write '{"subject_name":"Ada Lovelace","subject_kind":"person","text":"Lives in Marylebone, London.","type":"world"}')"
OLD_FACT="$(jq -r .memory.id <<<"$ada2")"
mcp "$TOKEN_A" memory_write '{"subject_name":"Ada Lovelace","subject_kind":"person","text":"Lives in St James Square, London.","type":"world"}' >/dev/null
mcp "$TOKEN_A" memory_update "{\"memory_id\":\"$ADA_FACT\",\"text\":\"Published the first algorithm in 1843.\"}" >/dev/null
babbage="$(mcp "$TOKEN_A" memory_write '{"subject_name":"Charles Babbage","subject_kind":"person","text":"Designed the Analytical Engine."}')"
BABBAGE="$(jq -r .entity.id <<<"$babbage")"
mcp "$TOKEN_A" code_relate "{\"from_id\":\"$ADA\",\"to_id\":\"$BABBAGE\",\"label\":\"collaborated_with\"}" >/dev/null
mcp "$TOKEN_A" memory_write '{"subject_name":"Analytical Society","subject_kind":"organisation","text":"Founded in Cambridge in 1812."}' >/dev/null
mcp "$TOKEN_A" memory_write '{"subject_name":"Cambridge","subject_kind":"location","text":"A university city in England."}' >/dev/null
mcp "$TOKEN_A" memory_write '{"subject_name":"orders_db","subject_kind":"db","text":"Runs Postgres 16."}' >/dev/null
gone="$(mcp "$TOKEN_A" memory_write '{"subject_name":"Charles Babbage","subject_kind":"person","text":"A fact that is deleted again."}')"
mcp "$TOKEN_A" memory_delete "{\"memory_id\":\"$(jq -r .memory.id <<<"$gone")\"}" >/dev/null   # a delete under the HNSW index
mcp "$TOKEN_A" code_entity_upsert '{"kind":"repository","name":"eunomia","summary":"memory system"}' >/dev/null
TEAM="$(mcp "$TOKEN_A" vault_create '{"name":"Team"}' | jq -r .id)"
mcp "$TOKEN_A" vault_create '{"name":"Research"}' >/dev/null
mcp "$TOKEN_A" memory_write "{\"subject_name\":\"Grace Hopper\",\"subject_kind\":\"person\",\"text\":\"Built the first compiler.\",\"vault_id\":\"$TEAM\"}" >/dev/null
SKILL="$(mcp "$TOKEN_A" skill_save '{"name":"deploy-rollback","description":"How to roll back a bad deploy","body":"# Roll back\n\n1. Revert."}' | jq -r .id)"
mcp "$TOKEN_A" skill_save '{"name":"deploy-rollback","description":"How to roll back a bad deploy","body":"# Roll back v2"}' >/dev/null
mcp "$TOKEN_A" skill_save '{"name":"release-notes","description":"How to write release notes","body":"Short, plain words."}' >/dev/null
mcp "$TOKEN_B" memory_write '{"subject_name":"orders_db","subject_kind":"db","text":"Runs Postgres 15 in the other org."}' >/dev/null
mcp "$TOKEN_B" skill_save '{"name":"other-org","description":"A skill in the second org","body":"b"}' >/dev/null
printf '# Notes\n\nThe update path keeps every document.\n' > "$W/notes.md"
api -f -H "Authorization: Bearer $TOKEN_A" -H 'Content-Type: text/markdown' --data-binary @"$W/notes.md" "$BASE/api/documents?filename=notes.md" >/dev/null
set +e
trap - ERR
wait_jobs 180 || die "seed jobs did not finish"
# A superseded fact. Superseding needs a chat model to judge contradictions, which this install has
# none of, so mark it the way entities::supersede does (store/entities.rs, mark_superseded).
ORG_A="$(echo "SELECT VALUE db FROM tenant;" | sql control | jq -r '.[0].result[]' | while read -r d; do
  [ "$(echo "SELECT count() FROM $OLD_FACT GROUP ALL;" | sql "$d" | jq -r '.[0].result[0].count // 0')" = 1 ] && echo "$d"; done | head -1)"
[ -n "$ORG_A" ] || die "could not find the org database of the first user"
echo "UPDATE $OLD_FACT SET status = 'superseded';" | sql "$ORG_A" >/dev/null
n_orgs() { [ "$(org_dbs | wc -l | tr -d ' ')" -eq "$1" ]; }
check "two users, two org databases" n_orgs 2

# ---- 5. snapshot -------------------------------------------------------------------------------
log "5. snapshot"
counts > "$W/counts-before.txt"; ledgers > "$W/ledgers-before.txt"
reads "$W/reads-before.txt" || { cat "$W/reads-before.txt"; die "reads before the update failed"; }
check "the seed reached the org tables" grep -qE '^org_.*\.memory [1-9]' "$W/counts-before.txt"
check "recall finds the seeded facts before the update" bash -c "grep '^recall-a ' '$W/reads-before.txt' | grep -q memory"
check "the superseded fact is in the history" grep -q superseded "$W/reads-before.txt"
check "one document" bash -c "grep '^documents ' '$W/reads-before.txt' | grep -q document"
log "   $(wc -l < "$W/counts-before.txt") tables; $(tr '\n' ';' < "$W/ledgers-before.txt")"

# ---- 6. pre-update backup ----------------------------------------------------------------------
log "6. backup now pre-update"
dc run --rm --no-deps -T backup now pre-update > "$W/backup.txt" 2>&1 || { cat "$W/backup.txt"; die "backup now pre-update failed"; }
BACKUP="$(dc exec -T backup sh -c 'ls -1t /backups | grep "^pre-update" | head -1')"
[ -n "$BACKUP" ] || die "no pre-update backup in the volume"
log "   $BACKUP"

# ---- 7. update as a user would -----------------------------------------------------------------
log "7. Update now, then $FROM's scripts/auto-update.sh"
dc stop updater >/dev/null 2>&1   # the host run below is THE updater for this test; no race with the service
api -f -b "$JAR_A" -X POST "$BASE/api/update/request" >/dev/null || die "POST /api/update/request failed"
check "the backend dropped the requested marker" test -f "$I/update-status/requested"
mkdir -p "$W/shim"
cat > "$W/shim/docker" <<EOF
#!/usr/bin/env bash
# only \`docker compose pull\` is faked: $FAKE exists in this daemon, not on ghcr
if [ "\${1:-}" = compose ] && [ "\${2:-}" = pull ]; then echo "shim: docker \$*" >> "$W/shim.log"; exit 0; fi
exec "$(command -v docker)" "\$@"
EOF
chmod +x "$W/shim/docker"
t=$SECONDS
EUNOMIA_DIR="$I" UP_RETRY_FOR=900 PATH="$W/shim:$PATH" bash "$I/scripts/auto-update.sh" > "$W/auto-update-1.txt" 2>&1; rc=$?
log "   auto-update.sh exit $rc after $((SECONDS - t)) s"
check "auto-update.sh exits 0" test "$rc" -eq 0
check "the shim saw exactly one compose pull" bash -c "[ \"\$(wc -l < '$W/shim.log')\" -eq 1 ]"

# ---- 8. assert the update ----------------------------------------------------------------------
log "8. asserting the update"
cp "$I/update-status/status.json" "$W/status-1.json" 2>/dev/null
check "status.json: current_version $FAKE" bash -c "jq -e '.current_version == \"$FAKE\"' '$I/update-status/status.json'"
check "status.json: no error, not applying" bash -c "jq -e '.error == null and .applying == false and .update_available == false' '$I/update-status/status.json'"
check "history.log: updated $FROM -> $FAKE" grep -q "updated $FROM -> $FAKE" "$I/update-status/history.log"
check "the checkout is on $FAKE" bash -c "[ \"\$(git -C '$I' rev-parse HEAD)\" = \"\$(git -C '$W/remote.git' rev-parse '$FAKE^{commit}')\" ]"
check ".env: EUNOMIA_IMAGE_TAG=$FAKE" grep -qx "EUNOMIA_IMAGE_TAG=$FAKE" "$I/.env"
wait_healthy 600; check "every service healthy after the update" test $? -eq 0
check "nothing left Created or exited (bar the one-shot)" bash -c "! (cd '$I' && docker compose ps -a --format '{{.Service}} {{.State}}') | grep -v '^surreal-upgrade ' | grep -v '^updater ' | grep -qE ' (created|exited)'"
check "surrealdb runs surrealdb/surrealdb:$PIN" bash -c "[ \"\$(docker inspect -f '{{.Config.Image}}' \"\$(cd '$I' && docker compose ps -q surrealdb)\")\" = surrealdb/surrealdb:$PIN ]"
check "surrealdb healthcheck is isready over ws" bash -c "docker inspect -f '{{json .Config.Healthcheck.Test}}' \"\$(cd '$I' && docker compose ps -q surrealdb)\" | grep -q 'ws://'"
for s in backend frontend backup; do
  check "$s runs eunomia-$s:$FAKE" bash -c "[ \"\$(docker inspect -f '{{.Config.Image}}' \"\$(cd '$I' && docker compose ps -q $s)\")\" = ghcr.io/cashara-io/eunomia-$s:$FAKE ]"
done
check "/readyz answers 200" curl -fsS -o /dev/null --max-time 10 "http://127.0.0.1:$BPORT/readyz"
wait_tenants 300; check "every ledger at the latest migration (tenant $LATEST_TENANT, control $LATEST_CONTROL)" test $? -eq 0
wait_jobs 180
counts > "$W/counts-after.txt"; ledgers > "$W/ledgers-after.txt"
reads "$W/reads-after.txt"; check "the reads answer after the update" test $? -eq 0
stable "$W/counts-before.txt" > "$W/stable-before.txt"; stable "$W/counts-after.txt" > "$W/stable-after.txt"
check "per-table row counts unchanged (bar $VOLATILE)" diff "$W/stable-before.txt" "$W/stable-after.txt"
diff "$W/stable-before.txt" "$W/stable-after.txt" | head -20
check "the same tables before and after" diff <(cut -d' ' -f1 "$W/counts-before.txt") <(cut -d' ' -f1 "$W/counts-after.txt")
check "the fixed reads unchanged" diff "$W/reads-before.txt" "$W/reads-after.txt"
diff "$W/reads-before.txt" "$W/reads-after.txt" | head -20
# Tenant 0022 (rebuild_vector_indexes): in every org ledger, the three HNSW indexes defined, and the
# rebuilt indexes answer kNN over the live rows (the reads above go through recall and skill_find, whose
# exact-scan fallback would hide a broken index, so this queries the index itself). Row counts: above.
check "every org ledger has 0022_rebuild_vector_indexes" ledger_has 22 rebuild_vector_indexes
check "every org defines the three HNSW indexes" vector_indexes
wait_vector_builds 300; check "the rebuilt vector indexes finished building" test $? -eq 0
check "kNN through the rebuilt memory and skill indexes returns the live rows" knn_live
check "no tenant.schema_behind" bash -c "! curl -sS -H 'Authorization: Bearer $TOKEN_A' -H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream' -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"recall\",\"arguments\":{\"query\":\"algorithm\"}}}' '$BASE/mcp' | grep -q schema_behind"

# ---- 9. idempotent -----------------------------------------------------------------------------
log "9. again: the updater, a backend restart, up -d"
hist="$(wc -l < "$I/update-status/history.log")"
EUNOMIA_DIR="$I" PATH="$W/shim:$PATH" bash "$I/scripts/auto-update.sh" > "$W/auto-update-2.txt" 2>&1; rc=$?
check "a second auto-update.sh exits 0" test "$rc" -eq 0
check "still on $FAKE, no new history line" bash -c "jq -e '.current_version == \"$FAKE\" and .error == null' '$I/update-status/status.json' && [ \"\$(wc -l < '$I/update-status/history.log')\" -eq $hist ]"
dc restart backend >/dev/null 2>&1; wait_healthy 300; check "healthy after a backend restart" test $? -eq 0
cids() { local s; for s in surrealdb backend frontend backup; do dc ps -q "$s"; done; }
ids1="$(cids)"
dc up -d > "$W/up-again.txt" 2>&1; check "a second up -d succeeds" test $? -eq 0
updater_runs() { [ "$(dc ps --format '{{.Service}} {{.State}}' updater)" = "updater running" ]; }
check "the updater service runs on $FAKE's compose file" updater_runs
dc stop updater >/dev/null 2>&1   # the updater service (started by up) would race the steps below
ids2="$(cids)"
check "the second up -d recreated nothing" test "$ids1" = "$ids2"
wait_healthy 300
ledgers > "$W/ledgers-again.txt"
check "no new ledger rows" diff "$W/ledgers-after.txt" "$W/ledgers-again.txt"
counts > "$W/counts-again.txt"; stable "$W/counts-again.txt" > "$W/stable-again.txt"
check "counts unchanged after the re-run" diff "$W/stable-before.txt" "$W/stable-again.txt"

# ---- 10. rollback ------------------------------------------------------------------------------
log "10. rollback to $FROM"
VOL="$(docker inspect -f '{{range .Mounts}}{{if eq .Destination "/data"}}{{.Name}}{{end}}{{end}}' "$(dc ps -q surrealdb)")"
dc stop >/dev/null 2>&1
# Informational, not a pass condition: can the old engine open a volume the new one has written?
docker volume create "$P-oldengine-copy" >/dev/null
docker run --rm --user root -v "$VOL:/from:ro" -v "$P-oldengine-copy:/to" --entrypoint sh "$CURL_IMG" -c 'cp -a /from/. /to/'
docker run -d --name "$P-oldengine" --user root -v "$P-oldengine-copy:/data" "$OLDPIN" start --user root --pass root rocksdb:/data/eunomia.db >/dev/null 2>&1
sleep 8
if docker exec "$P-oldengine" /surreal isready --endpoint ws://localhost:8000 >/dev/null 2>&1; then
  log "   INFO: $OLDPIN opens the volume $PIN wrote"; echo "opens" > "$W/oldengine.txt"
else
  log "   INFO: $OLDPIN does NOT open the volume $PIN wrote: $(docker logs "$P-oldengine" 2>&1 | tail -2 | tr '\n' ' ')"; echo "refuses" > "$W/oldengine.txt"
fi
docker logs "$P-oldengine" > "$W/oldengine-log.txt" 2>&1
docker rm -f "$P-oldengine" >/dev/null 2>&1; docker volume rm "$P-oldengine-copy" >/dev/null 2>&1
# The supported rollback: the old tag, a fresh data volume, the pre-update backup restored into it.
git -C "$I" checkout -q "$FROM" || die "checkout of $FROM failed"
sed -i.bak "s#^EUNOMIA_IMAGE_TAG=.*#EUNOMIA_IMAGE_TAG=$FROM#" "$I/.env" && rm -f "$I/.env.bak"
dc rm -f >/dev/null 2>&1
docker volume rm "$VOL" >/dev/null || die "could not remove $VOL"
dc up -d surrealdb backup > "$W/up-rollback-db.txt" 2>&1 || { cat "$W/up-rollback-db.txt"; die "old surrealdb and backup did not start"; }
# A fresh server has no namespace yet, and 3.x refuses a database statement in a missing one.
for _ in $(seq 30); do echo 'DEFINE NAMESPACE IF NOT EXISTS eunomia;' | sql_root 2>/dev/null | grep -q '"OK"' && break; sleep 2; done
(cd "$I" && bash backend/scripts/restore.sh "$BACKUP" --wipe) > "$W/restore.txt" 2>&1; rc=$?
check "restore.sh $BACKUP --wipe on $FROM exits 0" test "$rc" -eq 0
[ "$rc" -eq 0 ] || tail -20 "$W/restore.txt"
dc up -d > "$W/up-rollback.txt" 2>&1; dc stop updater >/dev/null 2>&1
wait_healthy 300; check "$FROM healthy on the restored data" test $? -eq 0
check "surrealdb is back on $OLDPIN" bash -c "[ \"\$(docker inspect -f '{{.Config.Image}}' \"\$(cd '$I' && docker compose ps -q surrealdb)\")\" = '$OLDPIN' ]"
check "backend is back on $FROM" bash -c "[ \"\$(docker inspect -f '{{.Config.Image}}' \"\$(cd '$I' && docker compose ps -q backend)\")\" = ghcr.io/cashara-io/eunomia-backend:$FROM ]"
wait_jobs 120
counts > "$W/counts-rollback.txt"; stable "$W/counts-rollback.txt" > "$W/stable-rollback.txt"
check "counts after the rollback match the snapshot" diff "$W/stable-before.txt" "$W/stable-rollback.txt"
diff "$W/stable-before.txt" "$W/stable-rollback.txt" | head -20
reads "$W/reads-rollback.txt"; check "the reads answer after the rollback" test $? -eq 0
check "the fixed reads after the rollback match the snapshot" diff "$W/reads-before.txt" "$W/reads-rollback.txt"
diff "$W/reads-before.txt" "$W/reads-rollback.txt" | head -20
ledgers > "$W/ledgers-rollback.txt"
check "the ledgers after the rollback match the snapshot" diff "$W/ledgers-before.txt" "$W/ledgers-rollback.txt"

[ "$FAIL" -eq 0 ]

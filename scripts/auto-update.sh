#!/usr/bin/env bash
# Self-update check/apply. Run every 20s by the `updater` compose service
# (scripts/updater.sh), or by hand / cron on the host -- a lock keeps two
# runners from overlapping. See docs/deployment.md's "Updates" section.
#
# The web-facing backend never gets git or the docker socket: it only drops
# marker files in update-status/ ("check" = check GitHub now, "requested" =
# apply the newest release), and this script does the rest.
#
# Writes $REPO/update-status/status.json (polled by GET /api/update/status)
# at most every CHECK_EVERY_MIN minutes, comparing the running release
# (EUNOMIA_IMAGE_TAG in .env) with the newest v* tag on GitHub. Applies an
# update -- check out that tag, pull its prebuilt images, restart -- only when
# update-status/requested exists (written by POST /api/update/request, i.e.
# someone clicked "Update now"). Never auto-applies on its own.
#
# Everything runs inside main() so bash has parsed the whole script before
# `git checkout` replaces this file with the new release's copy.
#
# Every run also applies Settings, HTTPS (see https_apply below).
set -uo pipefail
# cron/launchd start with a bare PATH; appended so an existing PATH still wins
export PATH="$PATH:/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin"

CHECK_EVERY_MIN=10
# After an update, keep retrying `docker compose up` for this long (seconds), starting this many
# seconds apart and doubling up to 2 minutes. See start_stack.
UP_RETRY_FOR="${UP_RETRY_FOR:-1800}"
UP_RETRY_DELAY="${UP_RETRY_DELAY:-10}"

main() {
  local repo status_dir current latest
  repo="${EUNOMIA_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
  status_dir="$repo/update-status"
  mkdir -p "$status_dir"
  cd "$repo" || exit 1

  # One runner at a time (container + an old host cron job may both exist).
  # A lock older than 30 minutes is from a crashed run.
  find "$status_dir/.lock" -maxdepth 0 -mmin +30 -exec rmdir {} \; 2>/dev/null
  mkdir "$status_dir/.lock" 2>/dev/null || exit 0
  trap "rmdir '$status_dir/.lock' 2>/dev/null" EXIT

  https_apply "$status_dir"
  heal_stack "$status_dir"

  # Eunomia moved from Qyrhal/Eunomia: installs cloned from there follow it.
  case "$(git remote get-url origin 2>/dev/null)" in
    *[Qq]yrhal/[Ee]unomia*) git remote set-url origin https://github.com/Cashara-io/eunomia.git ;;
  esac

  # Nothing requested and checked recently: skip the network round-trip.
  if [ ! -f "$status_dir/requested" ] && [ ! -f "$status_dir/check" ] \
    && [ -n "$(find "$status_dir/status.json" -mmin -$CHECK_EVERY_MIN 2>/dev/null)" ]; then
    exit 0
  fi
  rm -f "$status_dir/check"

  current="$(sed -n 's/^EUNOMIA_IMAGE_TAG=//p' .env 2>/dev/null | tail -1)"
  [ -n "$current" ] || current="$(git describe --tags --exact-match 2>/dev/null || echo unknown)"

  if ! latest="$(git ls-remote --tags --refs origin 'v*' 2>"$status_dir/.last-error" | sed 's#.*refs/tags/##' | sort -V | tail -1)" || [ -z "$latest" ]; then
    write_status "$status_dir" "$current" "$current" false "could not reach GitHub to check for releases: $(cat "$status_dir/.last-error")"
    exit 1
  fi

  if [ ! -f "$status_dir/requested" ] || [ "$current" = "$latest" ]; then
    rm -f "$status_dir/requested"
    write_status "$status_dir" "$current" "$latest" false ""
    exit 0
  fi

  write_status "$status_dir" "$current" "$latest" true ""
  rm -f "$status_dir/requested"

  # Local edits to tracked files (e.g. docker-compose.yml) can't be swapped
  # for the new release's copy safely -- report rather than clobber them.
  if ! git diff --quiet || ! git diff --cached --quiet; then
    write_status "$status_dir" "$current" "$latest" false "local changes in $repo -- commit or stash them, then click Update again"
    exit 1
  fi

  local prev_ref
  prev_ref="$(git rev-parse HEAD 2>/dev/null)"
  if ! { git fetch --depth 1 origin tag "$latest" --quiet && git checkout --quiet "$latest"; } 2>"$status_dir/.last-error"; then
    write_status "$status_dir" "$current" "$latest" false "$(cat "$status_dir/.last-error")"
    exit 1
  fi

  local had_tag=false
  if grep -q '^EUNOMIA_IMAGE_TAG=' .env; then
    had_tag=true
    sed -i.bak "s#^EUNOMIA_IMAGE_TAG=.*#EUNOMIA_IMAGE_TAG=$latest#" .env && rm -f .env.bak
  else
    echo "EUNOMIA_IMAGE_TAG=$latest" >> .env
  fi
  # Installs from before nightly backups have no backup key yet. A running backup service
  # without one made its own (in its volume): keep using that one, so earlier backups and new
  # ones share a key.
  if ! grep -q '^BACKUP_ENCRYPTION_KEY=.' .env; then
    local bkey
    bkey="$(docker compose exec -T backup cat /backups/.backup-key 2>/dev/null | tr -d '\n')"
    [[ "$bkey" =~ ^[A-Za-z0-9+/=]{16,}$ ]] || bkey="$(head -c 32 /dev/urandom | base64 | tr -d '\n')"
    sed -i.bak '/^BACKUP_ENCRYPTION_KEY=/d' .env && rm -f .env.bak
    echo "BACKUP_ENCRYPTION_KEY=$bkey" >> .env
  fi
  # A release that pins SurrealDB 3.x over 2.x (running, or stopped: the script finds the volume itself) needs its data moved
  # first (export, fresh volume, verify; see docs/upgrading-to-surrealdb-3.md).
  # The script rolls itself back on failure, so here we only undo the checkout
  # to let "Update now" be retried. It can run for a long time: keep the lock fresh.
  local new_major old_major
  new_major="$(docker compose config --images 2>/dev/null | sed -n 's#^surrealdb/surrealdb:v\{0,1\}\([0-9][0-9]*\)\..*#\1#p' | head -1)"
  old_major="$(docker inspect -f '{{.Config.Image}}' "$(docker compose ps -q surrealdb 2>/dev/null | head -1)" 2>/dev/null | sed -n 's#.*:v\{0,1\}\([0-9][0-9]*\)\..*#\1#p')"
  if [ "$new_major" = 3 ] && [ "$old_major" != 3 ]; then
    ( while sleep 60; do touch "$status_dir/.lock"; done ) & local keepalive=$!; disown "$keepalive"
    bash scripts/upgrade-surreal-v3.sh >> "$status_dir/upgrade.log" 2>&1; local rc=$?
    kill "$keepalive" 2>/dev/null
    if [ "$rc" -ne 0 ]; then
      git checkout --quiet "$prev_ref" 2>/dev/null
      # only write the old tag back if .env had one: "unknown" or a git tag would break the next pull
      if $had_tag; then sed -i.bak "s#^EUNOMIA_IMAGE_TAG=.*#EUNOMIA_IMAGE_TAG=$current#" .env; else sed -i.bak '/^EUNOMIA_IMAGE_TAG=/d' .env; fi
      rm -f .env.bak
      write_status "$status_dir" "$current" "$latest" false "SurrealDB 3 upgrade failed and was rolled back, nothing changed: $(tail -3 "$status_dir/upgrade.log")"
      exit 1
    fi
  fi

  # Installs from before the key was required ran with an empty ENCRYPTION_KEY, so their stored
  # credentials and org database passwords were written under it. Give them a real key and let the
  # backend still read the old values (ENCRYPTION_KEY_LEGACY_EMPTY=1); new writes use the new key.
  # See docs/deployment.md, "Rotating ENCRYPTION_KEY". An existing key is never replaced.
  # Done only after the SurrealDB upgrade step: a rolled-back update must leave .env able to run the old release.
  if ! grep -q '^ENCRYPTION_KEY=.' .env; then
    sed -i.bak -e '/^ENCRYPTION_KEY=/d' -e '/^ENCRYPTION_KEY_LEGACY_EMPTY=/d' .env && rm -f .env.bak
    echo "ENCRYPTION_KEY=$(head -c 32 /dev/urandom | base64 | tr -d '\n')" >> .env
    echo "ENCRYPTION_KEY_LEGACY_EMPTY=1" >> .env
  fi

  if ! grep -q '^JWT_SECRET=.' .env; then
    sed -i.bak '/^JWT_SECRET=/d' .env && rm -f .env.bak
    echo "JWT_SECRET=$(head -c 32 /dev/urandom | base64 | tr -d '\n')" >> .env
  fi

  # The key the backend and the secrets-helper sidecar share (docs/architecture/specs/broker.md). Without
  # one the sidecar still only answers on its internal network; hex, so it needs no quoting anywhere.
  if ! grep -q '^SECRETS_HELPER_KEY=.' .env; then
    sed -i.bak '/^SECRETS_HELPER_KEY=/d' .env && rm -f .env.bak
    echo "SECRETS_HELPER_KEY=$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')" >> .env
  fi

  # Everything except the updater itself (recreating it here would kill this
  # run); it is refreshed last, after the status is written.
  local services
  services="$(docker compose config --services 2>/dev/null | grep -vx updater | tr '\n' ' ')"
  touch "$status_dir/heal" # cleared once the stack is up; until then each run starts what is left
  # the images this release publishes; a compose file without secrets-helper (an older tag) pulls the three
  local images="backend frontend backup"
  printf '%s\n' $services | grep -qx secrets-helper && images="$images secrets-helper"
  # shellcheck disable=SC2086 # a list of service names
  if ! docker compose pull $images >/dev/null 2>"$status_dir/.last-error" \
    || ! start_stack "$status_dir" "$current" "$latest" $services; then
    write_status "$status_dir" "$latest" "$latest" false "restart failed: $(tail -5 "$status_dir/.last-error")"
    exit 1
  fi

  # HTTPS on: the new release may ship a changed Caddyfile, which compose
  # doesn't notice (it's a mounted file), so reload it. No downtime.
  if printf '%s\n' $services | grep -qx caddy; then
    docker compose exec -T caddy caddy reload --config /etc/caddy/Caddyfile >/dev/null 2>&1 || true
  fi

  rm -f "$status_dir/.last-error"
  write_status "$status_dir" "$latest" "$latest" false ""
  echo "$(date -u +%Y-%m-%dT%H:%M:%SZ) updated $current -> $latest" >> "$status_dir/history.log"
  # Pick up a changed updater definition. Not from in here: recreating the
  # container this runs in would kill the command halfway and leave no
  # updater. A short-lived sibling container does it a few seconds later
  # (a no-op when nothing changed). The repo is at the same path on the host.
  # The container runs the copy of scripts/updater.sh taken when it started, and compose
  # only sees the service definition, so a changed updater.sh forces a recreate.
  if [ -f /.dockerenv ] && docker compose config --services 2>/dev/null | grep -qx updater; then
    local recreate=""
    git diff --quiet "$prev_ref" HEAD -- scripts/updater.sh 2>/dev/null || recreate=--force-recreate
    docker run -d --rm -v /var/run/docker.sock:/var/run/docker.sock -v "$PWD:$PWD" -w "$PWD" docker:27-cli \
      sh -c "sleep 5 && docker compose up -d $recreate updater" >/dev/null 2>&1
  fi
}

# start_stack <status_dir> <current> <latest> <service>...: `docker compose up -d`, retried with
# backoff for up to UP_RETRY_FOR seconds. A first boot after a big update (moving data, running
# migrations) can outlast compose's wait for the backend to turn healthy: `up` then fails with
# "dependency failed to start" and leaves the services after it (the frontend) Created, so the site
# stays down even once the backend is fine. Each retry starts what is left. Progress goes to
# status.json; the caller reports the failure if the last try fails too.
start_stack() {
  local dir="$1" current="$2" latest="$3" delay="$UP_RETRY_DELAY" started=$SECONDS try=1
  shift 3
  until docker compose up -d --remove-orphans "$@" >/dev/null 2>"$dir/.last-error"; do
    [ $((SECONDS - started)) -lt "$UP_RETRY_FOR" ] || return 1
    write_status "$dir" "$current" "$latest" true "" "Waiting for Eunomia to start (try $try: $(tail -1 "$dir/.last-error")). Retrying in ${delay}s."
    touch "$dir/.lock" # a long first boot is not a crashed run
    sleep "$delay"
    try=$((try + 1))
    delay=$((delay * 2)); [ "$delay" -le 120 ] || delay=120
  done
  rm -f "$dir/heal"
}

# heal_stack <status_dir>: after an update whose `up` failed or was interrupted (the `heal` marker),
# once the backend is healthy, start any service left Created or exited, then clear the marker.
# Without the marker it does nothing, so a service stopped by hand stays stopped. Skips the updater
# itself and the one-shot surreal-upgrade.
heal_stack() {
  local ps stuck
  [ -f "$1/heal" ] || return 0
  ps="$(docker compose ps -a --format '{{.Service}} {{.State}} {{.Health}}' 2>/dev/null)" || return 0
  printf '%s\n' "$ps" | grep -q '^backend running healthy' || return 0
  stuck="$(printf '%s\n' "$ps" | awk '$1 != "updater" && $1 != "surreal-upgrade" && ($2 == "created" || $2 == "exited") { print $1 }' | sort -u | tr '\n' ' ')"
  if [ -n "$stuck" ]; then
    echo "$(date -u +%Y-%m-%dT%H:%M:%SZ) starting stuck services: $stuck"
    docker compose up -d $stuck >/dev/null 2>&1 || return 0
  fi
  rm -f "$1/heal"
}

# set_env <KEY> <value>: replace or append KEY=value in .env. Callers pass
# validated values only (no '#', '&' or newlines).
set_env() {
  if grep -q "^$1=" .env 2>/dev/null; then
    sed -i.bak "s#^$1=.*#$1=$2#" .env && rm -f .env.bak
  else
    echo "$1=$2" >> .env
  fi
}

# unset_env <KEY>...: remove the keys from .env.
unset_env() {
  local k
  for k in "$@"; do sed -i.bak "/^$k=/d" .env && rm -f .env.bak; done
}

# --- HTTPS (Settings, HTTPS) ----------------------------------------------
# POST /api/https writes update-status/https.json ({"enabled", "domain",
# "email"}). This validates it again (untrusted input), points .env at the
# domain, starts or removes the `caddy` service, and reports progress in
# update-status/https-status.json: off, pending (until the certificate
# answers), active, or error.
# Trust model: Caddy proxies to the frontend (never to the backend), so the
# backend keeps trusting only the frontend (TRUSTED_PROXIES=frontend). Turning
# HTTPS on makes the frontend believe the X-Forwarded-* headers Caddy sets
# (FRONTEND_TRUST_FORWARDED=1) and, so nobody can forge them, stops publishing
# its port on the network (FRONTEND_BIND=127.0.0.1; Caddy reaches it over the
# compose network). It also points PUBLIC_URL at the domain for OAuth.
https_apply() {
  local dir="$1" req="$1/https.json" enabled domain email
  if [ -f "$req" ]; then
    enabled="$(json_field enabled "$req")"
    domain="$(json_field domain "$req")"
    email="$(json_field email "$req")"
    rm -f "$req"
    if [ "$enabled" != true ]; then
      domain="$(sed -n 's/^EUNOMIA_DOMAIN=//p' .env | tail -1)"
      [ "$(sed -n 's/^PUBLIC_URL=//p' .env | tail -1)" = "https://$domain" ] && unset_env PUBLIC_URL
      unset_env COMPOSE_PROFILES FRONTEND_TRUST_FORWARDED FRONTEND_BIND
      if docker compose rm -sf caddy >/dev/null 2>"$dir/.https-error" && docker compose up -d backend frontend >/dev/null 2>>"$dir/.https-error"; then
        write_https_status "$dir" off "" ""
      else
        write_https_status "$dir" error "" "could not stop caddy: $(tail -3 "$dir/.https-error")"
      fi
      return
    fi
    if ! valid_domain "$domain" || ! valid_email "$email"; then
      write_https_status "$dir" error "" "refused: invalid domain or email"
      return
    fi
    set_env EUNOMIA_DOMAIN "$domain"
    set_env EUNOMIA_ACME_EMAIL "$email"
    set_env COMPOSE_PROFILES https
    set_env FRONTEND_TRUST_FORWARDED 1
    set_env FRONTEND_BIND 127.0.0.1
    set_env PUBLIC_URL "https://$domain"
    if ! docker compose up -d backend frontend caddy >/dev/null 2>"$dir/.https-error"; then
      write_https_status "$dir" error "$domain" "could not start caddy: $(tail -3 "$dir/.https-error")"
      return
    fi
    write_https_status "$dir" pending "$domain" ""
  fi

  # Waiting for the certificate (after the request above, or an install with
  # --domain): probe until it answers. Through the caddy container itself, so
  # a router without NAT loopback doesn't make a working setup look broken.
  grep -qx 'COMPOSE_PROFILES=https' .env 2>/dev/null || return 0
  case "$(json_field state "$dir/https-status.json" 2>/dev/null)" in ""|pending) ;; *) return 0 ;; esac
  domain="$(sed -n 's/^EUNOMIA_DOMAIN=//p' .env | tail -1)"
  local target=127.0.0.1
  [ -f /.dockerenv ] && target=caddy
  command -v curl >/dev/null || apk add --no-cache -q curl >/dev/null 2>&1 # updater containers started before curl was added
  if curl -sS -o /dev/null --max-time 10 --connect-to "$domain:443:$target:443" "https://$domain/" 2>"$dir/.https-error"; then
    write_https_status "$dir" active "$domain" ""
  else
    write_https_status "$dir" pending "$domain" "no certificate yet. Check that $domain points at this machine and ports 80 and 443 are open; Caddy keeps retrying. ($(tail -1 "$dir/.https-error"))"
  fi
}

# json_field <key> <file>: a string/bool value from one-key-per-line JSON.
json_field() {
  sed -n "s/^ *\"$1\": *\"\{0,1\}\([^\",]*\)\"\{0,1\},\{0,1\} *$/\1/p" "$2" | head -1
}

# A public DNS name Let's Encrypt can issue for: dot-separated labels of
# letters/digits/hyphens ending in an alphabetic TLD (so no IPs).
valid_domain() {
  local label='[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?'
  [ "${#1}" -le 253 ] && [[ "$1" =~ ^($label\.)+[A-Za-z]{2,63}$ ]]
}

valid_email() {
  [ "${#1}" -le 254 ] && [[ "$1" =~ ^[A-Za-z0-9._%+-]{1,64}@(.+)$ ]] && valid_domain "${BASH_REMATCH[1]}"
}

# write_https_status <dir> <state> <domain> <message>
write_https_status() {
  local message=null
  if [ -n "$4" ]; then
    message="\"$(printf '%s' "$4" | tr '\n\r\t' '   ' | sed 's/\\/\\\\/g; s/"/\\"/g')\""
  fi
  cat > "$1/https-status.json.tmp" <<EOF
{
  "state": "$2",
  "domain": "$3",
  "message": $message,
  "checked_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
}
EOF
  mv "$1/https-status.json.tmp" "$1/https-status.json"
}

# write_status <dir> <current> <latest> <applying> <error> [progress]
write_status() {
  local error=null progress=null
  if [ -n "$5" ]; then
    error="\"$(printf '%s' "$5" | tr '\n\r\t' '   ' | sed 's/\\/\\\\/g; s/"/\\"/g')\""
  fi
  if [ -n "${6:-}" ]; then
    progress="\"$(printf '%s' "$6" | tr '\n\r\t' '   ' | sed 's/\\/\\\\/g; s/"/\\"/g')\""
  fi
  cat > "$1/status.json.tmp" <<EOF
{
  "current_version": "$2",
  "latest_version": "$3",
  "update_available": $([ "$2" != "$3" ] && echo true || echo false),
  "checked_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "applying": $4,
  "error": $error,
  "progress": $progress
}
EOF
  mv "$1/status.json.tmp" "$1/status.json"
}

main "$@"
exit

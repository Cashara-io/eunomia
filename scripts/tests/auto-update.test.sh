#!/usr/bin/env bash
# Tests scripts/auto-update.sh: a local git repo stands in for GitHub and a
# `docker` shim records what would have run.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REAL_SCRIPT="$HERE/../auto-update.sh"
PASS=0; FAIL=0
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
check() { local d="$1"; shift; if "$@" >/dev/null 2>&1; then PASS=$((PASS + 1)); else FAIL=$((FAIL + 1)); echo "FAIL: $d"; fi; }
field() { python3 -c 'import json,sys;print(str(json.load(open(sys.argv[1]))[sys.argv[2]]).lower())' "$TMP/repo/update-status/status.json" "$1"; }
git_() { git -c user.email=t@t -c user.name=t "$@"; }

# "GitHub": v1.0.0 -> v1.1.0
mkdir -p "$TMP/src/scripts" "$TMP/bin"
cp "$REAL_SCRIPT" "$TMP/src/scripts/auto-update.sh"
(cd "$TMP/src" && git init -q -b main && echo one > compose.yml && git_ add -A && git_ commit -qm v1 && git tag v1.0.0 \
  && echo two > compose.yml && git_ commit -qam v1.1 && git tag v1.1.0)
git clone -q --branch v1.0.0 "file://$TMP/src" "$TMP/repo" 2>/dev/null
printf 'JWT_SECRET=x\nEUNOMIA_IMAGE_TAG=v1.0.0\n' > "$TMP/repo/.env"
# Besides logging, the shim answers the queries the SurrealDB 3 hook makes (the compose file's
# images in $TMP/images, the running surrealdb image in $TMP/running) and the service list
# ($TMP/services; default: no caddy).
printf 'surrealdb\nbackend\nfrontend\nupdater\n' > "$TMP/services"
cat > "$TMP/bin/docker" <<SHIM
#!/bin/sh
echo "\$@" >> "$TMP/docker.log"
case "\$*" in
  "compose config --images") cat "$TMP/images" 2>/dev/null ;;
  "compose config --services") cat "$TMP/services" ;;
  "compose ps -q surrealdb") [ -f "$TMP/running" ] && echo fakecid ;;
  inspect*) cat "$TMP/running" 2>/dev/null ;;
  "compose ps -a --format"*) cat "$TMP/ps" 2>/dev/null ;;
  # \$TMP/up_fails: how many more \`up --remove-orphans\` calls fail the way a slow first boot does
  "compose up -d --remove-orphans"*)
    n="\$(cat "$TMP/up_fails" 2>/dev/null || echo 0)"
    if [ "\$n" -gt 0 ]; then
      echo \$((n - 1)) > "$TMP/up_fails"
      echo "dependency failed to start: container eunomia-backend-1 is unhealthy" >&2
      exit 1
    fi ;;
esac
exit 0
SHIM
chmod +x "$TMP/bin/docker"
update() { EUNOMIA_DIR="$TMP/repo" PATH="$TMP/bin:$PATH" UP_RETRY_DELAY=0 bash "$TMP/repo/scripts/auto-update.sh"; }
# what ran besides the cheap per-run health check
ran() { grep -v '^compose ps -a --format' "$TMP/docker.log" 2>/dev/null; }
S="$TMP/repo/update-status"

# 1. a check alone reports, never applies
update
check "status says an update is available" test "$(field update_available)" = true
check "status names both versions" test "$(field current_version)" = v1.0.0 -a "$(field latest_version)" = v1.1.0
check "a check alone doesn't touch the checkout" test "$(cat "$TMP/repo/compose.yml")" = one
check "a check alone doesn't run docker (beyond the health check)" test -z "$(ran)"

# 1b. "Check now": a check marker forces a GitHub check inside the 10-minute window
(cd "$TMP/src" && git_ tag v1.0.5 HEAD~1)
update; check "throttled: still sees v1.1.0 as newest" test "$(field latest_version)" = v1.1.0
git -C "$TMP/src" tag -d v1.0.5 >/dev/null
touch "$S/check"; update
check "check marker consumed" test ! -e "$S/check"

# 1c. a second runner while one holds the lock does nothing
mkdir "$S/.lock"; touch "$S/requested"; update
check "locked: nothing applied" test "$(cat "$TMP/repo/compose.yml")" = one
check "locked: marker left for the lock holder" test -e "$S/requested"
rmdir "$S/.lock"; rm -f "$S/requested"

# 2. the button (marker file) applies it
touch "$S/requested"
update
check "checked out the new release" test "$(cat "$TMP/repo/compose.yml")" = two
check ".env is pinned to the new release" grep -q '^EUNOMIA_IMAGE_TAG=v1.1.0$' "$TMP/repo/.env"
check ".env keeps its other settings" grep -q '^JWT_SECRET=x$' "$TMP/repo/.env"
check ".env gains a backup key" grep -Eq '^BACKUP_ENCRYPTION_KEY=.{20,}$' "$TMP/repo/.env"
check ".env gains an encryption key" grep -Eq '^ENCRYPTION_KEY=.{20,}$' "$TMP/repo/.env"
check ".env marks the old empty-key data readable" grep -q '^ENCRYPTION_KEY_LEGACY_EMPTY=1$' "$TMP/repo/.env"
KEY_AFTER_FIRST="$(sed -n 's/^ENCRYPTION_KEY=//p' "$TMP/repo/.env")"
check "images were pulled" grep -q 'compose pull backend frontend' "$TMP/docker.log"
check "stack was restarted" grep -q 'compose up -d' "$TMP/docker.log"
check "the updater service is left out of the restart" bash -c "! grep -q 'up -d --remove-orphans.*updater' '$TMP/docker.log'"
check "lock released" test ! -e "$S/.lock"
check "never recreates the updater from inside itself" bash -c "! grep -q 'up -d updater' '$TMP/docker.log'"
check "status now up to date" test "$(field current_version)" = v1.1.0 -a "$(field update_available)" = false
check "marker file consumed" test ! -e "$S/requested"
check "history recorded" grep -q 'updated v1.0.0 -> v1.1.0' "$S/history.log"

# 3. a request with nothing newer is a no-op
: > "$TMP/docker.log"; touch "$S/requested"; update
check "nothing newer: no docker run (beyond the health check)" test -z "$(ran)"
check "nothing newer: marker consumed" test ! -e "$S/requested"

# 3b. without HTTPS no caddy reload; with HTTPS on, the update restarts and
#     reloads caddy (a changed Caddyfile isn't noticed by compose)
check "no HTTPS: no caddy reload" bash -c "! grep -q 'caddy reload' '$TMP/docker.log'"
(cd "$TMP/src" && echo https > compose.yml && git_ commit -qam v1.1.1 && git tag v1.1.1)
printf 'surrealdb\nbackend\nfrontend\ncaddy\nupdater\n' > "$TMP/services"
: > "$TMP/docker.log"; touch "$S/requested"; update
check "HTTPS on: caddy restarted with the stack" grep -q 'up -d --remove-orphans .*caddy' "$TMP/docker.log"
check "HTTPS on: Caddyfile reloaded" grep -q 'exec -T caddy caddy reload --config /etc/caddy/Caddyfile' "$TMP/docker.log"
check "HTTPS on: updater still not restarted by itself" bash -c "! grep -q 'up -d --remove-orphans.*updater' '$TMP/docker.log'"
printf 'surrealdb\nbackend\nfrontend\nupdater\n' > "$TMP/services"

# 4. local edits to tracked files are reported, not clobbered
(cd "$TMP/src" && echo three > compose.yml && git_ commit -qam v1.2 && git tag v1.2.0)
echo mine > "$TMP/repo/compose.yml"
touch "$S/requested"; update
check "refuses over local changes" test "$(field error)" != none
check "error says why" grep -q 'local changes' "$S/status.json"
check "local edit survives" test "$(cat "$TMP/repo/compose.yml")" = mine
(cd "$TMP/repo" && git checkout -q compose.yml)

# 4b. SurrealDB 3 hook: a release pinning 3.x over a running 2.x runs the upgrade
# script before pulling, and a failed upgrade leaves the install on the old release
(cd "$TMP/src" && mkdir -p scripts && printf '#!/bin/sh\necho UPGRADE >> "%s/docker.log"\nexit $(cat "%s/upgrade_rc")\n' "$TMP" "$TMP" > scripts/upgrade-surreal-v3.sh \
  && echo four > compose.yml && git_ add -A && git_ commit -qm v1.3 && git tag v1.3.0)
echo surrealdb/surrealdb:v3.3.0 > "$TMP/images"; echo surrealdb/surrealdb:v2.7.0 > "$TMP/running"
# an old install: no ENCRYPTION_KEY, no JWT_SECRET yet
sed -i.bak -e '/^ENCRYPTION_KEY/d' -e '/^JWT_SECRET=/d' "$TMP/repo/.env" && rm -f "$TMP/repo/.env.bak"
echo 1 > "$TMP/upgrade_rc"; : > "$TMP/docker.log"; touch "$S/requested"; update
check "failed upgrade: ran before anything else" grep -qx UPGRADE "$TMP/docker.log"
check "failed upgrade: nothing pulled" bash -c "! grep -q 'compose pull' '$TMP/docker.log'"
check "failed upgrade: checkout put back" test "$(cat "$TMP/repo/compose.yml")" = https
check "failed upgrade: .env back on the old release" grep -q '^EUNOMIA_IMAGE_TAG=v1.1.1$' "$TMP/repo/.env"
check "failed upgrade: no encryption key injected (the old release must still decrypt)" bash -c "! grep -q '^ENCRYPTION_KEY' '$TMP/repo/.env'"
check "failed upgrade: no JWT secret injected" bash -c "! grep -q '^JWT_SECRET=' '$TMP/repo/.env'"
check "failed upgrade: status says why" grep -q 'upgrade failed' "$S/status.json"
# same failure on an install whose .env has no EUNOMIA_IMAGE_TAG: the line must not be left behind
sed -i.bak '/^EUNOMIA_IMAGE_TAG=/d' "$TMP/repo/.env" && rm -f "$TMP/repo/.env.bak"
echo 1 > "$TMP/upgrade_rc"; : > "$TMP/docker.log"; touch "$S/requested"; update
check "failed upgrade without a tag in .env: none written back" bash -c "! grep -q '^EUNOMIA_IMAGE_TAG=' '$TMP/repo/.env'"
echo 0 > "$TMP/upgrade_rc"; : > "$TMP/docker.log"; touch "$S/requested"; update
check "upgrade runs once, before the pull" bash -c "[ \$(grep -c UPGRADE '$TMP/docker.log') = 1 ] && [ \$(grep -n UPGRADE '$TMP/docker.log' | cut -d: -f1) -lt \$(grep -n 'compose pull' '$TMP/docker.log' | cut -d: -f1) ]"
check "upgraded release is applied" test "$(cat "$TMP/repo/compose.yml")" = four
check "after a good upgrade the encryption key and legacy flag appear" bash -c "grep -Eq '^ENCRYPTION_KEY=.{20,}$' '$TMP/repo/.env' && grep -q '^ENCRYPTION_KEY_LEGACY_EMPTY=1$' '$TMP/repo/.env'"
check "a missing JWT secret is generated" grep -Eq '^JWT_SECRET=.{20,}$' "$TMP/repo/.env"
KEY_AFTER_FIRST="$(sed -n 's/^ENCRYPTION_KEY=//p' "$TMP/repo/.env")"; JWT_AFTER="$(sed -n 's/^JWT_SECRET=//p' "$TMP/repo/.env")"
echo surrealdb/surrealdb:v3.3.0 > "$TMP/running"
(cd "$TMP/src" && echo five > compose.yml && git_ commit -qam v1.4 && git tag v1.4.0)
: > "$TMP/docker.log"; touch "$S/requested"; update
check "already on 3.x: no upgrade" bash -c "! grep -q UPGRADE '$TMP/docker.log'"
check "already on 3.x: still updates" test "$(cat "$TMP/repo/compose.yml")" = five
rm -f "$TMP/images" "$TMP/running"

# 4c. an existing key survives every later update, and a blank one is replaced
check "later updates keep the encryption key" test "$(sed -n 's/^ENCRYPTION_KEY=//p' "$TMP/repo/.env")" = "$KEY_AFTER_FIRST"
check "later updates keep the JWT secret" test "$(sed -n 's/^JWT_SECRET=//p' "$TMP/repo/.env")" = "$JWT_AFTER"
check "a missing secrets-helper key is generated (hex)" grep -Eq '^SECRETS_HELPER_KEY=[0-9a-f]{64}$' "$TMP/repo/.env"
SH_KEY="$(sed -n 's/^SECRETS_HELPER_KEY=//p' "$TMP/repo/.env")"
check "later updates add only one key line" test "$(grep -c '^ENCRYPTION_KEY=' "$TMP/repo/.env")" = 1
sed -i.bak 's/^ENCRYPTION_KEY=.*/ENCRYPTION_KEY=/' "$TMP/repo/.env" && rm -f "$TMP/repo/.env.bak"
(cd "$TMP/src" && echo six > compose.yml && git_ commit -qam v1.5 && git tag v1.5.0)
touch "$S/requested"; update
check "a blank encryption key is replaced" grep -Eq '^ENCRYPTION_KEY=.{20,}$' "$TMP/repo/.env"

# 5. GitHub unreachable is reported
rm -f "$S/status.json"; (cd "$TMP/repo" && git remote set-url origin "file://$TMP/nowhere")
update
check "unreachable origin is reported" grep -q 'could not reach GitHub' "$S/status.json"

# 2.x stopped (what an install looks like after an old updater broke it): the script must still be called
(cd "$TMP/repo" && git remote set-url origin "file://$TMP/src")
(cd "$TMP/src" && echo nine > compose.yml && git_ commit -qam v1.9 && git tag v1.9.0)
echo surrealdb/surrealdb:v3.3.0 > "$TMP/images"; rm -f "$TMP/running"
: > "$TMP/docker.log"; touch "$S/requested"; update
check "surrealdb not running: upgrade script still runs" grep -qx UPGRADE "$TMP/docker.log"
check "surrealdb not running: release applied after the upgrade" test "$(cat "$TMP/repo/compose.yml")" = nine
rm -f "$TMP/images" "$TMP/running"

# 6. a slow first boot: `up` fails while the backend is still starting; it is retried until it works
(cd "$TMP/src" && echo ten > compose.yml && git_ commit -qam v2.0 && git tag v2.0.0)
echo 2 > "$TMP/up_fails"; : > "$TMP/docker.log"; touch "$S/requested"; update
check "failed up: retried until it succeeds" test "$(grep -c 'compose up -d --remove-orphans' "$TMP/docker.log")" = 3
check "failed up then success: reported as updated" test "$(field current_version)" = v2.0.0 -a "$(field error)" = none -a "$(field applying)" = false
check "failed up then success: history recorded" grep -q 'updated v1.9.0 -> v2.0.0' "$S/history.log"
# it never comes up within the retry window: reported, once the tries run out
(cd "$TMP/src" && echo eleven > compose.yml && git_ commit -qam v2.1 && git tag v2.1.0)
echo 1000 > "$TMP/up_fails"; : > "$TMP/docker.log"; touch "$S/requested"
EUNOMIA_DIR="$TMP/repo" PATH="$TMP/bin:$PATH" UP_RETRY_DELAY=0 UP_RETRY_FOR=1 bash "$TMP/repo/scripts/auto-update.sh"
check "up never succeeds: retried, then given up" test "$(grep -c 'compose up -d --remove-orphans' "$TMP/docker.log")" -gt 1
check "up never succeeds: the error says why" grep -q 'restart failed: dependency failed to start' "$S/status.json"
check "up never succeeds: lock released" test ! -e "$S/.lock"
rm -f "$TMP/up_fails"

# 7. self-heal: after a failed update (the marker it leaves), backend healthy but the frontend stuck
# Created (what a failed `up` leaves) gets started, once
check "a failed update leaves the heal marker" test -e "$S/heal"
printf 'backend running starting\nfrontend created \n' > "$TMP/ps"
: > "$TMP/docker.log"; update
check "backend not healthy yet: nothing started" test -z "$(ran)"
check "backend not healthy yet: marker kept" test -e "$S/heal"
printf 'surrealdb running healthy\nbackend running healthy\nfrontend created \nsurreal-upgrade exited \nupdater running \n' > "$TMP/ps"
: > "$TMP/docker.log"; update
check "stuck frontend is started" grep -qx 'compose up -d frontend' "$TMP/docker.log"
check "the one-shot surreal-upgrade and the updater are left alone" bash -c "! grep -q 'up -d.*\(surreal-upgrade\|updater\)' '$TMP/docker.log'"
check "healed: marker cleared" test ! -e "$S/heal"
: > "$TMP/docker.log"; update
check "no marker: a service stopped by hand stays stopped" test -z "$(cat "$TMP/docker.log")"
rm -f "$TMP/ps"

# a release with the secrets-helper sidecar: its image is pulled too, and the key from before is kept
printf 'surrealdb\nbackend\nfrontend\nsecrets-helper\nupdater\n' > "$TMP/services"
(cd "$TMP/src" && echo sidecar > compose.yml && git_ commit -qam v2.2 && git tag v2.2.0)
: > "$TMP/docker.log"; touch "$S/requested"; update
check "the secrets-helper image is pulled with the others" grep -q 'compose pull backend frontend backup secrets-helper' "$TMP/docker.log"
check "the secrets-helper key is kept across updates" test "$(sed -n 's/^SECRETS_HELPER_KEY=//p' "$TMP/repo/.env")" = "$SH_KEY"
check "one secrets-helper key line" test "$(grep -c '^SECRETS_HELPER_KEY=' "$TMP/repo/.env")" = 1
printf 'surrealdb\nbackend\nfrontend\nupdater\n' > "$TMP/services"

# installs cloned from the old Qyrhal/Eunomia repo are moved to Cashara-io/eunomia
(cd "$TMP/repo" && git remote set-url origin https://github.com/Qyrhal/Eunomia.git)
rm -f "$S/status.json"; GIT_TERMINAL_PROMPT=0 GIT_ALLOW_PROTOCOL=file update
check "old origin repointed" test "$(git -C "$TMP/repo" remote get-url origin)" = https://github.com/Cashara-io/eunomia.git
echo "auto-update: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]

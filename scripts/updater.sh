#!/bin/sh
# Entrypoint of the `updater` compose service: runs scripts/auto-update.sh every
# 20s, so Settings → Updates works on any install with no host cron/launchd.
#
# It holds the docker socket but publishes no ports and takes no input except
# the marker files the backend drops in update-status/ ("check", "requested",
# "https.json"). The worst a compromised backend can do is ask it to install
# the newest release tag from GitHub, or to serve HTTPS for a (validated)
# domain name.
#
# Compose resolves bind mounts like ./update-status against the project
# directory and hands that path to the daemon, so the repo must sit at its
# real host path in here too: it's mounted at /eunomia and symlinked to the
# host path read from this container's own mount table.
set -u

apk add --no-cache -q bash git coreutils findutils curl python3 >/dev/null 2>&1 || { echo "updater: apk add failed, retrying on restart"; exit 1; }
git config --global --add safe.directory '*'

host="$(docker inspect "$HOSTNAME" --format '{{range .Mounts}}{{if eq .Destination "/eunomia"}}{{.Source}}{{end}}{{end}}' 2>/dev/null)"
[ -n "$host" ] || { echo "updater: can't see my own mounts (is the docker socket mounted?)"; exit 1; }
if [ "$host" != /eunomia ] && [ ! -e "$host" ]; then
  mkdir -p "$(dirname "$host")" && ln -s /eunomia "$host"
fi
owner="$(stat -c %u:%g /eunomia)"
# The backend runs as uid/gid 10001 (backend/Dockerfile) and drops marker files here: the host
# owner keeps the directory, the backend's group may write to it, nobody else can.
status_perms() { chown -R "${owner%%:*}:10001" /eunomia/update-status 2>/dev/null; chmod 2775 /eunomia/update-status 2>/dev/null; }
mkdir -p /eunomia/update-status && status_perms

while :; do
  (cd "$host" && bash scripts/auto-update.sh) >> /eunomia/update-status/update.log 2>&1
  # root in here; keep the checkout owned by whoever owns it on the host
  chown -R "$owner" /eunomia/.git /eunomia/.env 2>/dev/null
  status_perms
  git -C /eunomia ls-files -z | (cd /eunomia && xargs -0 chown -h "$owner" 2>/dev/null)
  tail -n 500 /eunomia/update-status/update.log > /tmp/log && cat /tmp/log > /eunomia/update-status/update.log
  sleep 20
done

#!/usr/bin/env bash
# Local dev without the compose stack: SurrealDB in Docker (bound to
# localhost only), backend via `cargo run`, frontend via `bun run dev`.
# The frontend proxies /api to the backend on :8001 (frontend/next.config.ts).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

DB=eunomia-dev-surrealdb
# NOTE: the dev volume eunomia-dev-surreal must hold 3.x data. If it was last used by a 2.x run of this
# script, remove it (docker volume rm eunomia-dev-surreal) or upgrade it first (docs/upgrading-to-surrealdb-3.md).
if [ -z "$(docker ps -q -f name="^${DB}$")" ]; then
  echo "==> starting SurrealDB ($DB, data in volume eunomia-dev-surreal)"
  docker run -d --rm --name "$DB" --user root -p 127.0.0.1:8000:8000 -v eunomia-dev-surreal:/data \
    surrealdb/surrealdb:v3.3.5 start --user root --pass root rocksdb:/data/eunomia.db >/dev/null
fi
until docker exec "$DB" /surreal isready --endpoint http://localhost:8000 >/dev/null 2>&1; do sleep 1; done

[ -d frontend/node_modules ] || (cd frontend && bun install)

trap 'kill 0' EXIT INT TERM
(cd backend && cargo run) &
(cd frontend && bun run dev) &
echo "==> Eunomia dev on http://localhost:3000 (ctrl-c to stop; '$DB' keeps running -- docker stop $DB)"
wait

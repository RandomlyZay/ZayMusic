#!/usr/bin/env bash
# Local dev bring-up: navidrome, zayfetch, browser and the API gateway via the
# dev override. Data and dev.env stay in the checkout this script lives in.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if [ ! -f dev.env ]; then
  echo "dev.env missing — copying from dev.env.example"
  cp dev.env.example dev.env
fi

mkdir -p dev-music dev-navidrome dev-zayfetch/staging

compose() { docker compose -f docker-compose.yml -f docker-compose.dev.yml --env-file dev.env "$@"; }

compose up -d --build

echo "containers up. waiting briefly for navidrome…"
sleep 3

echo "bootstrapping navidrome admin (idempotent)…"
compose run --rm zayfetch python -m zayfetch.bootstrap_admin || {
    echo "bootstrap_admin failed (navidrome may still be starting); re-run:" >&2
    echo "  docker compose -f docker-compose.yml -f docker-compose.dev.yml --env-file dev.env run --rm zayfetch python -m zayfetch.bootstrap_admin" >&2
  }

echo "gateway: http://127.0.0.1:${APP_PORT:-3847}/healthz (point native clients at this server)"

#!/usr/bin/env bash
# Run on devbox. No GitHub runner time, real credentials, or production services.
set -euo pipefail
cd "$(dirname "$0")/.."
SCOPE=${1:-all}
case "$SCOPE" in all|native|gateway|services) ;; *) echo 'Usage: scripts/native-local-checks.sh [all|native|gateway|services]' >&2; exit 2 ;; esac
if [[ "$SCOPE" == all || "$SCOPE" == native ]]; then
  cargo test --locked -p zay-core
  ./scripts/native-check-swift.sh
  ./scripts/native-linux-handoff.sh
  ./scripts/native-linux-playlists.sh
  ./scripts/native-linux-management.sh
fi
if [[ "$SCOPE" == all || "$SCOPE" == gateway ]]; then
  cargo test --locked -p zay-gateway
  cargo clippy --locked -p zay-gateway --all-targets -- -D warnings
  docker build --file crates/zay-gateway/Dockerfile --tag zaymusic-gateway-preflight .
  NATIVE_GATEWAY_IMAGE=zaymusic-gateway-preflight ./scripts/native-gateway-fixture-smoke.sh
fi
if [[ "$SCOPE" == all || "$SCOPE" == services ]]; then
  docker build --tag zayfetch-native-preflight services/zayfetch
  docker run --rm --volume "$PWD/services/zayfetch/tests:/app/tests:ro,z" \
    zayfetch-native-preflight sh -c 'uv sync --locked --extra dev --python /usr/local/bin/python && uv run --locked pytest'
fi
printf 'Local %s checks passed. No GitHub Actions run was requested.\n' "$SCOPE"

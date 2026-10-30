#!/usr/bin/env bash
# Polls https://fedora-server.tailfc5dc.ts.net/healthz until it reports EXPECT,
# or times out after 5 minutes.
set -euo pipefail
URL="${ZAY_HEALTH_URL:-https://fedora-server.tailfc5dc.ts.net/healthz}"
EXPECT="${1:-}"
[ -n "$EXPECT" ] || { echo "usage: wait-healthy.sh <sha-or-slug>" >&2; exit 2; }
for i in $(seq 1 60); do
  body="$(curl -fsS --max-time 5 "$URL" 2>/dev/null || true)"
  if grep -q "$EXPECT" <<<"$body"; then echo "healthy: $body"; exit 0; fi
  echo "  [$i/60] waiting for '$EXPECT' … last: ${body:-<none>}"
  sleep 5
done
echo "timed out waiting for '$EXPECT' at $URL" >&2
exit 1

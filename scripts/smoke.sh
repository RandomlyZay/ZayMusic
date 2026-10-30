#!/usr/bin/env bash
# Post-deploy smoke checks. Run from devbox against production URL.
# None of these assume a populated library — they must pass against an empty install.
set -uo pipefail

BASE="${SMOKE_BASE:-https://fedora-server.tailfc5dc.ts.net}"
PASS=0; FAIL=0; SKIP=0

check() {
  local name="$1"; shift
  if "$@" >/dev/null 2>&1; then
    echo "PASS  $name"; PASS=$((PASS+1))
  else
    echo "FAIL  $name"; FAIL=$((FAIL+1))
  fi
}

# 1. healthz
body="$(curl -fsS --max-time 10 "$BASE/healthz" 2>/dev/null || true)"
if grep -q '"ok":true' <<<"$body" && grep -q '"sha"' <<<"$body"; then
  echo "PASS  healthz: $body"; PASS=$((PASS+1))
else
  echo "FAIL  healthz: ${body:-<none>}"; FAIL=$((FAIL+1))
fi

# 2. /rest/ping via proxy (Navidrome credentials are injected server-side)
pingbody="$(curl -fsS --max-time 10 "$BASE/rest/ping?u=x" 2>/dev/null || true)"
if grep -q '"status":"ok"' <<<"$pingbody"; then
  echo "PASS  rest/ping via proxy"; PASS=$((PASS+1))
else
  echo "FAIL  rest/ping: ${pingbody:-<none>}"; FAIL=$((FAIL+1))
fi

# 3. ZayFetch health (the gateway injects the service token)
fh="$(curl -fsS --max-time 10 "$BASE/api/fetch/healthz" 2>/dev/null || true)"
if grep -q 'yt_dlp' <<<"$fh"; then
  echo "PASS  api/fetch/healthz: $fh"; PASS=$((PASS+1))
else
  echo "FAIL  api/fetch/healthz: ${fh:-<none>}"; FAIL=$((FAIL+1))
fi

# 3b. native settings config + browser cross-site refusal (no upstream call)
nc="$(curl -fsS --max-time 10 "$BASE/api/native/config" 2>/dev/null || true)"
if grep -q '"remoteBrowserUrl"' <<<"$nc"; then
  echo "PASS  api/native/config"; PASS=$((PASS+1))
else
  echo "FAIL  api/native/config: ${nc:-<none>}"; FAIL=$((FAIL+1))
fi
xs="$(curl -sS --max-time 10 -o /dev/null -w '%{http_code}' -X POST \
  -H 'Origin: https://cross-site.invalid' "$BASE/rest/ping" 2>/dev/null || true)"
if [ "$xs" = 403 ]; then
  echo "PASS  cross-site browser request refused"; PASS=$((PASS+1))
else
  echo "FAIL  cross-site request returned ${xs:-<none>}"; FAIL=$((FAIL+1))
fi

# 4. resolve a known stable video (network-dependent → SKIP on failure)
RESOLVE_URL="https://www.youtube.com/watch?v=dQw4w9WgXcQ"
rh="$(curl -fsS --max-time 20 \
  --get --data-urlencode "url=$RESOLVE_URL" "$BASE/api/fetch/resolve" 2>/dev/null || true)"
if [ -z "$rh" ]; then
  echo "SKIP  resolve (network unavailable)"; SKIP=$((SKIP+1))
elif grep -q '"kind"' <<<"$rh" && grep -q '"candidates"' <<<"$rh"; then
  echo "PASS  resolve shape"; PASS=$((PASS+1))
else
  echo "FAIL  resolve: $rh"; FAIL=$((FAIL+1))
fi

# 5. search shape
sh="$(curl -fsS --max-time 20 \
  --get --data-urlencode "q=lorem ipsum zzzqqq" "$BASE/api/fetch/search" 2>/dev/null || true)"
if grep -q '"candidates"' <<<"$sh"; then
  echo "PASS  search shape"; PASS=$((PASS+1))
else
  echo "FAIL  search shape: ${sh:-<none>}"; FAIL=$((FAIL+1))
fi

# 6. HTTPS cert + no stray host ports
if curl -fsS --max-time 10 --head "$BASE/" >/dev/null 2>&1; then
  echo "PASS  https reachable with valid (Tailscale) cert"; PASS=$((PASS+1))
else
  echo "FAIL  https unreachable"; FAIL=$((FAIL+1))
fi
for port in 8080 4533 8081; do
  if curl -fsS --max-time 3 --head "http://fedora-server.tailfc5dc.ts.net:$port/" >/dev/null 2>&1; then
    echo "FAIL  port $port unexpectedly reachable"; FAIL=$((FAIL+1))
  else
    echo "PASS  port $port not exposed"; PASS=$((PASS+1))
  fi
done

echo "---"
echo "pass=$PASS fail=$FAIL skip=$SKIP"
[ "$FAIL" -eq 0 ]

#!/usr/bin/env bash
# Run the API gateway in front of the loopback fixture with fake credentials only.
# NATIVE_GATEWAY_IMAGE=<tag> tests a built image; otherwise the release binary.
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p build
TASK_DATA=$(mktemp -d)
TASK_CONTAINER=''
cleanup() {
  if [[ -n "$TASK_CONTAINER" ]]; then docker rm -f "$TASK_CONTAINER" >/dev/null 2>&1 || true; fi
  kill "${APP_PID:-}" "${FIXTURE_PID:-}" 2>/dev/null || true
  rm -rf "$TASK_DATA"
}
trap cleanup EXIT
python3 scripts/native-fixture-server.py --port 0 --ready-file "$TASK_DATA/api-url" --management-fixtures \
  > build/gateway-fixture-api.log 2>&1 &
FIXTURE_PID=$!
for _ in {1..50}; do [[ -s "$TASK_DATA/api-url" ]] && break; sleep 0.1; done
FIXTURE_URL=$(cat "$TASK_DATA/api-url")
TASK_PORT=$(python3 -c 'import socket; s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1]);s.close()')
FAKE_ENV=(HOST=127.0.0.1 PORT="$TASK_PORT" NAVIDROME_URL="$FIXTURE_URL" ZAYFETCH_URL="$FIXTURE_URL"
  NAVIDROME_USER=fake-test NAVIDROME_PASS=fake-test ZAYFETCH_TOKEN=fake-test BUILD_SHA=fixture
  PUBLIC_REMOTE_BROWSER_URL=https://fixture.invalid:8446/vnc.html)
if [[ -n "${NATIVE_GATEWAY_IMAGE:-}" ]]; then
  TASK_CONTAINER="zaymusic-gateway-fixture-$$"
  docker run --rm --name "$TASK_CONTAINER" --network host "${FAKE_ENV[@]/#/--env=}" \
    "$NATIVE_GATEWAY_IMAGE" > build/gateway-fixture-app.log 2>&1 &
else
  cargo build --locked --release -p zay-gateway
  env "${FAKE_ENV[@]}" target/release/zay-gateway > build/gateway-fixture-app.log 2>&1 &
fi
APP_PID=$!
python3 - "http://127.0.0.1:$TASK_PORT" <<'PY'
import json, sys, time, urllib.error, urllib.request
base = sys.argv[1]

def call(path, data=None, headers=None, method=None):
    request = urllib.request.Request(base + path, data=data, headers=headers or {}, method=method)
    try:
        with urllib.request.urlopen(request, timeout=10) as response:
            return response.status, response.headers, response.read()
    except urllib.error.HTTPError as failure:
        return failure.code, failure.headers, failure.read()

for _ in range(100):
    try:
        if call('/healthz')[0] == 200:
            break
    except OSError:
        time.sleep(0.1)
else:
    raise SystemExit('Gateway fixture app did not start')

def check(name, condition):
    if not condition:
        raise SystemExit(f'FAIL {name}')
    print(f'PASS {name}')

status, _, body = call('/healthz')
check('healthz', status == 200 and json.loads(body) | {'uptimeSec': 0} == {'ok': True, 'sha': 'fixture', 'uptimeSec': 0})
check('native config', json.loads(call('/api/native/config')[2]) == {'remoteBrowserUrl': 'https://fixture.invalid:8446/vnc.html'})
check('rest ping', json.loads(call('/rest/ping?u=forged')[2])['subsonic-response']['status'] == 'ok')
status, headers, body = call('/rest/getCoverArt?id=al-fixture-1&size=32')
check('cover art', status == 200 and headers['content-type'] == 'image/png' and body.startswith(b'\x89PNG'))
status, headers, body = call('/rest/stream?id=fixture-1')
check('stream', status == 200 and headers['content-type'] == 'audio/wav' and len(body) == int(headers['content-length']))
check('fetch jobs', call('/api/fetch/jobs')[0] == 200)
form = b'id=fixture-1&id=fixture-2&u=forged'
status, _, _ = call('/rest/star', form, {'content-type': 'application/x-www-form-urlencoded'})
events = json.loads(call('/api/fetch/__fixture/state')[2])['events']
check('repeated form fields', status == 200 and {'star:song:fixture-1', 'star:song:fixture-2'} <= set(events))
check('cross-site refused', call('/rest/star', form, {'origin': 'https://evil.invalid'})[0] == 403)
check('traversal refused', call('/api/fetch/%2e%2e/rest/ping')[0] == 400)
PY

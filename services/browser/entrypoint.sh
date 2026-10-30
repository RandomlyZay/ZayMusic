#!/bin/bash
# Starts the one browser this deployment owns: Xvfb -> x11vnc -> noVNC -> Chromium.
# On stop, ask Chromium to flush its profile and quit through CDP.
set -euo pipefail

PROFILE_DIR="${BROWSER_PROFILE_DIR:-/data/profile}"
DISPLAY_NUM="${DISPLAY_NUM:-99}"
# Chromium only ever binds CDP to loopback, so socat bridges the container
# network's :CDP_PORT to it (newer Chromium ignores --remote-debugging-address).
CDP_PORT="${CDP_PORT:-9222}"
CDP_INTERNAL_PORT="${CDP_INTERNAL_PORT:-9223}"
VNC_PORT="${VNC_PORT:-5900}"
NOVNC_PORT="${NOVNC_PORT:-6080}"

export DISPLAY=":${DISPLAY_NUM}"

# `docker stop`/`docker start` reuses the writable layer, so the X lock from
# the previous run can still be present and Xvfb would refuse to start.
rm -f "/tmp/.X${DISPLAY_NUM}-lock" "/tmp/.X11-unix/X${DISPLAY_NUM}"

mkdir -p "$PROFILE_DIR"

# A previous container left a SingletonLock naming its old hostname; Chromium
# would otherwise refuse to start ("profile appears to be in use"). Exactly one
# Chromium runs per container, so clearing the stale lock is safe.
rm -f "$PROFILE_DIR/SingletonLock" "$PROFILE_DIR/SingletonSocket" "$PROFILE_DIR/SingletonCookie"

Xvfb "$DISPLAY" -screen 0 1280x900x24 -nolisten tcp &
XVFB_PID=$!

# Wait for the X server before starting its clients.
for _ in $(seq 1 100); do
    if xdpyinfo -display "$DISPLAY" >/dev/null 2>&1; then
        break
    fi
    if ! kill -0 "$XVFB_PID" 2>/dev/null; then
        echo "Xvfb exited before the display came up" >&2
        exit 1
    fi
    sleep 0.2
done

x11vnc -display "$DISPLAY" -forever -shared -nopw -rfbport "$VNC_PORT" -quiet &
VNC_PID=$!
websockify --web=/usr/share/novnc/ "$NOVNC_PORT" "localhost:${VNC_PORT}" &
NOVNC_PID=$!
socat "TCP-LISTEN:${CDP_PORT},fork,reuseaddr,bind=0.0.0.0" "TCP:127.0.0.1:${CDP_INTERNAL_PORT}" &
CDP_PROXY_PID=$!

CHROMIUM_PID=""
shutdown() {
    trap '' TERM INT
    if [[ -n "$CHROMIUM_PID" ]] && kill -0 "$CHROMIUM_PID" 2>/dev/null; then
        # SIGTERM alone does not flush newly issued cookies. Browser.close
        # runs Chromium's normal exit path before Docker removes the container.
        python3 /usr/local/bin/close-browser.py "$CDP_INTERNAL_PORT" \
            || kill -TERM "$CHROMIUM_PID" 2>/dev/null || true
        wait "$CHROMIUM_PID" 2>/dev/null || true
    fi
    kill "$CDP_PROXY_PID" "$NOVNC_PID" "$VNC_PID" "$XVFB_PID" 2>/dev/null || true
    wait 2>/dev/null || true
}
trap 'shutdown; exit 0' TERM INT

# Restore the session (including session-only cookies) after a restart.
# There is no desktop keyring in this container; use a stable password store
# so the persisted profile remains readable when the container is recreated.
# The shell supervises Chromium; Docker's init forwards stop signals and reaps
# orphaned children. --no-sandbox is mandatory and
# --disable-dev-shm-usage avoids the 64 MB /dev/shm crash.
chromium \
    --user-data-dir="$PROFILE_DIR" \
    --restore-last-session \
    --password-store=basic \
    --remote-debugging-port="$CDP_INTERNAL_PORT" \
    --no-first-run \
    --no-default-browser-check \
    --no-sandbox \
    --test-type \
    --disable-dev-shm-usage \
    --disable-gpu \
    https://music.youtube.com &
CHROMIUM_PID=$!

status=0
wait "$CHROMIUM_PID" || status=$?
shutdown
exit "$status"

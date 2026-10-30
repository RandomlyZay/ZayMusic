#!/usr/bin/env bash
# Exercise native dialogs/menus against an isolated loopback fixture, without account data.
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p build/native-screenshots
if [[ "${ZAY_PLAYLIST_INNER:-}" != 1 ]]; then
  cargo build --locked --release -p zaymusic-linux
  exec dbus-run-session -- xvfb-run -a -s '-screen 0 1280x900x24' env ZAY_PLAYLIST_INNER=1 "$0"
fi
TASK_DATA=$(mktemp -d)
trap 'kill "${APP_PID:-}" "${FIXTURE_PID:-}" 2>/dev/null || true; rm -rf "$TASK_DATA"' EXIT
python3 scripts/native-fixture-server.py --port 0 --ready-file "$TASK_DATA/url" > build/native-playlist-fixtures.log 2>&1 &
FIXTURE_PID=$!
for _ in {1..50}; do [[ -s "$TASK_DATA/url" ]] && break; sleep 0.1; done
FIXTURE_URL=$(cat "$TASK_DATA/url")
ZAY_UI_READY_FILE="$TASK_DATA/ui-ready" ZAY_SILENT=1 XDG_DATA_HOME="$TASK_DATA" GDK_BACKEND=x11 GSK_RENDERER=cairo ZAY_SERVER="$FIXTURE_URL" target/release/zaymusic-linux > build/native-playlists.log 2>&1 &
APP_PID=$!
wait_page() {
  python3 - "$TASK_DATA/ui-ready" "$1" <<'PY'
import json,pathlib,sys,time
path=pathlib.Path(sys.argv[1]); end=time.monotonic()+15
while time.monotonic()<end:
    try:
        value=json.loads(path.read_text())
        if value.get('page')==sys.argv[2] and value.get('loading') is False: break
    except (OSError,ValueError): pass
    time.sleep(0.05)
else: raise SystemExit('Playlist UI did not settle')
PY
  sleep 0.2
}
expect_playlist() {
  python3 - "$FIXTURE_URL" "$1" "$2" <<'PY'
import json,sys,time,urllib.request
end=time.monotonic()+10
while time.monotonic()<end:
    try:
        data=json.load(urllib.request.urlopen(sys.argv[1]+'/rest/getPlaylists',timeout=1))['subsonic-response']['playlists']['playlist']
        found=next((p for p in data if p['name']==sys.argv[2]),None)
        if sys.argv[3]=='absent' and found is None: break
        if found is not None and (sys.argv[3]=='present' or [s['id'] for s in found['entry']]==json.loads(sys.argv[3])): break
    except (OSError,ValueError): pass
    time.sleep(0.1)
else: raise SystemExit('Playlist UI action did not update the fixture')
PY
}
named() { python3 scripts/native-linux-control.py "$TASK_DATA/ui-ready" "$WINDOW" "$@"; }
click() { xdotool mousemove --window "$WINDOW" "$1" "$2" click 1; sleep 0.35; }
# Adwaita dialogs are presented inside the main window; their entry has focus.
name_dialog() {
  sleep 0.5
  xdotool key ctrl+a
  xdotool type --clearmodifiers "$2"
  import -window "$WINDOW" "build/native-screenshots/linux-playlist-name-$1.png"
  xdotool key Return
}
# Window-relative positions for the 1100x760 layout (sidebar rows, list rows, row menus).
SIDEBAR_PLAYLISTS='90 226'
FIRST_ROW_Y=255
SECOND_ROW_Y=323
ROW_MENU_X=1040
wait_page Home
WINDOW=$(xdotool search --name '^ZayMusic$' | head -1)
import -window "$WINDOW" build/native-screenshots/linux-home-fixture.png
click $SIDEBAR_PLAYLISTS
wait_page Playlists
click 375 182
name_dialog new 'Native mix'
expect_playlist 'Native mix' present
wait_page Playlists
click 600 $SECOND_ROW_Y
wait_page Collection
# An empty playlist shows only the collection menu under its header.
click 307 308
import -window root build/native-screenshots/linux-playlist-collection-menu.png
click 307 363
name_dialog rename 'Renamed mix'
expect_playlist 'Renamed mix' present
wait_page Collection
click $SIDEBAR_PLAYLISTS
wait_page Playlists
click 600 $FIRST_ROW_Y
wait_page Collection
import -window "$WINDOW" build/native-screenshots/linux-playlist-collection.png
click $ROW_MENU_X 451
import -window root build/native-screenshots/linux-playlist-track-menu.png
named "Move up in playlist" --menu
expect_playlist 'Fixture playlist' '["fixture-2","fixture-1","fixture-3"]'
wait_page Collection
click $ROW_MENU_X 395
import -window root build/native-screenshots/linux-playlist-first-menu.png
named "Remove from playlist" --menu
expect_playlist 'Fixture playlist' '["fixture-1","fixture-3"]'
wait_page Collection
import -window "$WINDOW" build/native-screenshots/linux-playlist-tracks.png
click $ROW_MENU_X 395
named "Add to playlist" --menu
sleep 1
import -window "$WINDOW" build/native-screenshots/linux-playlist-picker.png
click 580 365
expect_playlist 'Renamed mix' '["fixture-1"]'
click $SIDEBAR_PLAYLISTS
wait_page Playlists
click 600 $SECOND_ROW_Y
wait_page Collection
click 499 323
import -window root build/native-screenshots/linux-playlist-delete-menu.png
click 508 406
sleep 0.6
import -window "$WINDOW" build/native-screenshots/linux-playlist-delete.png
click 634 430
expect_playlist 'Renamed mix' absent
wait_page Playlists
import -window "$WINDOW" build/native-screenshots/linux-playlists-finished.png
echo 'Native playlist create/rename/reorder/remove/add/delete dialogs and menus passed'

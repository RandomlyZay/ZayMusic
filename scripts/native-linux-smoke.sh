#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p build/native-screenshots
if [[ "${ZAY_HEADLESS_INNER:-}" != 1 ]]; then
  cargo build --locked --release -p zaymusic-linux
  exec dbus-run-session -- xvfb-run -a -s '-screen 0 1280x900x24' env ZAY_HEADLESS_INNER=1 "$0"
fi
TASK_DATA=$(mktemp -d)
trap 'kill "${APP_PID:-}" 2>/dev/null || true; rm -rf "$TASK_DATA"' EXIT
ZAY_UI_READY_FILE="$TASK_DATA/ui-ready" ZAY_SILENT=1 XDG_DATA_HOME="$TASK_DATA" GDK_BACKEND=x11 GSK_RENDERER=cairo ZAY_SERVER=http://127.0.0.1:3847 target/release/zaymusic-linux > build/native-linux.log 2>&1 &
APP_PID=$!
wait_page() {
  python3 - "$TASK_DATA/ui-ready" "$1" <<'PYWAIT'
import json,pathlib,sys,time
path=pathlib.Path(sys.argv[1]); expected=sys.argv[2]; end=time.monotonic()+20
while time.monotonic()<end:
    try:
        value=json.loads(path.read_text())
        if value.get('page')==expected and value.get('loading') is False: break
    except (OSError,ValueError): pass
    time.sleep(0.1)
else: raise SystemExit(f'Native page did not settle: {expected}')
PYWAIT
}
wait_page Home
kill -0 "$APP_PID"
WINDOW=$(xdotool search --name '^ZayMusic$' | head -1)
import -window "$WINDOW" build/native-screenshots/linux-home.png
# Open the stable Liked Songs hero, then play its first row.
xdotool mousemove --window "$WINDOW" 90 188 click 1
wait_page "Liked Songs"
xdotool mousemove --window "$WINDOW" 600 395 click 1
sleep 3
PLAYBACK_STATUS=$(gdbus call --session --dest org.mpris.MediaPlayer2.zaymusic --object-path /org/mpris/MediaPlayer2 --method org.freedesktop.DBus.Properties.Get org.mpris.MediaPlayer2.Player PlaybackStatus)
[[ "$PLAYBACK_STATUS" == *Playing* ]]
gdbus call --session --dest org.mpris.MediaPlayer2.zaymusic --object-path /org/mpris/MediaPlayer2 --method org.freedesktop.DBus.Properties.Get org.mpris.MediaPlayer2.Player Position | python3 -c 'import re,sys; value=re.search(r"int64 (\d+)",sys.stdin.read()); assert value and int(value[1]) > 0, "Playback clock did not advance"'
import -window "$WINDOW" build/native-screenshots/linux-playing.png
gdbus call --session --dest org.mpris.MediaPlayer2.zaymusic --object-path /org/mpris/MediaPlayer2 --method org.mpris.MediaPlayer2.Player.Pause
# Exercise sidebar navigation and queue controls without routing sound to speakers.
# Sidebar rows are window-relative y positions in the 1100x760 layout.
for entry in '188|Liked Songs|Liked' '264|Albums|Albums' '302|Artists|Artists' '226|Playlists|Playlists' '340|Downloads|Downloads' '378|Queue|Queue' '568|Settings|Settings' '454|New music|NewMusic'; do
  IFS='|' read -r ROW_Y PAGE FILE <<< "$entry"
  xdotool mousemove --window "$WINDOW" 90 "$ROW_Y" click 1
  wait_page "$PAGE"
  import -window "$WINDOW" "build/native-screenshots/linux-$FILE.png"
done
gdbus call --session --dest org.mpris.MediaPlayer2.zaymusic --object-path /org/mpris/MediaPlayer2 --method org.freedesktop.DBus.Properties.Get org.mpris.MediaPlayer2 Identity
gdbus call --session --dest org.mpris.MediaPlayer2.zaymusic --object-path /org/mpris/MediaPlayer2 --method org.mpris.MediaPlayer2.Player.PlayPause
kill -0 "$APP_PID"
echo 'Headless startup, navigation and MPRIS calls completed; screenshots require review.'

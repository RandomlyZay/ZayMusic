#!/usr/bin/env bash
# Drive native settings, jobs, imports, mixed search, saves and the queue dialog against the
# loopback --management-fixtures server. Fake data only; every UI action is checked against
# fixture or catalog state, and screenshots record what was on screen.
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p build/native-screenshots
BIN="${CARGO_TARGET_DIR:-target}/release/zaymusic-linux"
if [[ "${ZAY_MANAGEMENT_INNER:-}" != 1 ]]; then
  cargo build --locked --release -p zaymusic-linux
  exec dbus-run-session -- xvfb-run -a -s '-screen 0 1280x900x24' env ZAY_MANAGEMENT_INNER=1 "$0"
fi
TASK_DATA=$(mktemp -d)
trap 'kill "${APP_PID:-}" "${FIXTURE_PID:-}" 2>/dev/null || true; rm -rf "$TASK_DATA"' EXIT
python3 scripts/native-fixture-server.py --port 0 --management-fixtures --ready-file "$TASK_DATA/url" > build/native-management-fixtures.log 2>&1 &
FIXTURE_PID=$!
for _ in {1..50}; do [[ -s "$TASK_DATA/url" ]] && break; sleep 0.1; done
FIXTURE_URL=$(cat "$TASK_DATA/url")
ZAY_UI_READY_FILE="$TASK_DATA/ui-ready" ZAY_SILENT=1 XDG_DATA_HOME="$TASK_DATA" GDK_BACKEND=x11 GSK_RENDERER=cairo ZAY_SERVER="$FIXTURE_URL" "$BIN" > build/native-management.log 2>&1 &
APP_PID=$!
SHOTS=build/native-screenshots
KNOWN_FAILURES=()
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
else: raise SystemExit(f'{sys.argv[2]} did not settle')
PY
  sleep 0.3
}
# Poll fixture state `s` until the Python expression holds.
expect() {
  python3 - "$FIXTURE_URL" "$1" "$2" <<'PY'
import json,sys,time,urllib.request
end=time.monotonic()+10
while True:
    s=json.load(urllib.request.urlopen(sys.argv[1]+'/__fixture/state',timeout=2))
    if eval(sys.argv[2]): break
    if time.monotonic()>end: raise SystemExit(f'FAILED: {sys.argv[3]}\nstate: {json.dumps(s)[:2000]}')
    time.sleep(0.1)
PY
}
# Poll the app's persisted queue: q (raw), ids (main queue), cur (current id), pri (priority ids).
expect_queue() {
  python3 - "$TASK_DATA/zaymusic/catalog.sqlite" "$1" "$2" <<'PY'
import json,sqlite3,sys,time
end=time.monotonic()+10
while True:
    with sqlite3.connect(sys.argv[1]) as db:
        q=json.loads(db.execute("SELECT value FROM kv WHERE key='queue'").fetchone()[0])
    ids=[s['id'] for s in q['queue']]
    cur=(q.get('activeSong') or {}).get('id') or (ids[q['index']] if 0<=q['index']<len(ids) else None)
    pri=[s['id'] for s in q['secondaryQueue']]
    if eval(sys.argv[2]): break
    if time.monotonic()>end: raise SystemExit(f'FAILED: {sys.argv[3]}\nqueue: {ids} index {q["index"]} current {cur} priority {pri} shuffle {q["shuffleOrder"]}@{q["shufflePos"]}')
    time.sleep(0.1)
PY
}
known() { if ! expect "$1" "$2"; then KNOWN_FAILURES+=("$2"); fi; }
named() { python3 scripts/native-linux-control.py "$TASK_DATA/ui-ready" "$WINDOW" "$@"; }
click() { xdotool mousemove --window "$WINDOW" "$1" "$2" click 1; sleep 0.4; }
shot() { import -window "$WINDOW" "$SHOTS/linux-management-$1.png"; }
shot_root() { import -window root "$SHOTS/linux-management-$1.png"; }
scroll_main() { xdotool mousemove --window "$WINDOW" 660 420 click --repeat 30 --delay 15 5; sleep 0.6; }
# Lower sidebar entries (Jobs, Imports, YouTube Playlists) are only reachable once scrolled.
sidebar() { xdotool mousemove --window "$WINDOW" 120 400 click --repeat 15 --delay 15 5; sleep 0.4; click 90 "$1"; }
search() {
  click 670 28
  xdotool key ctrl+a
  xdotool type --clearmodifiers "$1"
  xdotool key Return
  wait_page Omnibar
}
menu() { click 1044 "$1"; sleep 0.3; click "$2" "$3"; }

wait_page Home
WINDOW=$(xdotool search --name '^ZayMusic$' | head -1)

# Settings: verify, manual cookies (multiline, never persisted), then the pending OAuth code.
click 90 568
wait_page Settings
shot settings
named "Verify connection" --scroll
shot settings-verify
expect "'cookies:verified' in s['events']" 'Verify connection reached the server'
sleep 3.5
scroll_main
named "Connect with cookies.txt…" --scroll
sleep 0.6
shot_root cookie-dialog
click 549 390
xdotool type --clearmodifiers '# Netscape HTTP Cookie File'
xdotool key Return
xdotool type --clearmodifiers $'fixture.invalid\tTRUE\t/\tTRUE\t0\tFAKE\tfake-cookie-value'
named "Connect"
expect "'cookies:saved' in s['events'] and s['cookieLines']==2" 'Multiline cookies were sent once'
wait_page Settings
sleep 3.5
scroll_main
named "Connect Google API" --scroll
expect "s['oauthPending'] and 'oauth:start' in s['events']" 'Connect Google API started device authorization'
wait_page Settings
scroll_main
shot settings-oauth
python3 - "$TASK_DATA" <<'PY'
import pathlib,sys
root=pathlib.Path(sys.argv[1])
for path in [*root.rglob('*'), pathlib.Path('build/native-management.log'), pathlib.Path('build/native-management-fixtures.log')]:
    if path.is_file():
        assert b'fake-cookie-value' not in path.read_bytes(), f'Cookie text persisted in {path}'
PY
if ! python3 - "$TASK_DATA/zaymusic/catalog.sqlite" <<'PY'
import sqlite3,sys
with sqlite3.connect(sys.argv[1]) as db:
    raise SystemExit(db.execute("SELECT count(*) FROM kv WHERE value LIKE '%FIX-CODE%'").fetchone()[0])
PY
then KNOWN_FAILURES+=('OAuth device code is persisted in the browse:Settings catalog cache'); fi

# Jobs: retry the failed job, cancel then dismiss the running one.
sidebar 494
wait_page Jobs
shot jobs
named "Retry"
expect "s['jobs'][0]['status']=='queued' and 'retry:fixture-job-failed' in s['events']" 'Retry requeued the failed job'
wait_page Jobs
named "Cancel" --last
expect "'cancel:fixture-job-running' in s['events']" 'Cancel reached the running job'
wait_page Jobs
shot jobs-cancelled
named "Dismiss"
named "Dismiss job"
expect "[j['id'] for j in s['jobs']]==['fixture-job-failed']" 'Dismiss removed only the cancelled job'
wait_page Jobs

# Imports: additive pull, track details, status refresh, then import a YouTube playlist.
sidebar 532
wait_page Imports
shot imports
named "Pull new songs"
expect "[t['title'] for t in s['imports'][0]['items']]==['Fixture import track','Fixture import missing','Fixture pulled track']" 'Pull added songs without removing any'
wait_page Imports
named "Track details"
sleep 0.6
shot import-details
xdotool key Escape
sleep 0.4
named "Refresh status"
expect "'sync:fixture-import' in s['events'] and s['imports'][0]['pending']==0" 'Refresh status synced the import'
sidebar 570
wait_page 'YouTube Playlists'
shot youtube-playlists
named "Import playlist"
expect "'import:PLfixture' in s['events'] and s['imports'][-1]['name']=='Fixture YouTube playlist'" 'Import playlist created an import'

# Mixed search: library songs, albums, an accent-insensitive artist, discovery songs and albums.
search Fixture
expect "['library','Fixture'] in s['searches'] and ['discovery','Fixture'] in s['searches']" 'Omnibar searched library and discovery'
shot search
scroll_main
shot search-sections
menu 175 1040 227
expect "'fixture-album-4' in s['starred']['album']" 'Saving an album from search starred it'
expect "len([q for k,q in s['searches'] if k=='library'])>=2" 'Search reloaded after saving'
known "[q for k,q in s['searches'] if k=='library'][-1]=='Fixture'" 'Reload after a save keeps the search query (main.rs RefreshCatalog loads with an empty query)'
search Fixture
scroll_main
menu 304 1043 356
expect "'fixture-artist-accent' in s['starred']['artist']" 'Following an artist from search starred it'
search Fixture
scroll_main
shot_root new-album-menu-before
menu 618 1040 670
expect "s['albumSaves']==[{'title':'Fixture discovery album','artist':'ZayMusic'}] and [e for e in s['events'] if e.startswith('job:fixtureAlb')]==['job:fixtureAlb1:','job:fixtureAlb2:','job:fixtureAlb3:']" 'Saving a new album acquired its tracks in order'
search Fixture
named "More options for Fixture song 1" --scroll
named "Remove from Liked Songs" --menu
expect "'fixture-1' not in s['starred']['song'] and 'unstar:song:fixture-1' in s['events']" 'Unlike removed the song save'

# Queue dialog: only songs are queued from mixed results; reorder, remove, priority and shuffle.
search Fixture
click 600 318
expect_queue "ids==['fixture-1','fixture-2','fixture-3','fixture-4','fixture-5','fixture-6','fixture-7','fixture-8'] and cur=='fixture-2'" 'Mixed results queue only songs and keep the selected song'
click 1010 713
sleep 0.6
shot queue
named "Move up: Fixture song 4"
expect_queue "ids[:5]==['fixture-1','fixture-2','fixture-4','fixture-3','fixture-5'] and cur=='fixture-2'" 'Move up reordered the upcoming song'
named "Move up: Fixture song 4"
sleep 0.5
expect_queue "ids[:5]==['fixture-1','fixture-2','fixture-4','fixture-3','fixture-5'] and cur=='fixture-2'" 'The first upcoming song cannot pass the current song'
named "Remove from queue: Fixture song 4"
expect_queue "'fixture-4' not in ids and cur=='fixture-2'" 'Remove dropped the upcoming song only'
xdotool key Escape
sleep 0.5
named "More options for Fixture song 1" --scroll
named "Play next" --menu
expect_queue "pri==['fixture-1'] and cur=='fixture-2'" 'Play next filled the priority lane'
click 449 695
expect_queue "q['shuffle'] and cur=='fixture-2'" 'Shuffle kept the current song'
python3 - "$TASK_DATA/zaymusic/catalog.sqlite" > "$TASK_DATA/before" <<'PY'
import json,sqlite3,sys
with sqlite3.connect(sys.argv[1]) as db:
    print(db.execute("SELECT value FROM kv WHERE key='queue'").fetchone()[0])
PY
click 1010 713
sleep 0.6
shot queue-shuffle
MOVE_TITLE=$(python3 - "$TASK_DATA/before" <<'PYTITLE'
import json,sys
q=json.load(open(sys.argv[1])); index=q['shuffleOrder'][q['shufflePos']+2]
print(q['queue'][index]['title'])
PYTITLE
)
named "Move up: $MOVE_TITLE"
expect_queue "(lambda b: ids==[s['id'] for s in b['queue']] and q['shuffleOrder'][:q['shufflePos']+1]==b['shuffleOrder'][:b['shufflePos']+1] and q['shuffleOrder'][q['shufflePos']+1:q['shufflePos']+3]==b['shuffleOrder'][b['shufflePos']+1:b['shufflePos']+3][::-1] and cur=='fixture-2')(json.load(open('$TASK_DATA/before')))" 'Shuffle move swapped play order without touching the list or current song'
shot queue-shuffle-moved
xdotool key Escape
sleep 0.4
shot finished
if ((${#KNOWN_FAILURES[@]})); then
  printf 'Native management smoke found %d product issue(s):\n' "${#KNOWN_FAILURES[@]}" >&2
  printf ' - %s\n' "${KNOWN_FAILURES[@]}" >&2
  exit 1
fi
echo 'Native settings, jobs, imports, mixed search, saves/follows and queue dialog passed'

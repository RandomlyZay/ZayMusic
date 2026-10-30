#!/usr/bin/env bash
# Generated temporary silence exercises native handoff without adding library audio.
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p build
if [[ "${ZAY_HANDOFF_INNER:-}" != 1 ]]; then
  cargo build --locked --release -p zaymusic-linux
  exec dbus-run-session -- xvfb-run -a -s '-screen 0 1280x900x24' env ZAY_HANDOFF_INNER=1 "$0"
fi
TASK_DATA=$(mktemp -d)
trap 'kill "${APP_PID:-}" 2>/dev/null || true; rm -rf "$TASK_DATA"' EXIT
python3 - "$TASK_DATA" <<'PY'
import json,pathlib,sqlite3,sys,wave
root=pathlib.Path(sys.argv[1]); app=root/'zaymusic';app.mkdir();(app/'audio').mkdir()
songs=[]
for index,name in enumerate('ABC'):
    path=app/'audio'/f'{name}.wav'
    with wave.open(str(path),'wb') as f:
        f.setparams((1,2,44100,0,'NONE','not compressed'));f.writeframes(bytes(22050))
    songs.append({'id':name,'title':name,'suffix':'wav','duration':0.25,'replayGain':{'trackGain':-6*index}})
with sqlite3.connect(app/'catalog.sqlite') as db:
    db.execute('CREATE TABLE kv(key TEXT PRIMARY KEY,value TEXT NOT NULL)')
    db.execute('INSERT INTO kv VALUES(?,?)',('queue',json.dumps({'queue':songs,'index':0})))
    db.execute('INSERT INTO kv VALUES(?,?)',('autoplay','false'))
    db.execute('CREATE TABLE downloads(id TEXT PRIMARY KEY,state TEXT NOT NULL,path TEXT NOT NULL,meta TEXT NOT NULL,bytes INTEGER NOT NULL DEFAULT 0,error TEXT)')
    for song in songs:
        path=app/'audio'/f'{song["id"]}.wav'
        db.execute('INSERT INTO downloads VALUES(?,?,?,?,?,NULL)',(song['id'],'ready',f'audio/{path.name}',json.dumps(song),path.stat().st_size))
PY
ZAY_UI_READY_FILE="$TASK_DATA/ready.json" ZAY_SILENT=1 XDG_DATA_HOME="$TASK_DATA" GDK_BACKEND=x11 GSK_RENDERER=cairo ZAY_SERVER=http://127.0.0.1:9 target/release/zaymusic-linux > build/native-handoff.log 2>&1 &
APP_PID=$!
gdbus wait --session --timeout=15 org.mpris.MediaPlayer2.zaymusic
gdbus call --session --dest org.mpris.MediaPlayer2.zaymusic --object-path /org/mpris/MediaPlayer2 --method org.mpris.MediaPlayer2.Player.Play >/dev/null
sleep 3
python3 - "$TASK_DATA/zaymusic/catalog.sqlite" <<'PY'
import json,sqlite3,sys
with sqlite3.connect(sys.argv[1]) as db:
    q=json.loads(db.execute("SELECT value FROM kv WHERE key='queue'").fetchone()[0])
    recents=[r[0] for r in db.execute('SELECT id FROM recent_songs ORDER BY played_at DESC')]
assert recents==['C','B','A'], f'Actual native playback did not record the three completed songs: {recents}; queue index {q["index"]}; history {[s["id"] for s in q.get("history",[])]}'
assert q['index']==2, f"Native handoff stopped at index {q['index']}"
assert [s['id'] for s in q['history']]==['A','B'], 'Native queue advanced before or after the wrong stream'
print('Three offline native WAV items auto-advanced with the server unreachable')
PY
python3 - "$TASK_DATA/ready.json" <<'PYGAIN'
import json,sys,time
for _ in range(30):
    try:
        value=json.load(open(sys.argv[1]))['gain']
        if abs(value-10**(-12/20))<.001:break
    except (OSError,ValueError,KeyError):pass
    time.sleep(.1)
else:raise AssertionError('ReplayGain did not follow the audible item')
PYGAIN
gdbus call --session --dest org.mpris.MediaPlayer2.zaymusic --object-path /org/mpris/MediaPlayer2 --method org.freedesktop.DBus.Properties.Set org.mpris.MediaPlayer2.Player Volume '<0.5>' >/dev/null
sleep 1
gdbus call --session --dest org.mpris.MediaPlayer2.zaymusic --object-path /org/mpris/MediaPlayer2 --method org.freedesktop.DBus.Properties.Get org.mpris.MediaPlayer2.Player Volume | python3 -c 'import re,sys;n=re.search(r"<([0-9.]+)>",sys.stdin.read());assert n and abs(float(n[1])-.5)<.001,"MPRIS volume did not update shared settings"'
python3 - "$TASK_DATA/ready.json" <<'PYVOLUME'
import json,sys
assert abs(json.load(open(sys.argv[1]))['gain']-.5*10**(-12/20))<.001,'User volume must multiply ReplayGain'
PYVOLUME

if command -v xdotool >/dev/null && command -v import >/dev/null; then
  mkdir -p build/native-screenshots
  WINDOW=$(xdotool search --name '^ZayMusic$' | head -1)
  xdotool windowsize --sync "$WINDOW" 1100 760
  xdotool mousemove --window "$WINDOW" 90 340 click 1
  sleep 1
  import -window "$WINDOW" build/native-screenshots/linux-offline.png
  xdotool mousemove --window "$WINDOW" 90 606 click 1
  sleep 1
  import -window "$WINDOW" build/native-screenshots/linux-recents.png
fi

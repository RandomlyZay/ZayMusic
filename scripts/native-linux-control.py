#!/usr/bin/env python3
"""Click a visible native control by its test-only geometry; no application actions bypassed."""
import json
import pathlib
import subprocess
import sys
import time

ready, window, label = sys.argv[1:4]
scroll = '--scroll' in sys.argv
menu = '--menu' in sys.argv
end = time.monotonic() + 12
last = []
while time.monotonic() < end:
    try:
        controls = json.loads(pathlib.Path(ready).read_text()).get('controls', [])
        last = [c['label'] for c in controls]
        matches = [c for c in controls if c['label'] == label and 0 < c['x'] < 1280]
        visible = [c for c in matches if 55 < c['y'] < (895 if menu else 660)]
        if visible:
            c = visible[-1] if '--last' in sys.argv else visible[0]
            subprocess.run(['xdotool', 'mousemove', '--window', window, str(round(c['x'])), str(round(c['y'])), 'click', '1'], check=True)
            print(f"Clicked native control {label} at {round(c['x'])},{round(c['y'])}")
            time.sleep(.4)
            break
        if scroll:
            direction = '4' if matches and matches[0]['y'] < 55 else '5'
            subprocess.run(['xdotool', 'mousemove', '--window', window, '700', '430', 'click', '--repeat', '3', '--delay', '30', direction], check=True)
            time.sleep(1.2)
        time.sleep(.3)
    except (OSError, ValueError, KeyError):
        time.sleep(.1)
else:
    raise SystemExit(f'Native control unavailable: {label}; available controls: {last}')

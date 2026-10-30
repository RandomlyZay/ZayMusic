#!/usr/bin/env python3
"""Loopback-only native UI fixtures. Never load credentials or real account data."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse, parse_qs
from threading import Lock
import json
import argparse
import io
import re
import struct
import unicodedata
import wave
import zlib
from pathlib import Path

ALBUMS = [{"id": f"fixture-album-{n}", "name": f"Fixture album {n}", "artist": "ZayMusic", "artistId": "fixture-artist", "coverArt": f"al-fixture-{n}", "songCount": 2, "year": 2026} for n in range(1, 5)]
SONGS = [{"id": f"fixture-{i}", "title": f"Fixture song {i}", "artist": "ZayMusic", "album": f"Fixture album {(i + 1) // 2}", "albumId": f"fixture-album-{(i + 1) // 2}", "coverArt": f"al-fixture-{(i + 1) // 2}", "duration": 120 + i * 17} for i in range(1, 9)]
PLAYLISTS = {"fixture-playlist": {"id": "fixture-playlist", "name": "Fixture playlist", "coverArt": "pl-fixture-playlist", "entry": SONGS[:3].copy()}}
ARTISTS = [{"id": "fixture-artist", "name": "ZayMusic", "coverArt": "ar-fixture-artist", "albumCount": len(ALBUMS)}]
# Fixed palette so generated covers are recognisably artwork, not app placeholders.
PALETTE = [((236, 72, 99), (64, 24, 120)), ((255, 170, 60), (180, 40, 60)), ((40, 200, 180), (20, 60, 140)), ((120, 110, 255), (20, 20, 60)), ((240, 240, 230), (40, 120, 90))]


def cover_png(key, size):
    """A deterministic striped cover with a disc, encoded without third-party imaging libraries."""
    size = max(16, min(size, 600))
    top, bottom = PALETTE[zlib.crc32(key.encode()) % len(PALETTE)]
    rows = []
    centre = size / 2
    for y in range(size):
        row = bytearray([0])
        for x in range(size):
            t = (x + y) / (2 * size)
            colour = [int(a + (b - a) * t) for a, b in zip(top, bottom)]
            if ((x + 2 * y) // max(4, size // 10)) % 2:
                colour = [min(255, c + 18) for c in colour]
            distance = ((x - centre) ** 2 + (y - centre) ** 2) ** 0.5
            if distance < size * 0.28:
                colour = [20, 20, 20] if distance > size * 0.06 else top
            row += bytes(colour)
        rows.append(bytes(row))
    def chunk(kind, data):
        return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))
    return b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', size, size, 8, 2, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(b''.join(rows))) + chunk(b'IEND', b'')
LOCK = Lock()
NEXT = 1
# --management-fixtures: richer settings/jobs/imports/discovery state for UI tests. Default
# behaviour (web, playlists, Swift checks) never touches anything below.
MANAGEMENT = False
VIDEO_ID = re.compile(r'[A-Za-z0-9_-]{6,64}')
PLAYLIST_ID = re.compile(r'[A-Za-z0-9_-]{1,128}')
EXTRA_ARTIST = {"id": "fixture-artist-accent", "name": "Fixturé Ensemble", "coverArt": "ar-fixture-artist-accent", "albumCount": 0}
STATE = {}


def reset_management():
    """Fresh, fake-only state. Cookie text and OAuth device codes are never part of it."""
    STATE.clear()
    STATE.update({
        'events': [],
        'cookies': {'configured': True, 'verified': True, 'premium': True, 'message': 'Fixture account verified',
                    'checkedAt': 1767225600, 'updatedAt': 1767225600, 'browser': 'Fixture browser', 'selfRelogin': True,
                    'oauth': {'configured': True, 'connected': False}},
        'oauthPending': False,
        'jobs': [
            {'id': 'fixture-job-failed', 'videoId': 'fixtureFail1', 'status': 'failed', 'stage': 'download', 'progress': 0.35,
             'error': 'Fixture download failed', 'overrides': {'title': 'Fixture failed job', 'artist': 'ZayMusic'}, 'playlistIds': []},
            {'id': 'fixture-job-running', 'videoId': 'fixtureRun1', 'status': 'running', 'stage': 'downloading', 'progress': 0.5,
             'error': None, 'overrides': {'title': 'Fixture running job', 'artist': 'ZayMusic'}, 'playlistIds': []},
        ],
        'imports': [{'id': 'fixture-import', 'ytmPlaylistId': 'PLfixtureImport', 'name': 'Fixture import', 'playlistId': 'fixture-playlist',
                     'createdAt': '2026-01-01T00:00:00Z', 'updatedAt': '2026-01-01T00:00:00Z', 'items': [
                         {'videoId': 'fixtureImp1', 'title': 'Fixture import track', 'artist': 'ZayMusic', 'position': 0, 'status': 'done', 'songId': 'fixture-1', 'error': None},
                         {'videoId': 'fixtureImp2', 'title': 'Fixture import missing', 'artist': 'ZayMusic', 'position': 1, 'status': 'failed', 'error': 'Fixture track unavailable'}]}],
        # Insertion order is star order; timestamps make liked ordering observable.
        'starred': {'song': {f'fixture-{i}': f'2026-01-0{i}T00:00:00Z' for i in range(1, 5)},
                    'album': {'fixture-album-1': '2026-01-01T00:00:00Z', 'fixture-album-2': '2026-01-02T00:00:00Z'},
                    'artist': {'fixture-artist': '2026-01-01T00:00:00Z'}},
        'albumSaves': [],
        # Fake query strings only, so drivers can tell which search a reload repeated.
        'searches': [],
        'cookieLines': 0,
    })
    for item in STATE['imports']:
        recount(item)


def recount(item):
    statuses = [track['status'] for track in item['items']]
    item.update(total=len(statuses), done=statuses.count('done'), failed=statuses.count('failed'),
                pending=len(statuses) - statuses.count('done') - statuses.count('failed'))
    return item


def normalize(text):
    return ''.join(c for c in unicodedata.normalize('NFD', text or '') if not unicodedata.combining(c)).lower().strip()


SILENCE = []


def silent_wav():
    """Ten minutes of 8-bit silence, long enough that UI drivers never race auto-advance."""
    if not SILENCE:
        buffer = io.BytesIO()
        with wave.open(buffer, 'wb') as f:
            f.setparams((1, 1, 4000, 0, 'NONE', 'not compressed'))
            f.writeframes(b'\x80' * 4000 * 600)
        SILENCE.append(buffer.getvalue())
    return SILENCE[0]


def starred(kind, item):
    stamp = STATE['starred'][kind].get(item['id'])
    return dict(item, starred=stamp) if stamp else dict(item)


def artists():
    return ARTISTS + [EXTRA_ARTIST]


def candidate(video, title, track=None, album='Fixture discovery album', art=''):
    return {'videoId': video, 'source': 'ytmusic', 'title': title, 'channel': 'ZayMusic', 'album': album, 'track': track,
            'durationSec': 180.0, 'thumbnails': [{'url': art, 'width': 300, 'height': 300}] if art else [], 'badges': [], 'score': 1.0,
            'url': f'https://www.youtube.com/watch?v={video}'}


def discovery_album(art):
    return {'browseId': 'MPREfixture', 'title': 'Fixture discovery album', 'artist': 'ZayMusic', 'year': 2026,
            'thumbnails': [{'url': art, 'width': 300, 'height': 300}], 'score': 1.0, 'pinned': False}


def album_tracks(art):
    return [candidate(f'fixtureAlb{n}', title, n, art=art) for n, title in
            enumerate(['Fixture discovery song', 'Fixture discovery interlude', 'Fixture discovery finale'], 1)]


def playlist_tracks(art):
    return [candidate(f'fixturePl{n}', f'Fixture YouTube track {n}', None, 'Fixture YouTube playlist', art) for n in range(1, 5)]


def event(name):
    STATE['events'].append(name)

class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_GET(self):
        self.handle_request()

    def do_POST(self):
        self.handle_request()

    def do_DELETE(self):
        self.handle_request()

    def handle_request(self):
        global NEXT
        url = urlparse(self.path)
        # The same fixture can sit behind the web gateway or serve native /api/fetch clients.
        if not url.path.startswith(('/rest/', '/api/fetch/')) and url.path != '/healthz':
            url = url._replace(path='/api/fetch' + url.path)
        params = parse_qs(url.query)
        body = self.rfile.read(int(self.headers.get('content-length', '0')))
        if self.headers.get('content-type', '').startswith('application/x-www-form-urlencoded'):
            params.update(parse_qs(body.decode(), keep_blank_values=True))
        field = lambda key, default='': params.get(key, [default])[0]
        result = {}
        status = 200
        with LOCK:
            endpoint = url.path.removeprefix('/rest/').removesuffix('.view')
            playlist_id = field('playlistId') or field('id')
            handled = self.management(url, endpoint, params, field, body) if MANAGEMENT else None
            if isinstance(handled, bytes):
                self.send_response(200)
                self.send_header('content-type', 'audio/wav')
                self.send_header('content-length', str(len(handled)))
                self.end_headers()
                self.wfile.write(handled)
                return
            if handled is not None:
                status, result = handled
            elif url.path == '/healthz':
                result = {'ok': True}
            elif endpoint == 'getCoverArt':
                image = cover_png(field('id'), int(field('size', '300')))
                self.send_response(200)
                self.send_header('content-type', 'image/png')
                self.send_header('content-length', str(len(image)))
                self.end_headers()
                self.wfile.write(image)
                return
            elif url.path == '/api/fetch/jobs':
                result = {'jobs': []}
            elif url.path == '/api/fetch/home':
                result = {'topSongs': SONGS[:5], 'rediscover': SONGS[5:]}
            elif endpoint == 'scrobble':
                result = {}
            elif endpoint == 'getStarred2':
                result = {'starred2': {'song': SONGS, 'album': ALBUMS, 'artist': ARTISTS}}
            elif endpoint == 'getAlbumList2':
                result = {'albumList2': {'album': ALBUMS}}
            elif endpoint == 'getAlbum':
                album = next((a for a in ALBUMS if a['id'] == field('id')), None)
                result = {'album': album and dict(album, song=[s for s in SONGS if s['albumId'] == album['id']])}
            elif endpoint == 'getArtist':
                result = {'artist': dict(ARTISTS[0], album=ALBUMS)}
            elif endpoint == 'search3':
                query = field('query').lower()
                result = {'searchResult3': {'song': [s for s in SONGS if query in s['title'].lower()], 'album': [], 'artist': []}}
            elif url.path == '/api/fetch/youtube/cookies':
                result = {'configured': False}
            elif endpoint == 'getArtists':
                result = {'artists': {'index': [{'name': 'Z', 'artist': ARTISTS}]}}
            elif endpoint == 'getPlaylists':
                result = {'playlists': {'playlist': [dict(p, songCount=len(p['entry'])) for p in PLAYLISTS.values()]}}
            elif endpoint == 'getPlaylist':
                result = {'playlist': PLAYLISTS.get(playlist_id)}
            elif endpoint == 'createPlaylist':
                if not playlist_id:
                    playlist_id = f'created-{NEXT}'
                    NEXT += 1
                    PLAYLISTS[playlist_id] = {'id': playlist_id, 'name': field('name'), 'entry': []}
                playlist = PLAYLISTS[playlist_id]
                playlist['entry'] = [dict(next((s for s in SONGS if s['id'] == key), {'id': key, 'title': key})) for key in params.get('songId', [])]
                result = {'playlist': dict(playlist, songCount=len(playlist['entry']))}
            elif endpoint == 'updatePlaylist':
                playlist = PLAYLISTS[playlist_id]
                if 'name' in params:
                    playlist['name'] = field('name')
                for index in sorted((int(n) for n in params.get('songIndexToRemove', [])), reverse=True):
                    playlist['entry'].pop(index)
                playlist['entry'].extend(dict(next(s for s in SONGS if s['id'] == key)) for key in params.get('songIdToAdd', []))
            elif endpoint == 'deletePlaylist':
                PLAYLISTS.pop(playlist_id, None)
            elif url.path == '/api/fetch/youtube/imports':
                result = {'imports': []}
            else:
                status = 404
                result = {'message': 'Fixture endpoint not implemented'}
        if url.path.startswith('/rest/'):
            result = {'subsonic-response': {'status': 'ok', **result}}
        encoded = json.dumps(result).encode()
        self.send_response(status)
        self.send_header('content-type', 'application/json')
        self.send_header('content-length', str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)

    def management(self, url, endpoint, params, field, body):
        """Returns (status, result), raw audio bytes, or None to fall through to default fixtures."""
        path = url.path.removeprefix('/api/fetch/') if url.path.startswith('/api/fetch/') else None
        method = self.command
        art = f"http://{self.headers.get('host', '127.0.0.1')}/rest/getCoverArt?size=300&id="
        json_body = lambda: json.loads(body or b'{}') if body else {}
        if url.path.startswith('/rest/'):
            if endpoint in ('stream', 'download'):
                return silent_wav()
            if endpoint in ('star', 'unstar'):
                for kind, key in (('song', 'id'), ('album', 'albumId'), ('artist', 'artistId')):
                    for item in params.get(key, []):
                        if endpoint == 'star':
                            STATE['starred'][kind][item] = f'2026-02-{len(STATE["events"]) % 27 + 1:02d}T00:00:00Z'
                        else:
                            STATE['starred'][kind].pop(item, None)
                        event(f'{endpoint}:{kind}:{item}')
                return 200, {}
            if endpoint == 'getStarred2':
                pick = lambda kind, rows: [starred(kind, r) for key in STATE['starred'][kind] for r in rows if r['id'] == key]
                return 200, {'starred2': {'song': pick('song', SONGS), 'album': pick('album', ALBUMS), 'artist': pick('artist', artists())}}
            if endpoint == 'search3':
                STATE['searches'].append(['library', field('query')])
                query = normalize(field('query'))
                hit = lambda *values: not query or any(query in normalize(v) for v in values)
                return 200, {'searchResult3': {
                    'song': [starred('song', s) for s in SONGS if hit(s['title'], s['artist'], s['album'])],
                    'album': [starred('album', a) for a in ALBUMS if hit(a['name'], a['artist'])],
                    'artist': [starred('artist', a) for a in artists() if hit(a['name'])]}}
            if endpoint == 'getAlbum':
                album = next((a for a in ALBUMS if a['id'] == field('id')), None)
                return (200, {'album': starred('album', album) | {'song': [starred('song', s) for s in SONGS if s['albumId'] == album['id']]}}) if album else (404, {'message': 'Unknown fixture album'})
            if endpoint == 'getArtist':
                artist = next((a for a in artists() if a['id'] == field('id')), None)
                return (200, {'artist': starred('artist', artist) | {'album': [starred('album', a) for a in ALBUMS if a['artistId'] == artist['id']]}}) if artist else (404, {'message': 'Unknown fixture artist'})
            if endpoint == 'getArtists':
                return 200, {'artists': {'index': [{'name': 'F', 'artist': [EXTRA_ARTIST]}, {'name': 'Z', 'artist': ARTISTS}]}}
            if endpoint == 'getGenres':
                return 200, {'genres': {'genre': [{'value': 'Fixture', 'songCount': len(SONGS), 'albumCount': len(ALBUMS)}]}}
            if endpoint == 'getSongsByGenre':
                return 200, {'songList': {'song': SONGS if field('genre') == 'Fixture' else []}}
            return None
        if path is None:
            return None
        if path == '__fixture/state':
            return 200, {'events': STATE['events'], 'jobs': STATE['jobs'], 'imports': STATE['imports'], 'starred': {k: list(v) for k, v in STATE['starred'].items()},
                         'albumSaves': STATE['albumSaves'], 'cookies': STATE['cookies'], 'oauthPending': STATE['oauthPending'],
                         'searches': STATE['searches'], 'cookieLines': STATE['cookieLines']}
        if path == 'youtube/cookies':
            cookies = STATE['cookies']
            if method == 'POST':
                # The pasted text is only validated, then dropped: never stored, echoed or logged.
                text = json_body().get('cookies')
                valid = isinstance(text, str) and len(text.strip()) >= 16
                lines = len(text.strip().splitlines()) if valid else 0
                del text
                if not valid:
                    return 400, {'detail': 'Paste the contents of a cookies.txt file'}
                cookies.update(configured=True, verified=True, message='Fixture account verified')
                STATE['cookieLines'] = lines
                event('cookies:saved')
            elif method == 'DELETE':
                cookies.update(configured=False, verified=False, message=None)
                event('cookies:deleted')
                return 200, {'ok': True, 'removed': True}
            elif field('verify') in ('true', '1'):
                event('cookies:verified')
            return 200, dict(cookies, oauth=dict(cookies['oauth']))
        if path == 'youtube/oauth/start' and method == 'POST':
            STATE['oauthPending'] = True
            event('oauth:start')
            return 200, {'user_code': 'FIX-CODE', 'verification_url': 'https://example.invalid/device', 'expires_in': 600}
        if path == 'youtube/oauth/poll' and method == 'POST':
            # Stays pending so screenshots can show the device code.
            return (200, {'connected': False, 'pending': True}) if STATE['oauthPending'] else (400, {'detail': 'No Google sign-in in progress. Start again.'})
        if path == 'youtube/oauth' and method == 'DELETE':
            STATE['oauthPending'] = False
            STATE['cookies']['oauth']['connected'] = False
            event('oauth:disconnect')
            return 200, {'ok': True, 'removed': True}
        if path == 'jobs':
            if method == 'GET':
                return 200, {'jobs': STATE['jobs']}
            request = json_body()
            video = request.get('videoId') or ''
            if not VIDEO_ID.fullmatch(video):
                return 400, {'detail': 'invalid YouTube video id'}
            targets = [t for t in request.get('playlistIds') or [] if isinstance(t, str) and PLAYLIST_ID.fullmatch(t)]
            job = {'id': f'fixture-job-{len(STATE["jobs"]) + len(STATE["events"])}', 'videoId': video, 'status': 'queued', 'stage': 'queued',
                   'progress': 0, 'error': None, 'overrides': request.get('overrides'), 'playlistIds': targets}
            STATE['jobs'].insert(0, job)
            event(f'job:{video}:{",".join(targets)}')
            return 200, {'job': job}
        match = re.fullmatch(r'jobs/([^/]+)(?:/(retry|cancel))?', path)
        if match:
            job = next((j for j in STATE['jobs'] if j['id'] == match[1]), None)
            if job is None:
                return 404, {'detail': 'job not found'}
            action = match[2] or ('delete' if method == 'DELETE' else 'get')
            if action == 'retry' and job['status'] in ('failed', 'cancelled'):
                job.update(status='queued', stage='queued', progress=0, error=None)
            elif action == 'cancel':
                job.update(status='cancelled', stage='cancelled')
            elif action == 'delete':
                STATE['jobs'].remove(job)
                event(f'delete:{job["id"]}')
                return 200, {'ok': True}
            if action != 'get':
                event(f'{action}:{job["id"]}')
            return 200, {'job': job}
        if path == 'youtube/imports':
            if method == 'GET':
                return 200, {'imports': STATE['imports']}
            playlist = json_body().get('playlistId')
            if not isinstance(playlist, str) or not PLAYLIST_ID.fullmatch(playlist):
                return 400, {'detail': 'playlistId required'}
            event(f'import:{playlist}')
            existing = next((i for i in STATE['imports'] if i['ytmPlaylistId'] == playlist), None)
            if existing is None:
                existing = recount({'id': f'fixture-import-{playlist}', 'ytmPlaylistId': playlist, 'name': 'Fixture YouTube playlist', 'playlistId': None,
                                    'createdAt': '2026-01-02T00:00:00Z', 'updatedAt': '2026-01-02T00:00:00Z',
                                    'items': [{'videoId': t['videoId'], 'title': t['title'], 'artist': 'ZayMusic', 'position': n, 'status': 'pending', 'error': None}
                                              for n, t in enumerate(playlist_tracks(''))]})
                STATE['imports'].append(existing)
            return 200, {'import': existing}
        match = re.fullmatch(r'youtube/imports/([^/]+)(?:/(pull|sync))?', path)
        if match:
            item = next((i for i in STATE['imports'] if i['id'] == match[1]), None)
            if item is None:
                return 404, {'detail': 'import not found'}
            if method == 'DELETE' and not match[2]:
                STATE['imports'].remove(item)
                event(f'import-delete:{item["id"]}')
                return 200, {'ok': True}
            if match[2] == 'pull':
                # Additive: only adds songs the local copy lacks, never removes any.
                if not any(t['videoId'] == 'fixturePull1' for t in item['items']):
                    item['items'].append({'videoId': 'fixturePull1', 'title': 'Fixture pulled track', 'artist': 'ZayMusic',
                                          'position': len(item['items']), 'status': 'pending', 'error': None})
            elif match[2] == 'sync':
                for track in item['items']:
                    if track['status'] == 'pending':
                        track['status'] = 'done'
            if match[2]:
                event(f'{match[2]}:{item["id"]}')
            return 200, {'import': recount(item)}
        if path == 'youtube/library/playlists':
            return 200, {'playlists': [{'playlistId': 'PLfixture', 'title': 'Fixture YouTube playlist', 'count': 4, 'owned': True,
                                        'thumbnails': [{'url': art + 'yt-PLfixture', 'width': 300, 'height': 300}]}]}
        if path == 'youtube/library/playlists/PLfixture':
            return 200, {'id': 'PLfixture', 'title': 'Fixture YouTube playlist', 'tracks': playlist_tracks(art + 'yt-PLfixture')}
        if path == 'search':
            STATE['searches'].append(['discovery', field('q')])
            query = normalize(field('q'))
            known = any(query in normalize(s['title']) for s in SONGS)
            return 200, {'candidates': [candidate('fixtureNew1', 'Fixture discovery song', art=art + 'yt-new-1'),
                                        candidate('fixtureNew2', 'Fixture discovery live', art=art + 'yt-new-2')],
                         'albums': [discovery_album(art + 'yt-album')],
                         'correction': None if known else {'artist': 'ZayMusic', 'title': 'Fixture song 1'},
                         'suggestions': ['fixture song', 'fixture album']}
        if path == 'suggest':
            return 200, {'suggestions': ['fixture song', 'fixture album'], 'correction': None}
        if path == 'album':
            if field('browseId') != 'MPREfixture':
                return 502, {'detail': 'Could not load this album'}
            return 200, {'album': discovery_album(art + 'yt-album'), 'candidates': album_tracks(art + 'yt-album')}
        if path == 'album-saves' and method == 'POST':
            request = json_body()
            STATE['albumSaves'].append({'title': request.get('title'), 'artist': request.get('artist')})
            event(f'album-save:{request.get("title")}')
            return 200, {'albumId': None}
        if path == 'resolve':
            target = field('url')
            if 'list=' in target:
                return 200, {'kind': 'playlist', 'title': 'Fixture YouTube playlist', 'candidates': playlist_tracks(art + 'yt-PLfixture')}
            if '/browse/' in target:
                return 200, {'kind': 'album', 'title': 'Fixture discovery album', 'candidates': album_tracks(art + 'yt-album')}
            video = parse_qs(urlparse(target).query).get('v', ['fixtureUrl1'])[0]
            return 200, {'kind': 'video', 'title': None, 'candidates': [candidate(video if VIDEO_ID.fullmatch(video) else 'fixtureUrl1', 'Fixture linked video', art=art + 'yt-url')]}
        if path.startswith('library/albums/') and path.endswith('/order'):
            album = path.removeprefix('library/albums/').removesuffix('/order')
            return 200, {'tracks': {s['id']: n for n, s in enumerate((s for s in SONGS if s['albumId'] == album), 1)}}
        if path == 'preview/warm':
            return 200, {}
        if path.startswith('preview/'):
            return silent_wav()
        if path == 'radio':
            return 200, {'candidates': [], 'sources': []}
        if path.startswith('music/') and method == 'DELETE':
            event(f'delete-song:{path.removeprefix("music/")}')
            return 200, {'ok': True}
        return None


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--port', type=int, default=18765)
    parser.add_argument('--ready-file')
    parser.add_argument('--management-fixtures', action='store_true', help='serve fake settings, jobs, imports and discovery data')
    args = parser.parse_args()
    if args.management_fixtures:
        MANAGEMENT = True
        reset_management()
    server = ThreadingHTTPServer(('127.0.0.1', args.port), Handler)
    if args.ready_file:
        Path(args.ready_file).write_text(f'http://127.0.0.1:{server.server_address[1]}')
    server.serve_forever()

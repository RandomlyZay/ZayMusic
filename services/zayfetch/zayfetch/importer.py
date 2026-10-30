"""One-way YouTube Music → ZayMusic playlist import.

The connected YouTube Music account is already the acquisition path, so its
library playlists come straight from the same session (``ytmusicapi`` browser
auth built from the stored cookies). Importing mirrors one YTM playlist into a
Navidrome playlist:

- tracks already in the library are linked by metadata match,
- the rest are queued as acquisition jobs (Premium-first, like any download),
- a reconciler adds tracks to the playlist as downloads land, in YTM order.

One-way and additive by design: ZayMusic never writes back to YouTube Music,
and a sync or pull only ever *adds* songs to the local playlist. Songs added
locally stay, the playlist keeps its id, and a YTM song missing locally (even
one removed on purpose) is added back on the next pull.
"""

from __future__ import annotations

import hashlib
import logging
import os
import re
import secrets
from datetime import datetime, timedelta, timezone

import httpx

from . import discovery, downloader, store
from .models import (
    FetchJob,
    ImportItem,
    ImportTrack,
    Overrides,
    PlaylistImport,
    Thumbnail,
    YouTubePlaylist,
)

log = logging.getLogger("zayfetch.importer")

_COOKIES_REQUIRED_MESSAGE = downloader.COOKIES_REQUIRED_MESSAGE
_BROWSER_UA = "ZayMusic/0.1 (+https://github.com/zaymusic)"


class ImportError_(RuntimeError):
    """Import could not be started/completed; message is user-facing."""


# ---------------------------------------------------------------------------
# Navidrome (Subsonic) client
# ---------------------------------------------------------------------------


def _navidrome_config() -> tuple[str, str, str]:
    url = os.environ.get("NAVIDROME_URL", "http://navidrome:4533").rstrip("/")
    user = os.environ.get("NAVIDROME_USER", "admin")
    password = os.environ.get("NAVIDROME_PASS", "")
    return url, user, password


def _subsonic_params(**extra: str) -> dict[str, str]:
    url, user, password = _navidrome_config()
    del url
    salt = secrets.token_hex(16)
    params = {
        "u": user,
        "t": hashlib.md5((password + salt).encode()).hexdigest(),
        "s": salt,
        "v": "1.16.1",
        "c": "ZayFetch",
        "f": "json",
    }
    params.update(extra)
    return params


def _unwrap(response: httpx.Response) -> dict:
    response.raise_for_status()
    body = response.json().get("subsonic-response", {})
    if body.get("status") != "ok":
        error = body.get("error", {})
        raise ImportError_(error.get("message") or "Navidrome request failed")
    return body


def _navidrome_get(endpoint: str, **params: str) -> dict:
    url, _, _ = _navidrome_config()
    with httpx.Client(timeout=20, headers={"User-Agent": _BROWSER_UA}) as client:
        return _unwrap(client.get(f"{url}/rest/{endpoint}", params=_subsonic_params(**params)))


def _navidrome_post(endpoint: str, data: dict | None = None, **params: str) -> dict:
    url, _, _ = _navidrome_config()
    with httpx.Client(timeout=30, headers={"User-Agent": _BROWSER_UA}) as client:
        return _unwrap(client.post(f"{url}/rest/{endpoint}", params=_subsonic_params(**params), data=data or {}))


def navidrome_search_songs(query: str, count: int = 20) -> list[dict]:
    body = _navidrome_get("search3", query=query, songCount=str(count),
                          artistCount="0", albumCount="0")
    songs = (body.get("searchResult3") or {}).get("song") or []
    if isinstance(songs, dict):
        songs = [songs]
    return [s for s in songs if isinstance(s, dict)]


def navidrome_create_playlist(name: str, song_ids: list[str]) -> str | None:
    body = _navidrome_post("createPlaylist", data={"name": name, "songId": song_ids})
    return (body.get("playlist") or {}).get("id")


def navidrome_replace_playlist_songs(playlist_id: str, song_ids: list[str]) -> None:
    """Rewrite a playlist's songs in place; the playlist id is preserved."""
    _navidrome_post("createPlaylist", data={"playlistId": playlist_id, "songId": song_ids})


def navidrome_delete_playlist(playlist_id: str) -> None:
    try:
        _navidrome_post("deletePlaylist", data={"id": playlist_id})
    except (ImportError_, httpx.HTTPError) as exc:
        log.info("could not delete playlist %s: %s", playlist_id, exc)


def navidrome_playlist_song_ids(playlist_id: str) -> list[str] | None:
    """Ordered song ids, or None when the playlist no longer exists."""
    try:
        body = _navidrome_get("getPlaylist", id=playlist_id)
    except (ImportError_, httpx.HTTPError):
        return None
    entries = (body.get("playlist") or {}).get("entry") or []
    if isinstance(entries, dict):
        entries = [entries]
    return [e.get("id") for e in entries if isinstance(e, dict) and e.get("id")]


# ---------------------------------------------------------------------------
# YouTube Music library
# ---------------------------------------------------------------------------


def _thumbnail_list(raw: list | None) -> list[Thumbnail]:
    out: list[Thumbnail] = []
    for t in raw or []:
        if isinstance(t, dict) and t.get("url"):
            out.append(Thumbnail(url=t.get("url", ""), width=t.get("width", 0), height=t.get("height", 0)))
    return out


def _artist_name(track: dict) -> str:
    artists = track.get("artists") or []
    if artists and isinstance(artists[0], dict):
        return str(artists[0].get("name") or "")
    return str(track.get("artist") or "")


def _album_name(track: dict) -> str:
    album = track.get("album")
    if isinstance(album, dict):
        return str(album.get("name") or "")
    return str(album or "")


def list_library_playlists() -> list[YouTubePlaylist]:
    """The connected account's YT Music library playlists (newest first)."""
    if not downloader.cookies_file():
        raise ImportError_(_COOKIES_REQUIRED_MESSAGE)
    try:
        raw = discovery.ytmusic_call("get_library_playlists", limit=None) or []
    except Exception as exc:  # noqa: BLE001 - surface an actionable message
        log.warning("could not list YT Music playlists: %s", exc)
        raise ImportError_("Could not read your YouTube Music playlists. Reconnect the account in Settings.") from exc
    out: list[YouTubePlaylist] = []
    for p in raw:
        if not isinstance(p, dict):
            continue
        playlist_id = str(p.get("playlistId") or "")
        if not playlist_id:
            continue
        out.append(YouTubePlaylist(
            playlistId=playlist_id,
            title=str(p.get("title") or "Untitled playlist"),
            count=_to_int(p.get("count")),
            owned=p.get("owned") if isinstance(p.get("owned"), bool) else None,
            thumbnails=_thumbnail_list(p.get("thumbnails")),
        ))
    return out


def _to_int(value: object) -> int:
    """YTM reports counts as formatted strings ("1,160")."""
    if isinstance(value, (int, float)):
        return int(value)
    try:
        return int(str(value or "0").replace(",", "").strip())
    except ValueError:
        return 0


def get_playlist_tracks(playlist_id: str) -> tuple[str, list[ImportTrack]]:
    """Title + ordered tracks for one YT Music playlist."""
    if not downloader.cookies_file():
        raise ImportError_(_COOKIES_REQUIRED_MESSAGE)
    try:
        # ytmusicapi stops at 100 tracks by default; limit=None reads them all.
        info = discovery.ytmusic_call("get_playlist", playlist_id, limit=None) or {}
    except Exception as exc:  # noqa: BLE001
        log.warning("could not read YT Music playlist %s: %s", playlist_id, exc)
        raise ImportError_("Could not read that YouTube Music playlist.") from exc
    title = str(info.get("title") or "Imported from YouTube Music")
    tracks: list[ImportTrack] = []
    position = 0
    for t in info.get("tracks") or []:
        if not isinstance(t, dict):
            continue
        video_id = str(t.get("videoId") or "")
        if not video_id:
            continue
        duration = t.get("duration_seconds")
        if duration in (None, ""):
            duration = _parse_duration(t.get("duration"))
        tracks.append(ImportTrack(
            videoId=video_id,
            title=str(t.get("title") or "Unknown title"),
            artist=_artist_name(t),
            album=_album_name(t),
            durationSec=_to_float(duration),
            position=position,
            thumbnails=_thumbnail_list(t.get("thumbnails")),
        ))
        position += 1
    return title, tracks


def _parse_duration(value: object) -> float:
    if isinstance(value, (int, float)):
        return float(value)
    if isinstance(value, str) and value:
        try:
            parts = [int(x) for x in value.split(":")]
        except ValueError:
            return 0.0
        return float(sum(p * (60 ** (len(parts) - i - 1)) for i, p in enumerate(parts)))
    return 0.0


def _to_float(value: object) -> float:
    try:
        return float(value or 0)
    except (TypeError, ValueError):
        return 0.0


def _parse_time(value: object) -> datetime | None:
    if not value:
        return None
    try:
        return datetime.fromisoformat(str(value).replace("Z", "+00:00"))
    except ValueError:
        return None


# ---------------------------------------------------------------------------
# Matching library songs to YT Music tracks
# ---------------------------------------------------------------------------

_NON_ALNUM = re.compile(r"[^a-z0-9]+")


def _normalize(text: str) -> str:
    return _NON_ALNUM.sub(" ", (text or "").casefold()).strip()


def _video_id_from_comment(comment: object) -> str:
    """yt-dlp embeds the source URL in the comment tag; recover the video id."""
    m = re.search(r"(?:v=|youtu\.be/|shorts/)([A-Za-z0-9_-]{6,64})", str(comment or ""))
    return m.group(1) if m else ""


def video_id_for_library_song(song_id: str) -> str:
    """Exact source video for any downloaded library song, from its comment tag."""
    if not song_id:
        return ""
    try:
        song = _navidrome_get("getSong", id=str(song_id)).get("song") or {}
    except (ImportError_, httpx.HTTPError) as exc:
        log.info("getSong failed for %r: %s", song_id, exc)
        return ""
    return _video_id_from_comment(song.get("comment"))


def find_library_song(track: ImportTrack) -> str | None:
    """Navidrome song id for a track that is already in the library, if any.

    Matches on normalized title + artist, with duration as a sanity
    check, and accepts a candidate outright when its embedded source comment
    carries the same YouTube video id."""
    quark = " ".join(part for part in (track.artist, track.title) if part).strip()
    if not quark:
        return None
    try:
        candidates = navidrome_search_songs(quark, count=25)
    except (ImportError_, httpx.HTTPError) as exc:
        log.info("library search failed for %r: %s", quark, exc)
        return None
    if not candidates:
        return None

    identity = store.song_identity(track.title, track.artist)
    for song in candidates:
        song_id = song.get("id")
        if not song_id:
            continue
        if track.videoId and _video_id_from_comment(song.get("comment")) == track.videoId:
            return str(song_id)
        # Token-set fuzzy matching treats "Song" and "Song (Live)" as equal.
        # Only collapse actual identity matches, not alternate recordings.
        if not identity or identity != store.song_identity(song.get("title"), song.get("artist")):
            continue
        # A grossly different runtime is a different recording (live, remix,
        # sped-up upload), even when the title/artist match.
        try:
            got_duration = float(song.get("duration") or 0)
        except (TypeError, ValueError):
            got_duration = 0.0
        if track.durationSec and got_duration and abs(track.durationSec - got_duration) > 25:
            continue
        return str(song_id)
    return None


# ---------------------------------------------------------------------------
# Import lifecycle
# ---------------------------------------------------------------------------


def _now() -> str:
    return datetime.now(timezone.utc).isoformat()


def build_import_model(import_row: dict) -> PlaylistImport:
    items = [ImportItem(**item) for item in store.list_import_items(import_row["id"])]
    return PlaylistImport(
        id=import_row["id"],
        ytmPlaylistId=import_row["ytmPlaylistId"],
        name=import_row["name"],
        playlistId=import_row["playlistId"],
        createdAt=import_row["createdAt"],
        updatedAt=import_row["updatedAt"],
        total=len(items),
        done=sum(1 for i in items if i.status == "done"),
        pending=sum(1 for i in items if i.status in ("pending", "downloading")),
        failed=sum(1 for i in items if i.status == "failed"),
        items=items,
    )


def list_imports() -> list[PlaylistImport]:
    return [build_import_model(row) for row in store.list_imports()]


def create_import(playlist_id: str) -> PlaylistImport:
    """Mirror one YT Music playlist into the local library.

    Re-importing the same YTM playlist reuses its existing local playlist
    (and syncs) rather than creating a duplicate."""
    if not downloader.cookies_file():
        raise ImportError_(_COOKIES_REQUIRED_MESSAGE)
    existing = store.get_import_by_ytm(playlist_id)
    title, tracks = get_playlist_tracks(playlist_id)
    if existing is not None:
        _seed_items(existing["id"], tracks)
        sync_import(existing["id"], recreate=True)
        return build_import_model(store.get_import(existing["id"]) or existing)

    nav_id: str | None = None
    try:
        nav_id = navidrome_create_playlist(title, [])
    except (ImportError_, httpx.HTTPError) as exc:
        log.info("could not create placeholder playlist %r yet: %s", title, exc)

    row = store.create_import(ytm_playlist_id=playlist_id, name=title, navidrome_playlist_id=nav_id)
    _seed_items(row["id"], tracks)
    sync_import(row["id"])
    return build_import_model(store.get_import(row["id"]) or row)


def _seed_items(import_id: str, tracks: list[ImportTrack]) -> None:
    """Insert tracks not yet recorded and refresh every item's YTM position.

    Failed items are retried: a pull is the explicit "try again"."""
    known = {item["videoId"]: item for item in store.list_import_items(import_id)}
    seen: set[str] = set()
    for track in tracks:
        if track.videoId in seen:
            continue
        seen.add(track.videoId)
        item = known.get(track.videoId)
        if item is not None:
            fields: dict = {}
            if item["position"] != track.position:
                fields["position"] = track.position
            if item["status"] == "failed":
                fields.update(status="pending", job_id=None, error=None)
            if fields:
                store.set_import_item_by_video(import_id, track.videoId, **fields)
            continue
        store.add_import_item(
            import_id,
            video_id=track.videoId,
            title=track.title,
            artist=track.artist,
            album=track.album,
            duration_sec=track.durationSec,
            position=track.position,
        )


def _queue_download(item: ImportItem) -> str | None:
    """Queue (or reuse) a Premium-first acquisition job for one track."""
    active = store.find_active_job(item.videoId)
    if active is not None:
        return active.id
    overrides = Overrides(
        title=item.title or None,
        artist=item.artist or None,
        album=item.album or None,
    )
    job = store.create_job(videoId=item.videoId, overrides=overrides)
    return job.id


def sync_import(import_id: str, *, recreate: bool = False) -> PlaylistImport:
    """Resolve pending items against the library and queue missing downloads.

    Items whose job is still queued/running are skipped: there is nothing to
    look up until the download lands, and probing Navidrome for every track on
    every tick would hammer it."""
    row = store.get_import(import_id)
    if row is None:
        raise ImportError_("That import no longer exists")
    for item in build_import_model(row).items:
        if item.status in ("done", "skipped"):
            continue
        job = store.get_job(item.jobId) if item.jobId else None
        if item.status == "failed":
            # Retrying from Downloads must also resume the playlist import.
            # A shared job may have a different id, so adopt that work too.
            if job is None or job.status in ("failed", "cancelled"):
                job = store.find_reusable_job(item.videoId, Overrides(title=item.title, artist=item.artist))
            if job is None:
                continue
            store.set_import_item_by_video(import_id, item.videoId, job_id=job.id)
        if job is not None and job.status in ("queued", "running"):
            if item.status != "downloading" or item.jobId != job.id:
                store.set_import_item_by_video(import_id, item.videoId, status="downloading", job_id=job.id)
            continue
        if job is not None and job.status in ("failed", "cancelled"):
            store.set_import_item_by_video(import_id, item.videoId, status="failed",
                                           error=job.error or "download failed")
            continue
        # No active job: either the download landed (job done) or we still need
        # one. Check the library first so a landed track links in.
        song_id = find_library_song(item)
        if song_id:
            store.set_import_item_by_video(import_id, item.videoId, status="done", song_id=song_id, error=None)
            continue
        if job is None:
            job_id = _queue_download(item)
            if job_id:
                store.set_import_item_by_video(import_id, item.videoId, status="downloading", job_id=job_id, error=None)
            else:
                store.set_import_item_by_video(import_id, item.videoId, status="failed", error="could not queue download")
        else:
            # Job finished but Navidrome has not indexed it yet; retry next tick.
            store.set_import_item_by_video(import_id, item.videoId, status="downloading", job_id=job.id)
    _sync_playlist(import_id, recreate=recreate)
    row = store.get_import(import_id) or row
    return build_import_model(row)


def merge_missing(current: list[str], ordered: list[str]) -> list[str]:
    """``current`` plus every song of ``ordered`` it lacks, never removing any.

    A missing song lands right after the nearest earlier YTM song already in
    the playlist (or before the nearest later one), so imports fill in YTM
    order while songs added locally keep their place."""
    result = list(current)
    present = set(result)
    for idx, song_id in enumerate(ordered):
        if song_id in present:
            continue
        insert_at = None
        for prev in reversed(ordered[:idx]):
            if prev in present:
                insert_at = result.index(prev) + 1
                break
        if insert_at is None:
            for nxt in ordered[idx + 1:]:
                if nxt in present:
                    insert_at = result.index(nxt)
                    break
        result.insert(len(result) if insert_at is None else insert_at, song_id)
        present.add(song_id)
    return result


def _sync_playlist(import_id: str, *, recreate: bool = False) -> None:
    """Add resolved tracks the local playlist lacks. Never removes anything.

    A playlist deleted from the library is only recreated by an explicit pull
    (``recreate``); the background reconciler leaves it gone."""
    row = store.get_import(import_id)
    if row is None:
        return
    items = store.list_import_items(import_id)
    ordered = [str(i["songId"]) for i in items if i["status"] == "done" and i["songId"]]
    playlist_id = row.get("playlistId")

    current = navidrome_playlist_song_ids(playlist_id) if playlist_id else None
    try:
        if current is None:
            if playlist_id and not recreate:
                return
            new_id = navidrome_create_playlist(row["name"], ordered)
            if new_id:
                store.update_import(import_id, navidrome_playlist_id=new_id)
            return
        merged = merge_missing(current, ordered)
        if merged != current:
            navidrome_replace_playlist_songs(playlist_id, merged)
    except (ImportError_, httpx.HTTPError) as exc:
        log.info("could not write playlist %r: %s", row["name"], exc)


def reconcile_imports() -> None:
    """Advance every import whose downloads have landed. Never raises."""
    try:
        rows = store.list_imports()
    except Exception as exc:  # noqa: BLE001
        log.info("could not list imports: %s", exc)
        return
    for row in rows:
        try:
            model = build_import_model(row)
            if model.total and model.done + model.failed >= model.total:
                continue
            sync_import(row["id"])
        except Exception as exc:  # noqa: BLE001 - a bad import must not kill the loop
            log.info("reconcile failed for import %s: %s", row.get("id"), exc)


def pull_import(import_id: str) -> PlaylistImport:
    """Re-read the YTM playlist and add whatever the local copy is missing."""
    row = store.get_import(import_id)
    if row is None:
        raise ImportError_("That import no longer exists")
    _, tracks = get_playlist_tracks(row["ytmPlaylistId"])
    _seed_items(import_id, tracks)
    return sync_import(import_id, recreate=True)


def remove_import(import_id: str) -> None:
    """Forget an import and its local playlist (library files are untouched)."""
    row = store.get_import(import_id)
    if row is None:
        raise ImportError_("That import no longer exists")
    if row.get("playlistId"):
        navidrome_delete_playlist(row["playlistId"])
    store.delete_import(import_id)


# ---------------------------------------------------------------------------
# Job playlist targets (the "+" button on a song picks where it lands)
# ---------------------------------------------------------------------------

# A download finished but the song never showed up in the library within this
# window: give up on the playlist add, exactly like stale album saves.
JOB_TARGET_EXPIRY = timedelta(days=3)


def _job_song_id(job: FetchJob) -> str | None:
    """Navidrome song id for a finished download: the recorded id, or a lookup."""
    if job.result and job.result.songId:
        return job.result.songId
    overrides = job.overrides
    if not overrides or not overrides.title:
        return None
    track = ImportTrack(videoId=job.videoId, title=overrides.title, artist=overrides.artist or "")
    return find_library_song(track)


def _drop_target(row: dict) -> None:
    store.delete_job_target(row["jobId"], row["playlistId"])


def reconcile_job_targets() -> None:
    """Add finished downloads to the playlists chosen when they were saved.

    Pending targets wait for scans to index the song (or for a retry to
    succeed); targets whose playlist vanished, whose song is already there,
    or that go unresolved too long are dropped. Never raises."""
    try:
        targets = store.list_job_targets()
    except Exception as exc:  # noqa: BLE001
        log.info("could not list job playlist targets: %s", exc)
        return
    now = datetime.now(timezone.utc)
    playlists: dict[str, list[str] | None] = {}
    songs: dict[str, str | None] = {}
    for row in targets:
        job_id = row["jobId"]
        try:
            job = store.get_job(job_id)
            if job is None:
                _drop_target(row)
                continue
            if job.status in ("queued", "running"):
                continue
            created = _parse_time(row["createdAt"])
            if created and now - created > JOB_TARGET_EXPIRY:
                log.info("giving up on playlist target %s → %s", job_id, row["playlistId"])
                _drop_target(row)
                continue
            if job.status != "done":
                continue  # failed/cancelled: a retry can still complete this
            song_id = songs.get(job_id) if job_id in songs else None
            if song_id is None and job_id not in songs:
                song_id = _job_song_id(job)
                songs[job_id] = song_id
            if not song_id:
                continue
            playlist_id = row["playlistId"]
            current = playlists.get(playlist_id)
            if current is None and playlist_id not in playlists:
                current = navidrome_playlist_song_ids(playlist_id)
                playlists[playlist_id] = current
            if current is None:
                log.info("playlist %s is gone; dropping target for job %s", playlist_id, job_id)
                _drop_target(row)
                continue
            if song_id in current:
                _drop_target(row)
                continue
            _navidrome_post("updatePlaylist", data={"playlistId": playlist_id, "songIdToAdd": [song_id]})
            current.append(song_id)
            _drop_target(row)
        except (ImportError_, httpx.HTTPError) as exc:
            log.info("playlist target %s → %s not applied yet: %s", job_id, row["playlistId"], exc)
        except Exception as exc:  # noqa: BLE001 - one bad target must not stop the rest
            log.info("reconcile failed for job target %s → %s: %s", job_id, row["playlistId"], exc)

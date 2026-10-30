"""Listening-based Home sections and saved (starred) albums.

Navidrome creates an album for every downloaded file, so "an album exists"
is not the same as "you saved it". Saved = starred: saving a YT Music album
records an intent here, and the reconcile loop stars the Navidrome album as
soon as its first track is indexed.
"""

from __future__ import annotations

import hashlib
import logging
import random
import re
import threading
import time
from datetime import date, datetime, timedelta, timezone
from typing import Any

import httpx

from . import store
from .importer import ImportError_, _navidrome_get, _navidrome_post, _normalize

log = logging.getLogger("zayfetch.library")

HOME_TTL = 600
PAGE = 500
TOP_SONGS = 10
REDISCOVER_SIZE = 8
REDISCOVER_MIN = 4
REDISCOVER_AFTER = timedelta(days=21)
# An album save gives up when nothing matching was indexed in this window.
ALBUM_SAVE_EXPIRY = timedelta(days=3)

_home_cache: tuple[float, dict[str, Any]] | None = None
_home_lock = threading.Lock()


# --- Home ------------------------------------------------------------------


def all_songs() -> list[dict]:
    """Every song in Navidrome (search3 with an empty query, paged)."""
    songs: list[dict] = []
    for offset in range(0, 100_000, PAGE):
        body = _navidrome_get("search3", query="", songCount=str(PAGE), songOffset=str(offset),
                              artistCount="0", albumCount="0")
        page = (body.get("searchResult3") or {}).get("song") or []
        if isinstance(page, dict):
            page = [page]
        songs.extend(s for s in page if isinstance(s, dict))
        if len(page) < PAGE:
            break
    return songs


_identity_cache: tuple[float, set[tuple[str, str]]] | None = None
_identity_lock = threading.Lock()


def known_recordings(ttl: float = 90.0) -> set[tuple[str, str]]:
    """Normalized (artist, title) pairs for every library song, cached briefly.

    Used to keep suggestions from repeating songs that are already owned."""
    global _identity_cache
    with _identity_lock:
        if _identity_cache and _identity_cache[0] > time.monotonic():
            return _identity_cache[1]

    def norm(text: object) -> str:
        return re.sub(r"\s+", " ", str(text or "").casefold()).strip()

    identities: set[tuple[str, str]] = set()
    for song in all_songs():
        artist = norm(song.get("artist"))
        title = norm(song.get("title"))
        if title:
            identities.add((artist, title))
    with _identity_lock:
        _identity_cache = (time.monotonic() + ttl, identities)
        return identities


def _parse_time(value: object) -> datetime | None:
    if not value:
        return None
    try:
        return datetime.fromisoformat(str(value).replace("Z", "+00:00"))
    except ValueError:
        return None


def top_songs(songs: list[dict], limit: int = TOP_SONGS) -> list[dict]:
    played = [s for s in songs if (s.get("playCount") or 0) > 0]
    played.sort(key=lambda s: (-(s.get("playCount") or 0), -(_parse_time(s.get("played")) or datetime.min.replace(tzinfo=timezone.utc)).timestamp()))
    return played[:limit]


def rediscover(songs: list[dict], *, now: datetime | None = None, day: date | None = None,
               exclude: set[str] | None = None) -> list[dict]:
    """Songs you clearly like but haven't heard in a while.

    Qualifies: played at least twice, or liked; and not played in the last
    three weeks (liked songs never played count when added that long ago).
    The pick is shuffled with a date seed so it changes once a day."""
    now = now or datetime.now(timezone.utc)
    cutoff = now - REDISCOVER_AFTER
    exclude = exclude or set()
    pool = []
    for s in songs:
        if s.get("id") in exclude:
            continue
        plays = s.get("playCount") or 0
        if plays < 2 and not s.get("starred"):
            continue
        last = _parse_time(s.get("played")) or _parse_time(s.get("created"))
        if last is None or last > cutoff:
            continue
        pool.append(s)
    if len(pool) < REDISCOVER_MIN:
        return []
    seed = int(hashlib.sha1(str(day or now.date()).encode()).hexdigest()[:8], 16)
    pool.sort(key=lambda s: str(s.get("id")))
    random.Random(seed).shuffle(pool)
    return pool[:REDISCOVER_SIZE]


def home(*, refresh: bool = False) -> dict[str, Any]:
    global _home_cache
    with _home_lock:
        if not refresh and _home_cache and _home_cache[0] > time.monotonic():
            return _home_cache[1]
        songs = all_songs()
        top = top_songs(songs)
        result = {
            "topSongs": top,
            "rediscover": rediscover(songs, exclude={str(s.get("id")) for s in top}),
        }
        _home_cache = (time.monotonic() + HOME_TTL, result)
        return result


# --- Saved albums ----------------------------------------------------------


def find_album(title: str, artist: str) -> dict | None:
    """The indexed Navidrome album for a YTM album title/artist, if any."""
    want_title = _normalize(title)
    want_artist = _normalize(artist)
    if not want_title:
        return None
    body = _navidrome_get("search3", query=title, albumCount="20", songCount="0", artistCount="0")
    albums = (body.get("searchResult3") or {}).get("album") or []
    if isinstance(albums, dict):
        albums = [albums]
    named = [a for a in albums if isinstance(a, dict) and _normalize(str(a.get("name") or "")) == want_title]
    if not named:
        return None
    if want_artist:
        for a in named:
            got = _normalize(str(a.get("artist") or ""))
            if got and (got == want_artist or want_artist in got or got in want_artist):
                return a
    # A unique title match is good enough when the artist tag differs
    # (e.g. "Various Artists" vs the YTM channel name).
    return named[0] if len(named) == 1 else None


def save_album(title: str, artist: str) -> dict[str, Any]:
    """Record the intent and star right away when the album is already indexed."""
    row = store.create_album_save(title.strip(), artist.strip())
    album_id = _try_star(row)
    return {**row, "album_id": album_id}


def _try_star(row: dict[str, Any]) -> str | None:
    """Star the matching album; its id, or None while it isn't indexed."""
    try:
        album = find_album(row["title"], row["artist"])
        if not album:
            return None
        _navidrome_post("star", albumId=str(album["id"]))
    except (ImportError_, httpx.HTTPError) as exc:
        log.info("album save %r not starred yet: %s", row["title"], exc)
        return None
    store.finish_album_save(row["id"], str(album["id"]))
    log.info("saved album %r (%s)", row["title"], album["id"])
    return str(album["id"])


def reconcile_album_saves() -> None:
    """Star albums whose tracks have landed; drop stale intents. Never raises."""
    try:
        rows = store.pending_album_saves()
    except Exception as exc:  # noqa: BLE001
        log.info("could not list album saves: %s", exc)
        return
    now = datetime.now(timezone.utc)
    for row in rows:
        if _try_star(row):
            continue
        created = _parse_time(row.get("created_at"))
        if created and now - created > ALBUM_SAVE_EXPIRY:
            store.delete_album_save(row["id"])

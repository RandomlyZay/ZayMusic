"""Recover missing library track numbers from an exact YT Music album match."""

from __future__ import annotations

import logging
import re
import threading
import time
import unicodedata

from . import discovery
from .importer import _navidrome_get

log = logging.getLogger("zayfetch.album_order")
_cache: dict[tuple[str, str], tuple[float, list[dict]]] = {}
_lock = threading.Lock()


def _normalize(text: str) -> str:
    return re.sub(r"[\W_]+", " ", unicodedata.normalize("NFKC", text or "").casefold()).strip()


def _tracklists(title: str, artist: str) -> list[dict]:
    key = (_normalize(title), _normalize(artist))
    with _lock:
        hit = _cache.get(key)
        if hit and hit[0] > time.monotonic():
            return hit[1]
    matches = []
    results = discovery.ytmusic_call("search", f"{artist} {title}", filter="albums", limit=5)
    for result in results or []:
        names = [a.get("name", "") for a in result.get("artists") or []]
        if (_normalize(result.get("title", "")) != key[0]
                or not any(_normalize(name) == key[1] for name in names)
                or not result.get("browseId")):
            continue
        info = discovery.ytmusic_call("get_album", result["browseId"])
        if (_normalize(info.get("title", "")) == key[0]
                and any(_normalize(a.get("name", "")) == key[1]
                        for a in info.get("artists") or [])):
            matches.append(info)
    with _lock:
        if len(_cache) >= 64:
            _cache.pop(next(iter(_cache)))
        _cache[key] = (time.monotonic() + (3600 if matches else 60), matches)
    return matches


def _duration(track: dict) -> float:
    value = track.get("duration_seconds") or track.get("duration") or 0
    try:
        if isinstance(value, str) and ":" in value:
            total = 0.0
            for part in value.split(":"):
                total = total * 60 + float(part)
            return total
        return float(value)
    except (TypeError, ValueError):
        return 0


def match_order(songs: list[dict], tracks: list[dict]) -> dict[str, int]:
    """Require a unique match for every local song, including partial albums.

    Existing tags take precedence. Conflicting editions or ambiguous repeated
    titles yield no recovery rather than silently assigning the wrong order.
    """
    positions: dict[str, list[tuple[int, dict]]] = {}
    for number, track in enumerate(tracks, 1):
        positions.setdefault(_normalize(track.get("title", "")), []).append((number, track))
    order = {}
    used = set()
    for song in songs:
        matches = positions.get(_normalize(song.get("title", "")), [])
        duration = _duration(song)
        matches = [(n, t) for n, t in matches
                   if not duration or not _duration(t) or abs(duration - _duration(t)) <= 5]
        if len(matches) != 1:
            return {}
        number, _ = matches[0]
        if number in used or ((song.get("track") or 0) > 0 and song["track"] != number):
            return {}
        used.add(number)
        order[song["id"]] = number
    return order


def recover(album_id: str) -> dict[str, int]:
    album = _navidrome_get("getAlbum", id=album_id).get("album") or {}
    songs = album.get("song") or []
    if isinstance(songs, dict):
        songs = [songs]
    if not songs or all((s.get("track") or 0) > 0 for s in songs):
        return {}
    # YTM exposes a flat album tracklist, not per-disc positions.
    if any((s.get("discNumber") or 1) > 1 for s in songs):
        return {}
    title, artist = album.get("name", ""), album.get("artist", "")
    if not title or not artist:
        return {}
    try:
        orders = [match_order(songs, info.get("tracks") or [])
                  for info in _tracklists(title, artist)]
        orders = [order for order in orders if order]
        if orders and all(order == orders[0] for order in orders):
            return orders[0]
    except Exception as exc:
        log.info("album order lookup unavailable for %r: %s", title, exc)
    return {}

"""Deezer canonical metadata for manual tag repair.

Not part of search: YouTube Music + YouTube answer *what to play*, and
this module is consulted only when the user asks to fix a song's tags
(``GET /metadata/lookup``). Answers *what the music canonically is* —
title, artist, album, duration, popularity (``rank``), explicit flag,
ISRC, cover.

Deezer's public API needs no key and no signup, but it is called
server-side only (no CORS involved). Endpoints used:

- ``GET /search?q=&limit=`` — canonical tracks for a query.
- ``GET /album/{id}`` — full ordered tracklist for an album.

Deezer publishes no rate limits; a small TTL cache plus backoff on
429/quota errors keeps us polite. Every public function here is total:
failures return ``None``/``{}`` and never raise, so a catalog outage can
never fail a lookup.
"""

from __future__ import annotations

import logging
import re
import threading
import time
from dataclasses import dataclass, field

import httpx

log = logging.getLogger("zayfetch.catalog")

BASE = "https://api.deezer.com"
_UA = "ZayFetch/0.1.0 (+https://github.com/zaymusic)"

_SEARCH_TTL = 300.0  # track search results barely change minute to minute
_ALBUM_TTL = 3600.0  # album tracklists barely change, period
_TIMEOUT = 8.0
_MAX_SEARCH_CACHE = 128
_MAX_ALBUM_CACHE = 64


@dataclass
class CatalogTrack:
    id: int
    title: str
    artist: str
    album: str = ""
    album_id: int | None = None
    duration: float = 0.0
    rank: int = 0
    explicit: bool | None = None
    isrc: str = ""
    cover: str = ""


@dataclass
class CatalogAlbum:
    id: int
    title: str
    artist: str = ""
    year: int | None = None
    explicit: bool | None = None
    cover: str = ""
    track_count: int = 0
    tracks: list[CatalogTrack] = field(default_factory=list)


@dataclass
class CatalogResult:
    query: str
    tracks: list[CatalogTrack] = field(default_factory=list)
    album: CatalogAlbum | None = None


# ---------------------------------------------------------------------------
# TTL cache + backoff
# ---------------------------------------------------------------------------

_cache: dict[str, tuple[float, object]] = {}
_cache_lock = threading.Lock()
_backoff_until = 0.0
_backoff_secs = 30.0


def _cache_get(key: str, *, allow_stale: bool = False) -> object | None:
    with _cache_lock:
        hit = _cache.get(key)
    if not hit:
        return None
    expiry, value = hit
    if expiry > time.monotonic() or allow_stale:
        return value
    return None


def _cache_put(key: str, value: object, ttl: float) -> None:
    with _cache_lock:
        if len(_cache) >= _MAX_SEARCH_CACHE + _MAX_ALBUM_CACHE:
            _cache.pop(next(iter(_cache)))
        _cache[key] = (time.monotonic() + ttl, value)


def _in_backoff() -> bool:
    return time.monotonic() < _backoff_until


def _note_success() -> None:
    global _backoff_secs
    _backoff_secs = 30.0


def _note_throttled() -> None:
    global _backoff_until, _backoff_secs
    _backoff_until = time.monotonic() + _backoff_secs
    log.warning("deezer throttled — backing off for %.0fs", _backoff_secs)
    _backoff_secs = min(_backoff_secs * 2, 300.0)


def clear_cache() -> None:
    """Test hook: drop all cached catalog data and backoff state."""
    global _backoff_until, _backoff_secs
    with _cache_lock:
        _cache.clear()
    _backoff_until = 0.0
    _backoff_secs = 30.0


# ---------------------------------------------------------------------------
# HTTP
# ---------------------------------------------------------------------------


def _get(path: str, params: dict | None = None) -> dict | None:
    """GET one Deezer JSON object. None on any failure; throttles back off."""
    url = f"{BASE}{path}"
    try:
        resp = httpx.get(url, params=params or {}, timeout=_TIMEOUT,
                         headers={"User-Agent": _UA})
    except httpx.HTTPError as e:
        log.info("deezer request failed for %s: %s", path, e)
        return None
    if resp.status_code == 429 or resp.status_code >= 500:
        _note_throttled()
        return None
    if resp.status_code != 200:
        log.info("deezer %s → HTTP %s", path, resp.status_code)
        return None
    try:
        body = resp.json()
    except ValueError:
        return None
    if isinstance(body, dict) and body.get("error"):
        # Quota errors arrive as HTTP 200 with an error envelope.
        _note_throttled()
        log.info("deezer %s → API error: %s", path, body["error"])
        return None
    _note_success()
    return body if isinstance(body, dict) else None


# ---------------------------------------------------------------------------
# parsing
# ---------------------------------------------------------------------------


def _parse_track(item: dict) -> CatalogTrack | None:
    if not isinstance(item, dict) or not item.get("id"):
        return None
    artist = item.get("artist") or {}
    album = item.get("album") or {}
    explicit = item.get("explicit_lyrics")
    try:
        track_id = int(item.get("id"))
    except (TypeError, ValueError):
        return None
    try:
        duration = float(item.get("duration") or 0)
    except (TypeError, ValueError):
        duration = 0.0
    try:
        rank = int(item.get("rank") or 0)
    except (TypeError, ValueError):
        rank = 0
    album_id = album.get("id")
    try:
        album_id = int(album_id) if album_id is not None else None
    except (TypeError, ValueError):
        album_id = None
    return CatalogTrack(
        id=track_id,
        title=str(item.get("title") or ""),
        artist=str(artist.get("name") or ""),
        album=str(album.get("title") or ""),
        album_id=album_id,
        duration=duration,
        rank=rank,
        explicit=explicit if isinstance(explicit, bool) else None,
        isrc=str(item.get("isrc") or ""),
        cover=str(album.get("cover_medium") or album.get("cover") or ""),
    )


def _parse_album_detail(body: dict) -> CatalogAlbum | None:
    if not isinstance(body, dict) or not body.get("id"):
        return None
    try:
        album_id = int(body.get("id"))
    except (TypeError, ValueError):
        return None
    artist = body.get("artist") or {}
    explicit = body.get("explicit_lyrics")
    year = None
    release = body.get("release_date") or ""
    m = re.match(r"(\d{4})", str(release))
    if m:
        year = int(m.group(1))
    try:
        track_count = int(body.get("nb_tracks") or 0)
    except (TypeError, ValueError):
        track_count = 0
    tracks: list[CatalogTrack] = []
    raw_tracks = body.get("tracks") or {}
    for item in raw_tracks.get("data") or []:
        # Album-scoped track entries lack the album block — inherit it.
        if isinstance(item, dict) and not item.get("album"):
            item = {
                **item,
                "album": {
                    "id": album_id,
                    "title": body.get("title") or "",
                    "cover_medium": body.get("cover_medium") or "",
                },
            }
        t = _parse_track(item)
        if t and t.title:
            tracks.append(t)
    return CatalogAlbum(
        id=album_id,
        title=str(body.get("title") or ""),
        artist=str(artist.get("name") or ""),
        year=year,
        explicit=explicit if isinstance(explicit, bool) else None,
        cover=str(body.get("cover_medium") or body.get("cover") or ""),
        track_count=track_count or len(tracks),
        tracks=tracks,
    )


# ---------------------------------------------------------------------------
# public lookup
# ---------------------------------------------------------------------------


def _search_tracks(query: str, limit: int = 25) -> list[CatalogTrack]:
    key = f"search:{query.casefold().strip()}:{limit}"
    hit = _cache_get(key)
    if isinstance(hit, list):
        return hit
    if _in_backoff():
        stale = _cache_get(key, allow_stale=True)
        return stale if isinstance(stale, list) else []
    body = _get("/search", {"q": query, "limit": limit})
    tracks: list[CatalogTrack] = []
    if body:
        for item in body.get("data") or []:
            t = _parse_track(item)
            if t and t.title and t.artist:
                tracks.append(t)
    _cache_put(key, tracks, _SEARCH_TTL)
    return tracks


def album_detail(album_id: int) -> CatalogAlbum | None:
    """Full ordered tracklist for one Deezer album. None on any failure."""
    key = f"album:{album_id}"
    hit = _cache_get(key)
    if isinstance(hit, CatalogAlbum):
        return hit
    if _in_backoff():
        stale = _cache_get(key, allow_stale=True)
        return stale if isinstance(stale, CatalogAlbum) else None
    body = _get(f"/album/{album_id}")
    album = _parse_album_detail(body) if body else None
    if album:
        _cache_put(key, album, _ALBUM_TTL)
    return album


def album_years(album_ids: list[int], *, fetch_limit: int = 3) -> dict[int, int]:
    """Release years for albums, best effort. Track search results carry no
    date, so fetch at most ``fetch_limit`` uncached album details (in
    parallel); cached details are free. Never raises."""
    years: dict[int, int] = {}
    missing: list[int] = []
    for album_id in dict.fromkeys(album_ids):
        hit = _cache_get(f"album:{album_id}")
        if isinstance(hit, CatalogAlbum):
            if hit.year:
                years[album_id] = hit.year
        elif len(missing) < fetch_limit:
            missing.append(album_id)
    if missing and not _in_backoff():
        from concurrent.futures import ThreadPoolExecutor

        def one(album_id: int) -> CatalogAlbum | None:
            try:
                return album_detail(album_id)
            except Exception as e:  # noqa: BLE001
                log.info("deezer album year lookup failed for %s: %s", album_id, e)
                return None

        with ThreadPoolExecutor(max_workers=len(missing)) as pool:
            for album_id, album in zip(missing, pool.map(one, missing)):
                if album and album.year:
                    years[album_id] = album.year
    return years


def lookup(query: str, limit: int = 25) -> CatalogResult | None:
    """Canonical tracks for a query, plus the full album when the query
    explicitly names one. Never raises; None means "no authority"."""
    try:
        q = (query or "").strip()
        if not q:
            return None
        tracks = _search_tracks(q, limit)
        if not tracks:
            return None
        result = CatalogResult(query=q, tracks=tracks)
        album = _pick_album(q, tracks)
        if album is not None:
            detail = album_detail(album.id)
            if detail and detail.tracks:
                result.album = detail
            else:
                # Search grouping still names the record even when the
                # detail fetch fails — keep the shell so the card pins.
                result.album = album
        return result
    except Exception as e:
        log.info("catalog lookup failed for %r: %s", query, e)
        return None


def _pick_album(query: str, tracks: list[CatalogTrack]) -> CatalogAlbum | None:
    """The album the query explicitly names, if any. Group the track
    results by album and keep the most popular album-intent match."""
    from .ranking import is_album_intent

    by_album: dict[int, list[CatalogTrack]] = {}
    for t in tracks:
        if t.album_id and t.album:
            by_album.setdefault(t.album_id, []).append(t)
    best: CatalogAlbum | None = None
    best_rank = -1
    for album_id, ts in by_album.items():
        album_title = ts[0].album
        artist = ts[0].artist
        if not is_album_intent(query, album_title, artist):
            continue
        popularity = sum(t.rank for t in ts)
        if popularity > best_rank:
            best_rank = popularity
            best = CatalogAlbum(
                id=album_id,
                title=album_title,
                artist=artist,
                cover=ts[0].cover,
                track_count=len(ts),
                tracks=[],
            )
    return best


# ---------------------------------------------------------------------------
# upload-title helpers (shared with the iTunes canonical matcher)
# ---------------------------------------------------------------------------

_CORP_SUFFIX_RE = re.compile(
    r"\s+(media|vevo|records|music|official|tv|hd|hq|productions|entertainment)$",
    re.I,
)
_DECORATION_RE = re.compile(
    r"\s*[\(\[]?\s*(official audio|official lyric video|official lyrics|"
    r"official video|official mv|audio|lyrics|lyric video|music video|\bmv\b|"
    r"visualizer|visualiser|hq|hd|4k)\s*[\)\]]?\s*$",
    re.I,
)
_SPLIT_RE = re.compile(r"\s[-–—:|]\s+")


def _alnum(text: str) -> str:
    return re.sub(r"[^a-z0-9]", "", (text or "").casefold())


def _strip_decorations(title: str) -> str:
    t = (title or "").strip()
    prev = None
    while prev != t:
        prev = t
        t = _DECORATION_RE.sub("", t).strip()
    return t


def _core_artist(channel: str) -> str:
    c = (channel or "").removesuffix(" - Topic").strip()
    c = _CORP_SUFFIX_RE.sub("", c).strip()
    return c


def _guess_pairs(title: str, channel: str) -> list[tuple[str, str]]:
    """(artist, title) guesses for one upload. Every artist guess comes
    from the upload's own text — never invented (same rule as the
    MusicBrainz repair)."""
    t = (title or "").strip()
    c = _core_artist(channel)
    pairs: list[tuple[str, str]] = []
    if c and t:
        pairs.append((c, t))
        pairs.append((c, _strip_decorations(t)))
    m = _SPLIT_RE.split(t, maxsplit=1)
    if len(m) == 2 and m[0].strip() and m[1].strip():
        pairs.append((m[0].strip(), m[1].strip()))
        pairs.append((m[0].strip(), _strip_decorations(m[1].strip())))
    seen: set[tuple[str, str]] = set()
    out: list[tuple[str, str]] = []
    for artist, name in pairs:
        key = (_alnum(artist), _alnum(name))
        if key[0] and key[1] and key not in seen:
            seen.add(key)
            out.append((artist, name))
    return out

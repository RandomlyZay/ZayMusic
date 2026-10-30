"""Discovery helpers — radio graph + optional Last.fm signals.

- WHERE the bytes come from: YouTube Music watch-playlist radio
  (their similarity graph, no Premium needed for the track list).
- Optional (free key): Last.fm for spelling correction and
  "people who listen also listen".

Every public function here is total: failures return []/None and
never raise, so a discovery outage can never fail search or radio.
"""

from __future__ import annotations

import logging
import os
import re
import threading
import time
from dataclasses import dataclass, field

import httpx

log = logging.getLogger("zayfetch.discovery")

_LASTFM_BASE = "https://ws.audioscrobbler.com/2.0/"
_UA = "ZayFetch/0.1.0 (+https://github.com/zaymusic)"
_BROWSER_UA = (
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 "
    "(KHTML, like Gecko) Chrome/146.0.0.0 Safari/537.36"
)
_TIMEOUT = 8.0

_SEARCH_TTL = 300.0
_MAX_CACHE = 128

_cache: dict[str, tuple[float, object]] = {}
_cache_lock = threading.Lock()


def _cache_get(key: str) -> object | None:
    with _cache_lock:
        hit = _cache.get(key)
    if not hit:
        return None
    expiry, value = hit
    if expiry > time.monotonic():
        return value
    return None


def _cache_put(key: str, value: object, ttl: float = _SEARCH_TTL) -> None:
    with _cache_lock:
        if len(_cache) >= _MAX_CACHE:
            _cache.pop(next(iter(_cache)))
        _cache[key] = (time.monotonic() + ttl, value)


def clear_cache() -> None:
    """Test hook."""
    with _cache_lock:
        _cache.clear()


def _lastfm_key() -> str:
    return os.environ.get("LASTFM_API_KEY", "").strip()


# ---------------------------------------------------------------------------
# Last.fm correction + similar (optional key)
# ---------------------------------------------------------------------------


@dataclass
class SimilarTrack:
    artist: str
    title: str


def lastfm_correction(artist: str, title: str) -> dict | None:
    """Corrected artist/title via track.getInfo autocorrect. None when
    unknown or no key. Never raises."""
    try:
        key = _lastfm_key()
        if not key or not artist.strip() or not title.strip():
            return None
        cache_key = f"lfmcorr:{artist.casefold()}:{title.casefold()}"
        hit = _cache_get(cache_key)
        if isinstance(hit, dict) or hit is None:
            # None is a valid negative cache only when explicitly stored
            pass
        try:
            resp = httpx.get(
                _LASTFM_BASE,
                params={
                    "method": "track.getInfo",
                    "artist": artist,
                    "track": title,
                    "api_key": key,
                    "format": "json",
                    "autocorrect": 1,
                },
                timeout=_TIMEOUT,
                headers={"User-Agent": _UA},
            )
        except httpx.HTTPError as e:
            log.info("last.fm correction failed: %s", e)
            return None
        if resp.status_code != 200:
            return None
        try:
            body = resp.json()
        except ValueError:
            return None
        track = body.get("track") if isinstance(body, dict) else None
        if not isinstance(track, dict):
            return None
        name = str(track.get("name") or "")
        art = track.get("artist") or {}
        aname = str(art.get("name") or "") if isinstance(art, dict) else ""
        if not name or not aname:
            return None
        if aname.casefold() == artist.casefold() and name.casefold() == title.casefold():
            return None
        out = {"artist": aname, "title": name}
        _cache_put(cache_key, out)
        return out
    except Exception as e:
        log.info("last.fm correction failed: %s", e)
        return None


def lastfm_similar(artist: str, title: str, limit: int = 10) -> list[SimilarTrack]:
    """Crowd-similar tracks ("people who listen also listen").
    Empty when unknown or no key. Never raises."""
    try:
        key = _lastfm_key()
        if not key or not artist.strip() or not title.strip():
            return []
        cache_key = f"lfmsim:{artist.casefold()}:{title.casefold()}:{limit}"
        hit = _cache_get(cache_key)
        if isinstance(hit, list):
            return hit
        try:
            resp = httpx.get(
                _LASTFM_BASE,
                params={
                    "method": "track.getSimilar",
                    "artist": artist,
                    "track": title,
                    "api_key": key,
                    "format": "json",
                    "autocorrect": 1,
                    "limit": limit,
                },
                timeout=_TIMEOUT,
                headers={"User-Agent": _UA},
            )
        except httpx.HTTPError as e:
            log.info("last.fm similar failed: %s", e)
            return []
        if resp.status_code != 200:
            return []
        try:
            body = resp.json()
        except ValueError:
            return []
        sim = body.get("similartracks") if isinstance(body, dict) else None
        tracks = sim.get("track") if isinstance(sim, dict) else None
        if isinstance(tracks, dict):
            tracks = [tracks]
        out: list[SimilarTrack] = []
        for t in tracks or []:
            if not isinstance(t, dict):
                continue
            name = str(t.get("name") or "")
            art = t.get("artist") or {}
            aname = str(art.get("name") or "") if isinstance(art, dict) else ""
            if name and aname:
                out.append(SimilarTrack(artist=aname, title=name))
            if len(out) >= limit:
                break
        _cache_put(cache_key, out)
        return out
    except Exception as e:
        log.info("last.fm similar failed: %s", e)
        return []


def lastfm_artist_top(artist: str, limit: int = 8) -> list[SimilarTrack]:
    """Top tracks for one artist (obscure fallback). Never raises."""
    try:
        key = _lastfm_key()
        if not key or not artist.strip():
            return []
        cache_key = f"lfmtop:{artist.casefold()}:{limit}"
        hit = _cache_get(cache_key)
        if isinstance(hit, list):
            return hit
        try:
            resp = httpx.get(
                _LASTFM_BASE,
                params={
                    "method": "artist.getTopTracks",
                    "artist": artist,
                    "api_key": key,
                    "format": "json",
                    "autocorrect": 1,
                    "limit": limit,
                },
                timeout=_TIMEOUT,
                headers={"User-Agent": _UA},
            )
        except httpx.HTTPError as e:
            log.info("last.fm top tracks failed: %s", e)
            return []
        if resp.status_code != 200:
            return []
        try:
            body = resp.json()
        except ValueError:
            return []
        top = body.get("toptracks") if isinstance(body, dict) else None
        tracks = top.get("track") if isinstance(top, dict) else None
        if isinstance(tracks, dict):
            tracks = [tracks]
        out: list[SimilarTrack] = []
        for t in tracks or []:
            if not isinstance(t, dict):
                continue
            name = str(t.get("name") or "")
            art = t.get("artist") or {}
            aname = str(art.get("name") or artist) if isinstance(art, dict) else artist
            if name and aname:
                out.append(SimilarTrack(artist=aname, title=name))
            if len(out) >= limit:
                break
        _cache_put(cache_key, out)
        return out
    except Exception as e:
        log.info("last.fm top tracks failed: %s", e)
        return []


# ---------------------------------------------------------------------------
# YTM suggestions + radio graph (no Premium needed for the track lists)
# ---------------------------------------------------------------------------


def _browser_auth() -> dict | None:
    """Browser-auth headers for ytmusicapi built from the stored cookies.

    Lets the app call YouTube Music's own API as the connected account, so
    suggestions, radio, album data, and the library come from Google's session
    instead of an anonymous visitor.

    The ``origin`` header is mandatory: for browser auth ytmusicapi recomputes
    ``SAPISIDHASH`` from ``"<sapisid> <origin>"``, so without it the hash is
    built with the literal string ``None`` and every account-scoped call is
    treated as signed out (empty library, silent fallback to anonymous)."""
    try:
        from ytmusicapi.auth.browser import get_authorization, sapisid_from_cookie

        from . import downloader

        cookies = downloader.read_cookies_dict()
        if not cookies:
            return None
        cookie = "; ".join(f"{k}={v}" for k, v in cookies.items())
        sapisid = sapisid_from_cookie(cookie)
        if not sapisid:
            return None
        return {
            "cookie": cookie,
            "authorization": get_authorization(sapisid),
            "x-goog-authuser": "0",
            "x-goog-visitor-id": cookies.get("VISITOR_INFO1_LIVE", ""),
            "origin": "https://music.youtube.com",
            "x-origin": "https://music.youtube.com",
            "user-agent": _BROWSER_UA,
        }
    except Exception as e:
        log.info("could not build YTM browser auth: %s", e)
        return None


def _ytmusic_clients(authenticated: bool = True):
    """Candidate YTMusic clients, best first: OAuth, browser cookies, anonymous.

    Built lazily so a working OAuth client never pays for the others."""
    from ytmusicapi import YTMusic

    from . import downloader

    if authenticated:
        if downloader.oauth_configured():
            try:
                from ytmusicapi.auth.oauth import OAuthCredentials

                credentials = OAuthCredentials(
                    downloader.oauth_client_id(), downloader.oauth_client_secret()
                )
                yield YTMusic(str(downloader.oauth_path()), oauth_credentials=credentials)
            except Exception as e:
                # A revoked refresh token must degrade to cookies, not raise.
                log.info("OAuth YTM client failed, falling back to cookies: %s", e)
        auth = _browser_auth()
        if auth:
            try:
                yield YTMusic(auth)
            except Exception as e:
                log.info("authenticated YTM client failed, using anonymous: %s", e)
    yield YTMusic()


def ytmusic_client(authenticated: bool = True):
    """A YTMusic client; bound to the connected account when possible.

    Preference order (§5.4): a self-renewing OAuth refresh token (API-only,
    never expires from cookie rotation), then the stored browser cookies,
    then anonymous."""
    return next(_ytmusic_clients(authenticated))


def ytmusic_call(method: str, *args, **kwargs):
    """Call a YTMusic method on each client in preference order.

    A rejected OAuth token falls through to the browser cookies (YouTube
    answers OAuth browse calls with HTTP 400 at times), and a stale cookie
    falls through to anonymous: search/radio never go down with the account.
    Private data (library, Liked Music) still needs one authenticated client
    to work; the last error is raised when every client fails."""
    error: Exception | None = None
    for client in _ytmusic_clients():
        try:
            return getattr(client, method)(*args, **kwargs)
        except Exception as e:
            log.info("YTM %s failed on %s client (%s); trying the next one",
                     method, getattr(client, "auth_type", "?"), e)
            error = e
    assert error is not None
    raise error


def ytm_suggestions(query: str, limit: int = 8) -> list[str]:
    """As-you-type suggestions from YTM. Never raises."""
    try:
        q = (query or "").strip()
        if not q:
            return []
        out = ytmusic_call("get_search_suggestions", q) or []
        seen: set[str] = set()
        clean: list[str] = []
        for s in out:
            text = s if isinstance(s, str) else str(s or "")
            text = text.strip()
            if text and text.casefold() not in seen:
                seen.add(text.casefold())
                clean.append(text)
            if len(clean) >= limit:
                break
        return clean
    except Exception as e:
        log.info("ytm suggestions failed for %r: %s", query, e)
        return []


@dataclass
class RadioTrack:
    videoId: str
    title: str
    artist: str
    thumbnails: list[dict] = field(default_factory=list)
    durationSec: float = 0.0


def ytm_radio(video_id: str, limit: int = 25) -> list[RadioTrack]:
    """YTM's own "what plays next" graph for one video.
    Uses the connected account when available. Never raises."""
    try:
        import re as _re

        if not _re.fullmatch(r"[A-Za-z0-9_-]{11}", video_id or ""):
            return []
        data = ytmusic_call("get_watch_playlist", videoId=video_id, radio=True, limit=limit) or {}
        tracks = data.get("tracks") or []
        out: list[RadioTrack] = []
        for t in tracks:
            if not isinstance(t, dict):
                continue
            vid = t.get("videoId") or ""
            if not vid or vid == video_id:
                continue
            title = str(t.get("title") or "")
            if not title:
                continue
            artists = t.get("artists") or []
            artist = artists[0].get("name", "") if artists and isinstance(artists[0], dict) else ""
            thumbs = [
                {"url": th.get("url", ""), "width": th.get("width", 0), "height": th.get("height", 0)}
            for th in (t.get("thumbnails") or t.get("thumbnail") or [])
                if isinstance(th, dict) and th.get("url")
            ]
            dur = t.get("lengthSeconds") or t.get("duration_seconds") or t.get("length") or 0
            if isinstance(dur, str):
                try:
                    parts = [int(x) for x in dur.split(":")]
                    dur = sum(p * (60 ** (len(parts) - i - 1)) for i, p in enumerate(parts))
                except Exception:
                    dur = 0
            try:
                dur = float(dur or 0)
            except (TypeError, ValueError):
                dur = 0.0
            out.append(RadioTrack(videoId=vid, title=title, artist=str(artist or ""), thumbnails=thumbs, durationSec=dur))
            if len(out) >= limit:
                break
        return out
    except Exception as e:
        log.info("ytm radio failed for %s: %s", video_id, e)
        return []


_ALNUM_RE = re.compile(r"[^a-z0-9]")


def _alnum(text: str) -> str:
    return _ALNUM_RE.sub("", (text or "").casefold())


def split_artist_title(query: str) -> tuple[str, str]:
    """Best-effort "Artist - Title" / "Title by Artist" parse for
    correction + similar lookups. Never raises."""
    try:
        q = (query or "").strip()
        for sep in (" - ", " – ", " — ", " | ", " : "):
            if sep in q:
                left, _, right = q.partition(sep)
                if left.strip() and right.strip():
                    return left.strip(), right.strip()
        m = re.match(r"(.+?)\s+by\s+(.+)", q, re.I)
        if m and m.group(1).strip() and m.group(2).strip():
            return m.group(2).strip(), m.group(1).strip()
        return "", q
    except Exception:
        return "", query or ""

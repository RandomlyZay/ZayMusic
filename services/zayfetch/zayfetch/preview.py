"""Direct audio URLs for new-music playback.

Extraction is the slow part of playing a track that isn't in the library
(several seconds of YouTube round trips), so this module:

- caches each googlevideo URL for as long as YouTube says it is valid
  (its ``expire=`` parameter), not a fixed few minutes;
- shares one extraction between every caller asking for the same video
  (the player and a warm-up request racing each other cost one extraction);
- runs yt-dlp in-process with one long-lived ``YoutubeDL`` per worker
  thread, skipping interpreter start-up and reusing the player-JS cache,
  falling back to the CLI on unexpected errors;
- lets the client warm the next queue items in the background.
"""

from __future__ import annotations

import logging
import os
import re
import subprocess
import threading
import time
from collections import OrderedDict
from concurrent.futures import Future, ThreadPoolExecutor
from typing import Any

from . import downloader

log = logging.getLogger("zayfetch.preview")

CACHE_MAX = 1024
# googlevideo URLs live ~6 h; without an expire hint assume a safe 5 h.
FALLBACK_TTL = 5 * 3600
# Stop serving a URL this long before YouTube's expiry so a long track
# never runs out mid-stream.
EXPIRY_MARGIN = 600
WARM_WORKERS = 2
FORMAT = "bestaudio[ext=m4a]/bestaudio"

_cache: OrderedDict[str, tuple[float, str]] = OrderedDict()
_inflight: dict[str, Future[str]] = {}
_lock = threading.Lock()
_warm_pool = ThreadPoolExecutor(max_workers=WARM_WORKERS, thread_name_prefix="preview-warm")
_local = threading.local()


class Unavailable(Exception):
    """No playable audio URL could be extracted for the video."""


def url_ttl(url: str, now: float | None = None) -> float:
    """Seconds this URL can still be handed out (0 = don't cache)."""
    m = re.search(r"[?&/]expire[=/](\d+)", url)
    if not m:
        return FALLBACK_TTL
    remaining = int(m.group(1)) - (now if now is not None else time.time()) - EXPIRY_MARGIN
    return max(0.0, remaining)


def cached(video_id: str) -> str | None:
    with _lock:
        return _cached_locked(video_id)


def _cached_locked(video_id: str) -> str | None:
    hit = _cache.get(video_id)
    if not hit:
        return None
    if hit[0] <= time.monotonic():
        _cache.pop(video_id, None)
        return None
    _cache.move_to_end(video_id)
    return hit[1]


def _store(video_id: str, url: str) -> None:
    ttl = url_ttl(url)
    if ttl <= 0:
        return
    with _lock:
        _cache[video_id] = (time.monotonic() + ttl, url)
        _cache.move_to_end(video_id)
        while len(_cache) > CACHE_MAX:
            _cache.popitem(last=False)


def _claim(video_id: str) -> tuple[Future[str], bool]:
    """The shared future for this video, and whether the caller must run it."""
    with _lock:
        hit = _cached_locked(video_id)
        if hit:
            done: Future[str] = Future()
            done.set_result(hit)
            return done, False
        pending = _inflight.get(video_id)
        if pending:
            return pending, False
        fut: Future[str] = Future()
        _inflight[video_id] = fut
        return fut, True


def _run(video_id: str, fut: Future[str]) -> None:
    started = time.monotonic()
    try:
        url = _extract(video_id)
        _store(video_id, url)
        fut.set_result(url)
        log.info("preview: extracted %s extract_ms=%d", video_id, (time.monotonic() - started) * 1000)
    except BaseException as e:  # noqa: BLE001 — waiters must always be released
        log.info("preview: extraction failed for %s after %d ms: %s",
                 video_id, (time.monotonic() - started) * 1000, e)
        fut.set_exception(e if isinstance(e, Exception) else Unavailable(str(e)))
    finally:
        with _lock:
            if _inflight.get(video_id) is fut:
                _inflight.pop(video_id, None)


def audio_url(video_id: str) -> str:
    """Playable googlevideo URL, raising ``Unavailable`` on failure.

    Blocks the calling thread; a concurrent request for the same video
    waits on the same extraction instead of starting another."""
    fut, owner = _claim(video_id)
    if owner:
        _run(video_id, fut)
    return fut.result()


def warm(video_ids: list[str]) -> int:
    """Start background extraction for videos not already cached/in flight."""
    started = 0
    for vid in video_ids:
        fut, owner = _claim(vid)
        if owner:
            _warm_pool.submit(_run, vid, fut)
            started += 1
    return started


def _extract(video_id: str) -> str:
    url = extract_once(video_id, with_cookies=True)
    if url is None:
        raise Unavailable("Audio is unavailable for this track")
    return url


def extract_once(video_id: str, *, with_cookies: bool) -> str | None:
    """One extraction attempt; in-process first, the CLI if that breaks."""
    try:
        return _extract_in_process(video_id, with_cookies=with_cookies)
    except _NoAudio:
        return None
    except Exception as e:  # noqa: BLE001 — any embedding problem falls back to the CLI
        log.warning("preview: in-process yt-dlp failed for %s (%s); using the CLI", video_id, e)
        return _extract_cli(video_id, with_cookies=with_cookies)


class _NoAudio(Exception):
    """yt-dlp ran fine but this video has no usable audio format."""


def _ydl_options(with_cookies: bool) -> dict[str, Any]:
    """In-process equivalent of ``downloader.ytdlp_cmd`` for URL extraction."""
    youtube: dict[str, list[str]] = {}
    opts: dict[str, Any] = {
        "quiet": True,
        "no_warnings": True,
        "noplaylist": True,
        "format": FORMAT,
        "skip_download": True,
        "socket_timeout": 20,
        "extractor_args": {"youtube": youtube},
    }
    cookies = downloader.cookies_file() if with_cookies else ""
    if cookies:
        opts["cookiefile"] = cookies
        youtube["player_client"] = ["web_music"]
        # The per-request client-config download adds ~1 s and is not
        # needed for web_music with a PO token.
        youtube["player_skip"] = ["configs"]
        if downloader.POT_PROVIDER_URL:
            opts["extractor_args"]["youtubepot-bgutilhttp"] = {"base_url": [downloader.POT_PROVIDER_URL]}
    try:
        import curl_cffi  # noqa: F401
        from yt_dlp.networking.impersonate import ImpersonateTarget

        opts["impersonate"] = ImpersonateTarget.from_str("chrome")
    except ImportError:
        pass
    return opts


def _ydl(with_cookies: bool):
    """This thread's YoutubeDL, rebuilt whenever the cookies file changes."""
    import yt_dlp

    cookies = downloader.cookies_file() if with_cookies else ""
    try:
        stamp = os.stat(cookies).st_mtime_ns if cookies else 0
    except OSError:
        stamp = 0
    key = (with_cookies, cookies, stamp)
    instances: dict[bool, tuple[tuple, Any]] = getattr(_local, "instances", None) or {}
    _local.instances = instances
    current = instances.get(with_cookies)
    if current and current[0] == key:
        return current[1]
    ydl = yt_dlp.YoutubeDL(_ydl_options(with_cookies))
    # The jar is loaded; stop YoutubeDL writing it back over the file the
    # session manager owns.
    ydl.params["cookiefile"] = None
    instances[with_cookies] = (key, ydl)
    return ydl


def _extract_in_process(video_id: str, *, with_cookies: bool) -> str:
    from yt_dlp.utils import DownloadError as YtdlpDownloadError

    ydl = _ydl(with_cookies)
    try:
        info = ydl.extract_info(f"https://www.youtube.com/watch?v={video_id}", download=False)
    except YtdlpDownloadError as e:
        raise _NoAudio(str(e)) from e
    url = (info or {}).get("url") or ""
    if not url.startswith("https://"):
        raise _NoAudio("no direct audio URL")
    return url


def _extract_cli(video_id: str, *, with_cookies: bool) -> str | None:
    try:
        result = subprocess.run(
            downloader.ytdlp_cmd("--no-playlist", "-f", FORMAT, "-g",
                                 f"https://www.youtube.com/watch?v={video_id}",
                                 with_cookies=with_cookies),
            capture_output=True, text=True, timeout=60,
        )
    except (OSError, subprocess.TimeoutExpired) as e:
        log.info("preview extraction failed for %s: %s", video_id, e)
        return None
    if result.returncode != 0 or not result.stdout.strip():
        return None
    url = result.stdout.strip().splitlines()[0]
    return url if url.startswith("https://") else None

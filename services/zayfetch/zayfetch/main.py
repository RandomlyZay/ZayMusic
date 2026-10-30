"""ZayFetch FastAPI service (§8.3)."""

from __future__ import annotations

import asyncio
import logging
import os
import re
import shutil
import subprocess
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from typing import Any

import httpx
from fastapi import Body, Depends, FastAPI, Header, HTTPException, Query, Request
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import StreamingResponse
from pydantic import ValidationError
from starlette.concurrency import run_in_threadpool

from . import catalog, discovery, downloader, importer, loudness, mover, ranking, session, store, tagger
from . import album_order, library
from . import preview as previews
from .downloader import DownloadError, ytdlp_cmd
from .models import (
    Candidate,
    Correction,
    FetchJob,
    HealthResponse,
    JobResponse,
    JobResult,
    JobsResponse,
    MetadataCandidate,
    MetadataLookupResponse,
    OkResponse,
    Overrides,
    PlaylistImportsResponse,
    PreviewWarmRequest,
    AlbumSaveRequest,
    RadioResponse,
    ResolveResponse,
    SearchResponse,
    SongTagsRequest,
    SongTagsResponse,
    SuggestResponse,
    AlbumResponse,
    AlbumResult,
    Thumbnail,
    YouTubePlaylistsResponse,
    YouTubePlaylistTracksResponse,
)
from .ranking import album_score, is_album_intent, parse_views, rank

logging.basicConfig(level=logging.INFO, format="%(asctime)s %(name)s %(levelname)s %(message)s")
log = logging.getLogger("zayfetch")

app = FastAPI(title="ZayFetch", version="0.1.0")

app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_methods=["*"],
    allow_headers=["*"],
)

TOKEN = os.environ.get("ZAYFETCH_TOKEN", "")
MUSIC_DIR = Path(os.environ.get("MUSIC_DIR", "/music"))
NAVIDROME_URL = os.environ.get("NAVIDROME_URL", "http://navidrome:4533").rstrip("/")
NAVIDROME_USER = os.environ.get("NAVIDROME_USER", "admin")
NAVIDROME_PASS = os.environ.get("NAVIDROME_PASS", "")


def require_token(x_zay_token: str | None = Header(default=None)) -> None:
    if TOKEN and x_zay_token != TOKEN:
        raise HTTPException(status_code=401, detail="invalid token")


# ---------------------------------------------------------------------------
# worker (single loop, §9.6)
# ---------------------------------------------------------------------------

_cancel_flags: set[str] = set()
_worker_started = False
_worker_lock = threading.Lock()


def _set_cancel(job_id: str) -> None:
    _cancel_flags.add(job_id)


def _clear_cancel(job_id: str) -> None:
    _cancel_flags.discard(job_id)


def _is_cancelled(job_id: str) -> bool:
    return job_id in _cancel_flags


def _probe_duration(path: Path) -> float:
    """Audio length via mutagen. 0.0 when unreadable. Never raises."""
    try:
        from mutagen import File as _MutagenFile

        audio = _MutagenFile(str(path))
        if audio is not None and getattr(audio, "info", None) is not None:
            return float(audio.info.length or 0)
    except Exception as e:
        log.info("duration probe failed for %s: %s", path.name, e)
    return 0.0


def run_job(job: FetchJob) -> None:
    """Synchronous pipeline for one job: download → tag → loudness → move → scan."""
    job_id = job.id
    _clear_cancel(job_id)
    staging = Path(os.environ.get("STAGING_DIR", "/data/staging"))
    job_dir = staging / job_id
    job_dir.mkdir(parents=True, exist_ok=True)
    cover: Path | None = None

    def on_prog(p: float, stage: str) -> None:
        if _is_cancelled(job_id):
            raise DownloadError("cancelled")
        store.update_job(job_id, progress=p * 0.85, stage=stage, heartbeat=True)

    try:
        # Legacy queues can contain duplicate jobs. Recheck at execution time,
        # including files awaiting a Navidrome scan, before downloading again.
        existing = _existing_download_result(job.videoId, job.overrides, exclude_id=job.id)
        if existing:
            store.update_job(job_id, status="done", stage="existing", progress=1,
                             result=existing, heartbeat=True)
            return
        store.update_job(job_id, stage="download", progress=0.0, heartbeat=True)
        # Premium-first: the connected YouTube Music account is the
        # acquisition path. No probing, swapping, or rescuing — gated
        # label albums just download like any other track.
        if not downloader.cookies_file():
            raise DownloadError(downloader.COOKIES_REQUIRED_MESSAGE)
        meta = downloader.download(
            job.videoId,
            staging=staging,
            on_progress=on_prog,
            cancel_check=lambda: _is_cancelled(job_id),
        )
        if not meta.path:
            raise DownloadError("no file produced")
        meta = downloader.normalize_for_library(meta)

        title = meta.title or job.videoId
        artist = meta.artist or ""
        album = meta.album or ""
        track = job.overrides.track if job.overrides else None
        year = job.overrides.year if job.overrides else None

        # Recover the common "Artist - Title" upload convention when yt-dlp
        # cannot provide artist metadata, and remove duplicate artist prefixes.
        if " - " in title:
            left, _, right = title.partition(" - ")
            if not artist:
                artist, title = left.strip(), right.strip()
            elif left.strip().casefold() == artist.strip().casefold():
                title = right.strip()

        # Random-uploader uploads ("J.Cole Change Official Audio" by "Kal")
        # carry no usable tags. MusicBrainz can often recover the real
        # artist/title/album — but never let it override explicit overrides
        # (e.g. album-view adds already know the record).
        ov = job.overrides
        if not (ov and ov.artist and ov.album):
            try:
                from .musicbrainz_repair import repair_metadata

                fixed = repair_metadata(title=title, artist=artist, album=album)
                title = fixed.get("title", title)
                artist = fixed.get("artist", artist)
                album = fixed.get("album", album)
            except Exception as e:
                log.info("metadata repair skipped for job %s: %s", job_id, e)

        store.update_job(job_id, stage="tag", progress=0.88, heartbeat=True)
        tags = tagger.apply_tags(
            meta.path,
            title=title,
            artist=artist,
            album=album,
            track=track,
            year=year,
            overrides=job.overrides,
        )
        title = tags.get("title", title)
        artist = tags.get("artist", artist or "Unknown Artist")
        album = tags.get("album", album or "Unknown Album")

        if meta.thumbnail:
            cover = tagger.download_thumbnail(meta.thumbnail, job_dir)
            if cover:
                tagger.embed_cover(meta.path, cover)

        store.update_job(job_id, stage="loudness", progress=0.93, heartbeat=True)
        loudness.analyze_and_write(meta.path)

        if _is_cancelled(job_id):
            raise DownloadError("cancelled")

        store.update_job(job_id, stage="move", progress=0.96, heartbeat=True)
        dest = mover.move_to_library(
            meta.path,
            MUSIC_DIR,
            artist=artist,
            album=album,
            track=track,
            title=title,
            ext=meta.ext,
            cover_path=cover,
        )
        rel = str(dest.relative_to(MUSIC_DIR)) if dest.is_relative_to(MUSIC_DIR) else str(dest)

        store.update_job(job_id, stage="scan", progress=0.98, heartbeat=True)
        _start_navidrome_scan()

        result = {
            "path": rel,
            "title": title,
            "artist": artist,
            "album": album,
            "durationSec": meta.duration,
            "codec": meta.ext,
        }
        from .models import JobResult

        store.update_job(
            job_id,
            status="done",
            progress=1.0,
            stage="done",
            result=JobResult(**result),
            heartbeat=True,
        )
        log.info("job %s done → %s", job_id, rel)
    except DownloadError as e:
        if str(e) == "cancelled":
            store.update_job(job_id, status="cancelled", error="cancelled", stage="cancelled")
        else:
            store.update_job(job_id, status="failed", error=str(e), stage="failed")
        log.warning("job %s failed: %s", job_id, e)
    except Exception as e:
        store.update_job(job_id, status="failed", error=str(e)[:500], stage="failed")
        log.exception("job %s crashed", job_id)
    finally:
        _clear_cancel(job_id)
        # clean staging job dir (keep on failure for debug? plan says transient)
        try:
            if job_dir.exists() and shutil.rmtree:
                shutil.rmtree(job_dir, ignore_errors=True)
            download_dir = staging / job.videoId
            if download_dir.exists():
                shutil.rmtree(download_dir, ignore_errors=True)
        except Exception:
            pass


def _start_navidrome_scan() -> None:
    import hashlib
    import secrets

    import httpx

    if not NAVIDROME_URL:
        return
    s = secrets.token_hex(16)
    t = hashlib.md5((NAVIDROME_PASS + s).encode()).hexdigest()
    params = {
        "u": NAVIDROME_USER,
        "t": t,
        "s": s,
        "v": "1.16.1",
        "c": "ZayFetch",
        "f": "json",
    }
    try:
        httpx.post(f"{NAVIDROME_URL}/rest/startScan", params=params, timeout=15)
    except Exception as e:
        log.warning("startScan failed (scan schedule will pick it up): %s", e)


def _navidrome_song_path(song_id: str) -> str:
    """Look up a song's library path through the authenticated Subsonic API."""
    import hashlib
    import secrets

    salt = secrets.token_hex(16)
    params = {
        "u": NAVIDROME_USER,
        "t": hashlib.md5((NAVIDROME_PASS + salt).encode()).hexdigest(),
        "s": salt,
        "v": "1.16.1",
        "c": "ZayFetch",
        "f": "json",
        "id": song_id,
    }
    try:
        response = httpx.get(f"{NAVIDROME_URL}/rest/getSong", params=params, timeout=10)
        response.raise_for_status()
        body = response.json().get("subsonic-response", {})
    except (httpx.HTTPError, ValueError) as exc:
        log.warning("Navidrome getSong failed for %s: %s", song_id, exc)
        raise HTTPException(status_code=502, detail="Could not look up this song") from exc

    if body.get("status") != "ok":
        error = body.get("error", {})
        if error.get("code") == 70:
            raise HTTPException(status_code=404, detail="Song not found")
        log.warning("Navidrome getSong failed for %s: %s", song_id, error.get("message"))
        raise HTTPException(status_code=502, detail="Could not look up this song")
    song = body.get("song") or {}
    path = song.get("path")
    if not isinstance(path, str) or not path:
        raise HTTPException(status_code=404, detail="Song has no library file")
    return path


def _resolve_library_file(song_path: str) -> Path:
    """Resolve a Navidrome path and refuse anything outside MUSIC_DIR."""
    root = MUSIC_DIR.resolve()
    path = Path(song_path)
    candidate = path if path.is_absolute() else root / path
    try:
        resolved = candidate.resolve(strict=True)
    except FileNotFoundError as exc:
        # Navidrome can disagree with the on-disk filename about the track
        # number: it synthesizes its path from the file's TAGS (e.g. a peer
        # file named "00 - Change.flac" whose stale TRACKNUMBER=93 shows up
        # as "93 - Change.flac"). Strip track prefixes from BOTH sides, so
        # either direction of mismatch still resolves the same recording.
        # Only recover that exact variant in the same validated album dir.
        try:
            parent = candidate.parent.resolve(strict=True)
        except (FileNotFoundError, OSError) as parent_exc:
            raise HTTPException(status_code=404, detail="Song file not found") from parent_exc
        if not parent.is_relative_to(root):
            raise HTTPException(status_code=400, detail="Song path is outside the music library")
        if not parent.is_dir():
            raise HTTPException(status_code=404, detail="Song file not found")

        def _bare(stem: str) -> str:
            return re.sub(r"^\d+\s*-\s*", "", stem, count=1).casefold()

        requested_stem = _bare(candidate.stem)
        requested_suffix = candidate.suffix.casefold()
        matches = [
            sibling
            for sibling in parent.iterdir()
            if sibling.is_file()
            and sibling.suffix.casefold() == requested_suffix
            and _bare(sibling.stem) == requested_stem
        ]
        if not matches:
            raise HTTPException(status_code=404, detail="Song file not found") from exc
        if len(matches) > 1:
            raise HTTPException(status_code=409, detail="Song file path is ambiguous")
        try:
            resolved = matches[0].resolve(strict=True)
        except (FileNotFoundError, OSError) as match_exc:
            raise HTTPException(status_code=404, detail="Song file not found") from match_exc
    except OSError as exc:
        raise HTTPException(status_code=404, detail="Song file not found") from exc
    if resolved == root or not resolved.is_relative_to(root):
        raise HTTPException(status_code=400, detail="Song path is outside the music library")
    if not resolved.is_file():
        raise HTTPException(status_code=400, detail="Song path is not a file")
    return resolved


def _existing_download_result(video_id: str, overrides: Overrides | None,
                              *, exclude_id: str = "") -> JobResult | None:
    completed = store.find_reusable_job(video_id, overrides, exclude_id=exclude_id, completed_only=True)
    if completed:
        return completed.result
    if not overrides or not overrides.title:
        return None
    track = importer.ImportTrack(videoId=video_id, title=overrides.title,
                                 artist=overrides.artist or "")
    song_id = importer.find_library_song(track)
    if not song_id:
        return None
    try:
        song = importer._navidrome_get("getSong", id=song_id).get("song") or {}
        path = _resolve_library_file(str(song.get("path") or ""))
    except (importer.ImportError_, httpx.HTTPError, HTTPException):
        return None
    return JobResult(path=str(path.relative_to(MUSIC_DIR.resolve())), songId=song_id,
                     title=str(song.get("title") or overrides.title),
                     artist=str(song.get("artist") or overrides.artist or ""),
                     album=str(song.get("album") or overrides.album or ""),
                     durationSec=float(song.get("duration") or 0), codec=path.suffix.lstrip("."))


@app.delete("/music/{song_id}", response_model=OkResponse, dependencies=[Depends(require_token)])
def delete_music(song_id: str) -> OkResponse:
    """Permanently delete one library track and ask Navidrome to rescan."""
    if len(song_id) > 256:
        raise HTTPException(status_code=400, detail="invalid song id")
    path = _resolve_library_file(_navidrome_song_path(song_id))
    try:
        path.unlink()
    except OSError as exc:
        log.warning("Could not delete library file %s: %s", path, exc)
        raise HTTPException(status_code=500, detail="Could not delete song file") from exc
    _start_navidrome_scan()
    return OkResponse(ok=True)


@app.post("/music/{song_id}/tags", response_model=SongTagsResponse,
          dependencies=[Depends(require_token)])
def edit_music_tags(song_id: str, body: Any = Body(default=None)) -> SongTagsResponse:
    """Rewrite one library track's title/artist/album/track/year/genre.

    Only sent fields change (null or "" clears; the title cannot be
    cleared). The file is edited as a verified sibling copy and swapped in
    atomically, then Navidrome rescans. Navidrome may assign the song a new
    id after that rescan, because its track ids derive from these tags."""
    if len(song_id) > 256:
        raise HTTPException(status_code=400, detail="invalid song id")
    if not isinstance(body, dict):
        raise HTTPException(status_code=422, detail="Send the tag changes as a JSON object")
    try:
        request = SongTagsRequest.model_validate(body)
    except ValidationError as exc:
        raise HTTPException(status_code=422, detail=_tag_validation_detail(exc)) from exc
    path = _resolve_library_file(_navidrome_song_path(song_id))
    try:
        tags, changed = tagger.edit_song_tags(path, request.edits())
    except tagger.UnsupportedTagFormat as exc:
        raise HTTPException(status_code=415, detail=str(exc)) from exc
    except tagger.InvalidTagValue as exc:
        raise HTTPException(status_code=422, detail=str(exc)) from exc
    except (tagger.TagEditError, OSError) as exc:
        log.warning("Could not edit song tags (%s)", type(exc).__name__)
        raise HTTPException(status_code=500, detail="Could not update song tags") from exc
    if changed:
        _start_navidrome_scan()
    return SongTagsResponse(id=song_id, changed=changed, **tags)


def _tag_validation_detail(exc: ValidationError) -> str:
    """One readable sentence for native clients, which show `detail` verbatim."""
    error = exc.errors(include_url=False, include_input=False)[0]
    if error.get("type") == "extra_forbidden":
        field = ".".join(str(part) for part in error.get("loc", ()))
        return f"{field} cannot be edited"
    message = str(error.get("msg") or "Invalid tag value")
    return message.removeprefix("Value error, ")


async def worker_loop() -> None:
    """One-at-a-time queue consumer with heartbeats (§9.6)."""
    tick = 0
    while True:
        try:
            store.requeue_stale_heartbeats(60)
            job = store.next_queued()
            if job:
                # run sync work in a thread so the event loop stays responsive
                await asyncio.to_thread(run_job, job)
            else:
                await asyncio.sleep(0.75)
            tick += 1
            if tick % 8 == 0:
                store.requeue_stale_heartbeats(60)
        except asyncio.CancelledError:
            break
        except Exception:
            log.exception("worker loop error")
            await asyncio.sleep(1)


def ensure_worker() -> None:
    global _worker_started
    with _worker_lock:
        if _worker_started:
            return
        _worker_started = True
        try:
            loop = asyncio.get_running_loop()
            loop.create_task(worker_loop())
        except RuntimeError:
            # no running loop yet — started from lifespan
            pass


# ---------------------------------------------------------------------------
# YouTube Music import reconciler
# ---------------------------------------------------------------------------

_reconciler_started = False
_reconciler_lock = threading.Lock()


async def reconcile_loop() -> None:
    """Fold finished downloads back into their imported playlists (§import)."""
    while True:
        try:
            await asyncio.to_thread(importer.reconcile_imports)
            await asyncio.to_thread(library.reconcile_album_saves)
            await asyncio.to_thread(importer.reconcile_job_targets)
        except asyncio.CancelledError:
            break
        except Exception:
            log.exception("import reconcile loop error")
        await asyncio.sleep(8)


def ensure_reconciler() -> None:
    global _reconciler_started
    with _reconciler_lock:
        if _reconciler_started:
            return
        _reconciler_started = True
        try:
            loop = asyncio.get_running_loop()
            loop.create_task(reconcile_loop())
        except RuntimeError:
            # no running loop yet — started from lifespan
            pass


# ---------------------------------------------------------------------------
# managed browser session keeper (§5.3)
# ---------------------------------------------------------------------------

_session_keeper_started = False
_session_keeper_lock = threading.Lock()


def _session_keeper_interval_minutes() -> int:
    raw = os.environ.get("YOUTUBE_SESSION_TOUCH_MINUTES", "30").strip()
    try:
        return int(raw)
    except ValueError:
        return 30


def _relogin_min_seconds() -> float:
    """Minimum spacing between automatic sign-in attempts."""
    raw = os.environ.get("YOUTUBE_RELOGIN_MIN_MINUTES", "60").strip()
    try:
        return max(5.0, float(raw) * 60)
    except ValueError:
        return 3600.0


def _maybe_relogin() -> None:
    """When the browser session ended, sign back in automatically.

    Throttled by YOUTUBE_RELOGIN_MIN_MINUTES; records the attempt in
    youtube.status.json so the UI can show what happened. Never raises."""
    try:
        stored = downloader.read_cookies_status()
        if not session.relogin_configured():
            return
        last = float(stored.get("lastReloginAt") or 0)
        if time.time() - last < _relogin_min_seconds():
            return
        log.info("browser session ended — attempting automatic Google sign-in")
        result = session.attempt_relogin()
        stored = downloader.read_cookies_status()
        stored["lastReloginAt"] = time.time()
        stored["lastReloginState"] = result
        downloader.write_cookies_status(stored)
        if result == "reconnected":
            _session_keeper_tick()
            _reverify_youtube(force=True)
        elif result == "needs_human":
            log.warning("automatic sign-in needs the human — open the remote browser")
        else:
            log.info("automatic sign-in did not complete (%s)", result)
    except Exception:
        log.exception("automatic relogin check failed")


_reverify_lock = threading.Lock()
_last_reverify = 0.0
REVERIFY_MIN_INTERVAL = 30.0


def _reverify_youtube(*, force: bool = False) -> dict:
    """Re-run the sign-in probe and store the result.

    The stored ``verified`` flag is a cache: after the managed browser is
    signed back in, the cookies on disk are good again but the flag still
    says otherwise until something re-probes. Throttled so a burst of
    requests costs one probe."""
    global _last_reverify
    with _reverify_lock:
        stored = downloader.read_cookies_status()
        if not force and time.monotonic() - _last_reverify < REVERIFY_MIN_INTERVAL:
            return stored
        _last_reverify = time.monotonic()
        if not downloader.cookies_file():
            return stored
        verified, premium, message = _probe_youtube_auth()
        stored = downloader.read_cookies_status()
        stored.update({"verified": verified, "premium": premium, "message": message,
                       "checkedAt": time.time()})
        downloader.write_cookies_status(stored)
        log.info("youtube status re-verified: verified=%s premium=%s", verified, premium)
        return stored


def _session_keeper_tick() -> str:
    """Export once, refresh the stored cookies + status. Never raises.

    Returns the browser state: connected / signed_out / disconnected."""
    text, state = session.sync_session()
    if text and state == "connected":
        downloader.write_netscape(text)
    stored = downloader.read_cookies_status()
    stored["browser"] = state
    if text and state == "connected":
        stored["lastSyncAt"] = time.time()
    downloader.write_cookies_status(stored)
    if text and state == "connected" and not stored.get("verified"):
        # Signed back in through the browser: the fresh cookies deserve a
        # fresh probe instead of the stale "rejected" verdict.
        _reverify_youtube()
    return state


async def session_keeper_loop() -> None:
    """Keep the browser active and sync Google's latest rotated cookies.

    A no-op (and disabled) when YOUTUBE_SESSION_TOUCH_MINUTES is 0; backs off
    to 120 minutes after consecutive failures. Never raises out of the loop."""
    interval = _session_keeper_interval_minutes()
    if interval <= 0:
        log.info("managed session keeper disabled (touch interval 0)")
        return
    failures = 0
    while True:
        try:
            await asyncio.to_thread(session.touch_session)
            state = await asyncio.to_thread(_session_keeper_tick)
            if state == "signed_out":
                await asyncio.to_thread(_maybe_relogin)
                state = await asyncio.to_thread(
                    lambda: str(downloader.read_cookies_status().get("browser") or state)
                )
            if state == "disconnected":
                failures += 1
            else:
                failures = 0
                log.info("managed session synced (%s)", state)
        except asyncio.CancelledError:
            break
        except Exception:
            failures += 1
            log.exception("session keeper tick failed")
        await asyncio.sleep(120 * 60 if failures >= 2 else interval * 60)


def ensure_session_keeper() -> None:
    """Idempotent; exactly one loop. Skips entirely when browser mode is off."""
    global _session_keeper_started
    if not session.browser_enabled():
        return
    with _session_keeper_lock:
        if _session_keeper_started:
            return
        _session_keeper_started = True
        try:
            loop = asyncio.get_running_loop()
            loop.create_task(session_keeper_loop())
        except RuntimeError:
            # no running loop yet — started from lifespan
            pass


@app.on_event("startup")
async def _startup() -> None:
    Path(os.environ.get("STAGING_DIR", "/data/staging")).mkdir(parents=True, exist_ok=True)
    MUSIC_DIR.mkdir(parents=True, exist_ok=True)
    store.get_conn()
    # Single worker only: ensure_worker is idempotent via _worker_started,
    # so exactly one queue consumer exists. (A previous version reset the
    # flag and called it twice — two concurrent workers could claim two jobs.)
    ensure_worker()
    ensure_reconciler()
    _reprobe_youtube_status()
    ensure_session_keeper()


def _reprobe_youtube_status() -> None:
    """Refresh the cached connection status in the background on boot.

    A cached 'could not be verified' can be stale after an upgrade (e.g.
    the PO Token provider arriving); re-probing here means Settings shows
    the truth without the user reconnecting. Never blocks startup."""
    if not downloader.cookies_file():
        return

    def run() -> None:
        try:
            verified, premium, message = _probe_youtube_auth()
            downloader.write_cookies_status({
                "verified": verified,
                "premium": premium,
                "message": message,
                "checkedAt": time.time(),
            })
            log.info("youtube status refreshed: verified=%s premium=%s", verified, premium)
        except Exception as e:
            log.info("youtube status refresh failed: %s", e)

    threading.Thread(target=run, daemon=True).start()


# ---------------------------------------------------------------------------
# routes
# ---------------------------------------------------------------------------


@app.get("/healthz", response_model=HealthResponse)
def healthz() -> HealthResponse:
    return HealthResponse(ok=True, yt_dlp=downloader.ytdlp_version())


# ---------------------------------------------------------------------------
# YouTube Music connection (Premium-first acquisition)
# ---------------------------------------------------------------------------


def _youtube_status() -> dict:
    """Current connection state without a live probe."""
    path = downloader.cookies_path()
    configured = bool(downloader.cookies_file())
    stored = downloader.read_cookies_status()
    updated_at = None
    if configured:
        try:
            updated_at = path.stat().st_mtime
        except OSError:
            updated_at = None
    expiry = downloader.session_expiry_info()
    browser_state = stored.get("browser") if session.browser_enabled() else None
    return {
        "configured": configured,
        "verified": bool(stored.get("verified")) if configured else False,
        "premium": bool(stored.get("premium")) if configured else False,
        "message": str(stored.get("message") or "") if configured else "",
        "checkedAt": stored.get("checkedAt"),
        "updatedAt": updated_at,
        "browser": browser_state,
        "lastSyncAt": stored.get("lastSyncAt"),
        "selfRelogin": session.relogin_configured(),
        "lastReloginAt": stored.get("lastReloginAt"),
        "lastReloginState": stored.get("lastReloginState"),
        "expiresAt": expiry.get("expiresAt"),
        "expiresInDays": expiry.get("expiresInDays"),
        "oauth": {
            "configured": downloader.oauth_configured(),
            "connected": downloader.oauth_path().is_file(),
        },
    }


@app.get("/youtube/cookies", dependencies=[Depends(require_token)])
def youtube_cookies_status(verify: bool = Query(default=False)) -> dict:
    if verify and downloader.cookies_file():
        # "Check connection": re-run the real sign-in probe, not just the browser check.
        _reverify_youtube(force=True)
    status = _youtube_status()
    if session.browser_enabled():
        # Sign-in can change between keeper ticks. Check the live browser so
        # returning from noVNC does not leave the user with a stale warning.
        logged_in = session.browser_logged_in()
        status["browser"] = (
            "disconnected" if logged_in is None
            else "connected" if logged_in else "signed_out"
        )
    return status


@app.post("/youtube/cookies", dependencies=[Depends(require_token)])
async def youtube_cookies_save(request: Request) -> dict:
    """Store a cookies.txt and verify the account from this server.

    The probe runs yt-dlp against a known Premium-gated track, so the
    answer reflects the actual download path — not just cookie syntax."""
    body = await request.json()
    text = body.get("cookies") if isinstance(body, dict) else None
    if not isinstance(text, str) or len(text.strip()) < 16:
        raise HTTPException(status_code=400, detail="Paste the contents of a cookies.txt file")
    try:
        downloader.write_cookies(text)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e)) from e
    verified, premium, message = await run_in_threadpool(_probe_youtube_auth)
    downloader.write_cookies_status({
        "verified": verified,
        "premium": premium,
        "message": message,
        "checkedAt": time.time(),
    })
    return _youtube_status()


@app.delete("/youtube/cookies", dependencies=[Depends(require_token)])
def youtube_cookies_delete() -> dict:
    removed = downloader.remove_cookies()
    return {"ok": True, "removed": removed}


# --- Google API OAuth (device flow; API-only, §5.4) ------------------------

OAUTH_NOT_CONFIGURED_MESSAGE = (
    "Google API sign-in is not configured. Set YTM_OAUTH_CLIENT_ID and "
    "YTM_OAUTH_CLIENT_SECRET (OAuth client type “TVs and Limited Input devices”)."
)


def _oauth_credentials():
    from ytmusicapi.auth.oauth import OAuthCredentials

    return OAuthCredentials(
        downloader.oauth_client_id(), downloader.oauth_client_secret()
    )


@app.post("/youtube/oauth/start", dependencies=[Depends(require_token)])
async def youtube_oauth_start() -> dict:
    """Begin a Google device-flow sign-in; returns the code the user enters."""
    if not downloader.oauth_configured():
        raise HTTPException(status_code=400, detail=OAUTH_NOT_CONFIGURED_MESSAGE)
    try:
        code = await run_in_threadpool(_oauth_credentials().get_code)
    except Exception as e:  # noqa: BLE001
        log.warning("oauth start failed: %s", e)
        raise HTTPException(
            status_code=502,
            detail="Could not start Google sign-in. Check the OAuth client settings.",
        ) from e
    device_code = str(code.get("device_code") or "")
    if not device_code:
        raise HTTPException(status_code=502, detail="Google did not return a device code.")
    downloader.write_device_code(device_code)
    return {
        "user_code": str(code.get("user_code") or ""),
        "verification_url": str(code.get("verification_url") or ""),
        "expires_in": int(code.get("expires_in") or 0),
    }


@app.post("/youtube/oauth/poll", dependencies=[Depends(require_token)])
async def youtube_oauth_poll() -> dict:
    """Finish a device-flow sign-in. Pending maps to ``{connected: false, pending: true}``."""
    if not downloader.oauth_configured():
        raise HTTPException(status_code=400, detail=OAUTH_NOT_CONFIGURED_MESSAGE)
    device_code = downloader.read_device_code()
    if not device_code:
        raise HTTPException(status_code=400, detail="No Google sign-in in progress. Start again.")
    try:
        token = await run_in_threadpool(_oauth_credentials().token_from_code, device_code)
    except Exception as e:  # noqa: BLE001
        log.warning("oauth poll failed: %s", e)
        raise HTTPException(
            status_code=400, detail="Google rejected the sign-in. Start again."
        ) from e
    if not isinstance(token, dict):
        raise HTTPException(status_code=502, detail="Google returned an unexpected response.")
    error = str(token.get("error") or "")
    if error in ("authorization_pending", "slow_down"):
        return {"connected": False, "pending": True}
    if error:
        downloader.remove_device_code()
        raise HTTPException(
            status_code=400, detail="Google sign-in expired or was denied. Start again."
        )
    if not token.get("access_token") or not token.get("refresh_token"):
        raise HTTPException(status_code=502, detail="Google returned an incomplete token.")
    access_expires = int(token.get("expires_in") or 0)
    downloader.write_oauth_token({
        "scope": token.get("scope"),
        "token_type": token.get("token_type") or "Bearer",
        "access_token": token["access_token"],
        "refresh_token": token["refresh_token"],
        "expires_in": int(token.get("refresh_token_expires_in") or access_expires),
        "expires_at": int(time.time()) + access_expires,
    })
    downloader.remove_device_code()
    return {"connected": True}


@app.delete("/youtube/oauth", dependencies=[Depends(require_token)])
def youtube_oauth_delete() -> dict:
    removed = downloader.remove_oauth()
    return {"ok": True, "removed": removed}


# ---------------------------------------------------------------------------
# One-way YouTube Music playlist import
# ---------------------------------------------------------------------------


def _import_http_error(exc: Exception) -> HTTPException:
    if isinstance(exc, importer.ImportError_):
        status = 412 if not downloader.cookies_file() else 502
        return HTTPException(status_code=status, detail=str(exc))
    log.warning("import failed: %s", exc)
    return HTTPException(status_code=502, detail="Import failed. Try again in a moment.")


def _require_youtube() -> None:
    """Import needs a live, verified connection — not just a stored cookie file."""
    if not downloader.cookies_file():
        raise HTTPException(status_code=412, detail=downloader.COOKIES_REQUIRED_MESSAGE)
    if not _youtube_status().get("verified") and not _reverify_youtube().get("verified"):
        raise HTTPException(
            status_code=412,
            detail="YouTube Music sign-in needs attention. Reconnect your account in "
                   "Settings → YouTube Music.",
        )


@app.get("/youtube/library/playlists", response_model=YouTubePlaylistsResponse,
         dependencies=[Depends(require_token)])
def youtube_library_playlists() -> YouTubePlaylistsResponse:
    """The connected account's YT Music library playlists."""
    _require_youtube()
    try:
        return YouTubePlaylistsResponse(playlists=importer.list_library_playlists())
    except Exception as exc:  # noqa: BLE001
        raise _import_http_error(exc) from exc


@app.get("/youtube/library/playlists/{playlist_id}", response_model=YouTubePlaylistTracksResponse,
         dependencies=[Depends(require_token)])
def youtube_library_playlist_tracks(playlist_id: str) -> YouTubePlaylistTracksResponse:
    """Ordered tracks for one YT Music playlist (preview before importing)."""
    if not re.fullmatch(r"[A-Za-z0-9_-]{1,128}", playlist_id):
        raise HTTPException(status_code=400, detail="invalid playlist id")
    _require_youtube()
    try:
        title, tracks = importer.get_playlist_tracks(playlist_id)
    except Exception as exc:  # noqa: BLE001
        raise _import_http_error(exc) from exc
    return YouTubePlaylistTracksResponse(id=playlist_id, title=title, tracks=tracks)


@app.post("/youtube/imports", dependencies=[Depends(require_token)])
async def create_youtube_import(request: Request) -> dict:
    """Mirror one YT Music playlist locally: link what exists, download the rest."""
    body = await request.json()
    playlist_id = body.get("playlistId") if isinstance(body, dict) else None
    if not isinstance(playlist_id, str) or not re.fullmatch(r"[A-Za-z0-9_-]{1,128}", playlist_id):
        raise HTTPException(status_code=400, detail="playlistId required")
    await run_in_threadpool(_require_youtube)
    try:
        result = await run_in_threadpool(importer.create_import, playlist_id)
    except Exception as exc:  # noqa: BLE001
        raise _import_http_error(exc) from exc
    ensure_worker()
    ensure_reconciler()
    return {"import": result.model_dump()}


@app.get("/youtube/imports", response_model=PlaylistImportsResponse,
         dependencies=[Depends(require_token)])
def list_youtube_imports() -> PlaylistImportsResponse:
    return PlaylistImportsResponse(imports=importer.list_imports())


@app.post("/youtube/imports/{import_id}/sync", dependencies=[Depends(require_token)])
def sync_youtube_import(import_id: str) -> dict:
    try:
        result = importer.sync_import(import_id)
    except Exception as exc:  # noqa: BLE001
        raise _import_http_error(exc) from exc
    ensure_worker()
    return {"import": result.model_dump()}


@app.post("/youtube/imports/{import_id}/pull", dependencies=[Depends(require_token)])
async def pull_youtube_import(import_id: str) -> dict:
    """Re-read the YTM playlist and add whatever the local playlist lacks."""
    await run_in_threadpool(_require_youtube)
    try:
        result = await run_in_threadpool(importer.pull_import, import_id)
    except Exception as exc:  # noqa: BLE001
        raise _import_http_error(exc) from exc
    ensure_worker()
    ensure_reconciler()
    return {"import": result.model_dump()}


@app.delete("/youtube/imports/{import_id}", response_model=OkResponse,
            dependencies=[Depends(require_token)])
def delete_youtube_import(import_id: str) -> OkResponse:
    try:
        importer.remove_import(import_id)
    except Exception as exc:  # noqa: BLE001
        raise _import_http_error(exc) from exc
    return OkResponse(ok=True)


@app.get("/metadata/lookup", response_model=MetadataLookupResponse,
         dependencies=[Depends(require_token)])
def metadata_lookup(
    q: str = Query(default=""),
    artist: str = Query(default=""),
    title: str = Query(default=""),
    limit: int = Query(8, ge=1, le=25),
) -> MetadataLookupResponse:
    """Canonical metadata candidates for manually fixing one song's tags.

    Deezer is not part of search any more; it exists for exactly this —
    open a song, ask for the real title/artist/album/year/artwork, apply
    one. Total: a catalog outage returns []."""
    query = q.strip() or " ".join(part for part in (artist.strip(), title.strip()) if part).strip()
    if not query:
        return MetadataLookupResponse()
    try:
        result = catalog.lookup(query, limit=limit * 2)
    except Exception as e:
        log.info("deezer metadata lookup failed for %r: %s", query, e)
        result = None
    if result is None:
        return MetadataLookupResponse()
    # Dedupe on folded title+artist.
    seen: set[tuple[str, str]] = set()
    tracks: list[catalog.CatalogTrack] = []
    for track in result.tracks:
        key = (
            re.sub(r"[^a-z0-9]", "", track.title.casefold()),
            re.sub(r"[^a-z0-9]", "", track.artist.casefold()),
        )
        if not key[0] or not key[1] or key in seen:
            continue
        seen.add(key)
        tracks.append(track)
    tracks = tracks[:limit]
    years: dict[int, int] = {}
    if result.album is not None and result.album.year:
        years[result.album.id] = result.album.year
    try:
        years |= catalog.album_years(
            [t.album_id for t in tracks if t.album_id and t.album_id not in years]
        )
    except Exception as e:  # noqa: BLE001 — years are optional enrichment
        log.info("deezer album years failed for %r: %s", query, e)
    return MetadataLookupResponse(candidates=[
        MetadataCandidate(
            source="deezer", title=track.title, artist=track.artist,
            album=track.album, year=years.get(track.album_id or 0),
            artwork=track.cover, durationSec=track.duration,
            explicit=track.explicit, isrc=track.isrc,
        )
        for track in tracks
    ])


@app.get("/search", response_model=SearchResponse, dependencies=[Depends(require_token)])
def search(q: str = Query(..., min_length=1), limit: int = Query(10, ge=1, le=50),
           source: str = Query("all", pattern="^(all|music|youtube)$")) -> SearchResponse:
    """YouTube Music + YouTube search. No third-party catalog: ranking is
    pure YouTube signal. Never filters; only ranks (§8.3)."""
    count = max(limit, 20)
    with ThreadPoolExecutor(max_workers=2) as pool:
        providers_f = pool.submit(_search_providers, q, count, source)
        suggest_f = pool.submit(discovery.ytm_suggestions, q, 8)
        cands, albums, top_artist = providers_f.result()
        suggestions = suggest_f.result()
    album_out: list[AlbumResult] = []
    pinned_title = ""
    for a in albums[:6]:
        ar = AlbumResult(**a)
        ar.score = album_score(ar.title, ar.artist, q)
        if not pinned_title and is_album_intent(q, ar.title, ar.artist):
            ar.pinned = True
            pinned_title = ar.title
        album_out.append(ar)
    ranked = rank(cands, query=q, top_artist=top_artist, pinned_album=pinned_title)
    # de-dupe by videoId preserving best score
    seen: dict[str, Candidate] = {}
    for c in ranked:
        if c.videoId not in seen:
            seen[c.videoId] = c
    out = list(seen.values())[:limit]
    # Spelling correction is a separate cheap lookup: only when the query
    # parses as artist+title and Last.fm knows a correction. Never blocks.
    correction: Correction | None = None
    try:
        aq, tq = discovery.split_artist_title(q)
        if aq and tq:
            fixed = discovery.lastfm_correction(aq, tq)
            if fixed:
                correction = Correction(**fixed)
    except Exception as e:
        log.info("correction lookup failed for %r: %s", q, e)
    return SearchResponse(candidates=out, albums=album_out,
                          correction=correction, suggestions=suggestions)


@app.get("/suggest", dependencies=[Depends(require_token)])
def suggest(q: str = Query(..., min_length=1)) -> SuggestResponse:
    """As-you-type resolve: YTM suggestions + Last.fm correction.
    Fast, total — never fails the omnibar."""
    with ThreadPoolExecutor(max_workers=2) as pool:
        sug_f = pool.submit(discovery.ytm_suggestions, q, 8)
        aq, tq = discovery.split_artist_title(q)
        corr_f = pool.submit(discovery.lastfm_correction, aq, tq) if aq and tq else None
        suggestions = sug_f.result()
        correction = None
        if corr_f is not None:
            try:
                fixed = corr_f.result()
                correction = Correction(**fixed) if fixed else None
            except Exception:
                correction = None
    return SuggestResponse(suggestions=suggestions, correction=correction)


@app.get("/radio", response_model=RadioResponse, dependencies=[Depends(require_token)])
def radio(videoId: str = Query(default=""), songId: str = "", artist: str = Query(default=""),
          title: str = Query(default=""), durationSec: float = 0,
          limit: int = Query(20, ge=1, le=50), excludeLibrary: bool = False) -> RadioResponse:
    """Autoplay graph: YTM watch-playlist radio first (their similarity
    graph), Last.fm crowd-similar resolved to playable uploads second,
    artist top-tracks third. Never empty when anything is known.

    With excludeLibrary, candidates already owned as library songs are
    skipped so only new music is returned."""
    want = max(min(limit, 50), 1)
    owned = library.known_recordings() if excludeLibrary else set()
    out: list[Candidate] = []
    sources: list[str] = []
    seen: set[str] = {videoId} if videoId else set()
    recordings: set[tuple[str, str]] = set()
    if artist and title:
        recordings.add((artist.casefold().strip(), title.casefold().strip()))

    def add(c: Candidate) -> None:
        identity = (c.channel.casefold().strip(), c.title.casefold().strip())
        if c.videoId and c.videoId not in seen and identity not in recordings and identity not in owned:
            seen.add(c.videoId)
            recordings.add(identity)
            out.append(c)

    # Saved library tracks have Navidrome IDs rather than YouTube IDs.
    # Resolve their identity before asking for radio; otherwise autoplay
    # silently returns nothing on a free, keyless installation.
    if not re.fullmatch(r"[A-Za-z0-9_-]{11}", videoId or ""):
        # Seeds need the *exact* source video: radiating from a remix or
        # mirror search guess drifts into unrelated genres. Prefer the
        # recorded import mapping, then the video embedded in a downloaded
        # file's comment tag, and only search as a last resort.
        imported = ""
        if songId.strip():
            try:
                imported = store.video_for_song(songId.strip()) or ""
            except Exception as exc:  # noqa: BLE001
                log.info("radio seed library lookup failed for %r: %s", songId, exc)
            if not imported:
                try:
                    imported = importer.video_id_for_library_song(songId.strip())
                except Exception as exc:  # noqa: BLE001
                    log.info("radio seed comment lookup failed for %r: %s", songId, exc)
            if imported and not re.fullmatch(r"[A-Za-z0-9_-]{11}", imported):
                imported = ""
        if imported:
            videoId = imported
            seen.add(videoId)
        elif title.strip():
            query = f"{artist} {title}".strip()
            try:
                seeds = _search_ytmusic(query, 5)
                if not seeds:
                    seeds = _search_yt_dlp(query, 5)
                if seeds:
                    videoId = _pick_seed_video(seeds, artist, title, durationSec)
                    seen.add(videoId)
            except Exception as exc:
                log.info("radio seed lookup failed for %r: %s", query, exc)

    # Tier 1: YTM radio — the closest thing to "people who listen also listen".
    if re.fullmatch(r"[A-Za-z0-9_-]{11}", videoId or ""):
        for rt in discovery.ytm_radio(videoId, min(50, want * 2)):
            add(Candidate(
                videoId=rt.videoId, source="ytmusic", title=rt.title,
                channel=rt.artist or artist, durationSec=rt.durationSec,
                thumbnails=[Thumbnail(**th) for th in rt.thumbnails],
                badges=["has-metadata"],
            ))
        if out:
            sources.append("ytm-radio")

    # Tier 2: Last.fm crowd-similar, resolved to playable uploads.
    similar: list = []
    if len(out) < want and artist.strip() and title.strip():
        similar = discovery.lastfm_similar(artist, title, min(10, want))
    if similar:
        resolved = _resolve_similar(similar, want - len(out))
        for c in resolved:
            add(c)
        if resolved:
            sources.append("lastfm-similar")

    # Tier 3: artist top-tracks fallback (obscure songs with no graph).
    if len(out) < want and artist.strip():
        top = discovery.lastfm_artist_top(artist, min(8, want - len(out)))
        # don't re-add the seed song itself
        top = [t for t in top
               if not (t.title.casefold() == title.casefold()
                       and t.artist.casefold() == artist.casefold())]
        if top:
            resolved = _resolve_similar(top, want - len(out))
            for c in resolved:
                add(c)
            if resolved:
                sources.append("artist-top")

    return RadioResponse(candidates=out[:want], sources=sources)


def _pick_seed_video(candidates: list[Candidate], artist: str, title: str, duration: float) -> str:
    """Most faithful search hit for a seed song: same recording, right runtime.

    Radio seeded from the wrong upload (live, remix, cover, sped-up) derails
    the whole suggestions graph, so prefer exact artist/title identity and a
    duration within ±25s before trusting generic relevance ranking."""
    pool = [c for c in candidates if c.videoId]
    if not pool:
        return ""
    want = (artist.casefold().strip(), title.casefold().strip())
    if duration:
        close = [c for c in pool if c.durationSec and abs(c.durationSec - duration) <= 25]
        if close:
            pool = close
    if want[1]:
        same = [c for c in pool if (c.channel.casefold().strip(), c.title.casefold().strip()) == want]
        if same:
            pool = same
    ranked = rank(pool, query=f"{artist} {title}".strip())
    return ranked[0].videoId if ranked else ""


def _resolve_similar(similar: list, budget: int) -> list[Candidate]:
    """Resolve crowd-similar (artist, title) pairs to playable YT uploads.
    One bounded ytsearch per pair, in parallel. Total — misses just skip."""
    items = similar[:max(budget, 0)]
    if not items:
        return []

    def one(pair: Any) -> Candidate | None:
        try:
            q = f"{pair.artist} - {pair.title}"
            hits = _search_yt_dlp(q, 3)
            if not hits:
                hits = _search_ytmusic(q, 3)
            return hits[0] if hits else None
        except Exception as e:
            log.info("similar resolve failed for %s - %s: %s",
                     getattr(pair, "artist", ""), getattr(pair, "title", ""), e)
            return None

    with ThreadPoolExecutor(max_workers=min(5, len(items))) as pool:
        results = list(pool.map(one, items))
    return [c for c in results if c is not None]


def _search_providers(
    q: str, count: int, source: str
) -> tuple[list[Candidate], list[dict], str]:
    """The pre-catalog provider fan-out (YTM + YouTube)."""
    if source == "music":
        return _music_search(q, count)
    if source == "youtube":
        # Same artist signal as the music path — the frontend merges both
        # responses by score, so they must be ranked with identical inputs.
        with ThreadPoolExecutor(max_workers=2) as pool:
            cands_f = pool.submit(_search_yt_dlp, q, count)
            meta_f = pool.submit(_find_albums, q)
            cands = cands_f.result()
            _, top_artist = meta_f.result()
        return cands, [], top_artist
    return _search_both(q, count)


def _extract_preview_url(video_id: str) -> str:
    try:
        return previews.audio_url(video_id)
    except previews.Unavailable as e:
        raise HTTPException(status_code=502, detail="Audio is unavailable for this track") from e


def _resolve_playable_url(
    video_id: str,
    artist: str = "",
    title: str = "",
    expected_dur: float | None = None,
) -> str:
    """Playable audio URL for one video.

    Premium-first: authenticated yt-dlp plays gated uploads directly.
    Without a connected account, public uploads still work and a failure
    gets an actionable error instead of a source swap."""
    try:
        return _extract_preview_url(video_id)
    except HTTPException:
        if not downloader.cookies_file():
            raise HTTPException(status_code=412, detail=downloader.COOKIES_REQUIRED_MESSAGE)
        raise


@app.get("/preview/{video_id}", dependencies=[Depends(require_token)])
async def preview(
    video_id: str,
    request: Request,
    artist: str = Query(default=""),
    title: str = Query(default=""),
    duration: float | None = Query(default=None),
) -> StreamingResponse:
    """Stream new-music audio, forwarding byte ranges for seeking."""
    if not re.fullmatch(r"[A-Za-z0-9_-]{11}", video_id):
        raise HTTPException(status_code=400, detail="invalid video id")
    try:
        url = await run_in_threadpool(_resolve_playable_url, video_id, artist, title, duration)
        client = httpx.AsyncClient(timeout=30, follow_redirects=True,
                                   headers={"User-Agent": _BROWSER_UA})
        headers = {"Range": request.headers["range"]} if "range" in request.headers else {}
        try:
            upstream = await client.send(client.build_request("GET", url, headers=headers), stream=True)
        except Exception:
            await client.aclose()
            raise
        if upstream.status_code not in (200, 206):
            await upstream.aclose()
            await client.aclose()
            raise HTTPException(status_code=502, detail="Audio is unavailable for this track")

        async def chunks():
            try:
                async for chunk in upstream.aiter_bytes():
                    yield chunk
            finally:
                await upstream.aclose()
                await client.aclose()

        passthrough = {k: v for k, v in upstream.headers.items()
                       if k.lower() in ("content-type", "content-length", "content-range", "accept-ranges")}
        passthrough["Cache-Control"] = "private, no-store"
        return StreamingResponse(chunks(), status_code=upstream.status_code, headers=passthrough)
    except HTTPException:
        raise
    except (OSError, subprocess.TimeoutExpired, httpx.HTTPError) as exc:
        log.warning("preview failed for %s: %s", video_id, exc)
        raise HTTPException(status_code=502, detail="Audio is unavailable for this track") from exc


@app.post("/preview/warm", dependencies=[Depends(require_token)])
def preview_warm(body: PreviewWarmRequest) -> dict[str, int]:
    """Start extracting upcoming tracks so pressing next is instant."""
    ids = [v for v in body.videoIds if re.fullmatch(r"[A-Za-z0-9_-]{11}", v)]
    return {"started": previews.warm(ids)}


@app.get("/home", dependencies=[Depends(require_token)])
def home_sections(refresh: bool = Query(default=False)) -> dict[str, Any]:
    """Top songs and Rediscover, from Navidrome play counts (cached 10 min)."""
    try:
        return library.home(refresh=refresh)
    except (importer.ImportError_, httpx.HTTPError) as exc:
        raise HTTPException(status_code=502, detail="Could not read your library") from exc


@app.post("/album-saves", dependencies=[Depends(require_token)])
def album_save(body: AlbumSaveRequest) -> dict[str, Any]:
    """Save a YT Music album: star its Navidrome album once tracks are indexed."""
    row = library.save_album(body.title, body.artist)
    ensure_reconciler()
    return {"albumId": row.get("album_id")}


@app.get("/library/albums/{album_id}/order", dependencies=[Depends(require_token)])
def library_album_order(album_id: str) -> dict[str, Any]:
    try:
        return {"tracks": album_order.recover(album_id)}
    except (importer.ImportError_, httpx.HTTPError) as exc:
        raise HTTPException(status_code=502, detail="Could not read your album") from exc


@app.get("/resolve", response_model=ResolveResponse, dependencies=[Depends(require_token)])
def resolve(url: str = Query(...)) -> ResolveResponse:
    """Resolve a YouTube/YT Music URL to video or playlist candidates."""
    kind, cands, title = _resolve_url(url)
    return ResolveResponse(kind=kind, candidates=cands, title=title)


@app.get("/album", response_model=AlbumResponse, dependencies=[Depends(require_token)])
def get_album(browseId: str = Query(..., min_length=4, max_length=64)) -> AlbumResponse:
    """Open a YTM album (from /search albums) as an ordered tracklist."""
    if not re.fullmatch(r"[A-Za-z0-9_-]+", browseId):
        raise HTTPException(status_code=400, detail="invalid album id")
    try:
        info = discovery.ytmusic_call("get_album", browseId)
    except Exception as e:
        log.warning("get_album failed for %s: %s", browseId, e)
        raise HTTPException(status_code=502, detail="Could not load this album") from e
    artists = info.get("artists") or []
    artist = artists[0].get("name", "") if artists else ""
    explicit = info.get("isExplicit")
    album = AlbumResult(
        browseId=browseId,
        title=info.get("title") or "",
        artist=artist,
        year=info.get("year"),
        explicit=explicit if isinstance(explicit, bool) else None,
        thumbnails=[
            Thumbnail(url=t.get("url", ""), width=t.get("width", 0), height=t.get("height", 0))
            for t in (info.get("thumbnails") or [])
            if t.get("url")
        ],
    )
    # Playlist order, NOT score order — opening an album shows the record.
    cands = _expand_album({"browseId": browseId, "title": album.title, "artist": artist})
    if not cands:
        raise HTTPException(status_code=502, detail="No playable tracks found for this album")
    return AlbumResponse(album=album, candidates=cands)


@app.post("/jobs", response_model=JobResponse, dependencies=[Depends(require_token)])
async def create_job(request: Request) -> JobResponse:
    body = await request.json()
    overrides = None
    if body.get("overrides"):
        overrides = Overrides.model_validate(body["overrides"])
    video_id = body.get("videoId") or ""
    url = body.get("url")
    if not video_id and url:
        # extract video id from url
        m = re.search(r"(?:v=|youtu\.be/|shorts/)([\w-]{6,})", url)
        if m:
            video_id = m.group(1)
    if not video_id:
        raise HTTPException(status_code=400, detail="videoId or url required")
    if not re.fullmatch(r"[A-Za-z0-9_-]{6,64}", video_id):
        raise HTTPException(status_code=400, detail="invalid YouTube video id")
    targets = [
        target
        for target in (body.get("playlistIds") or [])
        if isinstance(target, str) and re.fullmatch(r"[A-Za-z0-9_-]{1,64}", target)
    ]
    reusable = store.find_reusable_job(video_id, overrides)
    if reusable:
        if targets:
            store.add_job_targets(reusable.id, targets)
        return JobResponse(job=reusable)
    existing = await run_in_threadpool(_existing_download_result, video_id, overrides)
    # Premium-first: YouTube jobs exist only for a connected account.
    if not existing and not downloader.cookies_file():
        raise HTTPException(status_code=412, detail=downloader.COOKIES_REQUIRED_MESSAGE)
    job = store.create_job(videoId=video_id, url=url, overrides=overrides, existing_result=existing)
    if targets:
        store.add_job_targets(job.id, targets)
    ensure_worker()
    return JobResponse(job=job)


@app.get("/jobs", response_model=JobsResponse, dependencies=[Depends(require_token)])
def list_jobs(limit: int = Query(50, ge=1, le=200)) -> JobsResponse:
    return JobsResponse(jobs=store.list_jobs(limit))


@app.get("/jobs/{job_id}", response_model=JobResponse, dependencies=[Depends(require_token)])
def get_job(job_id: str) -> JobResponse:
    job = store.get_job(job_id)
    if not job:
        raise HTTPException(status_code=404, detail="job not found")
    return JobResponse(job=job)


@app.post("/jobs/{job_id}/retry", response_model=JobResponse, dependencies=[Depends(require_token)])
def retry_job(job_id: str) -> JobResponse:
    previous = store.get_job(job_id)
    if previous and previous.status in ("failed", "cancelled"):
        existing = _existing_download_result(previous.videoId, previous.overrides, exclude_id=previous.id)
        if existing:
            job = store.update_job(job_id, status="done", stage="existing", progress=1, result=existing)
            return JobResponse(job=job)
    job = store.retry_job(job_id)
    if not job:
        raise HTTPException(status_code=404, detail="job not found")
    ensure_worker()
    return JobResponse(job=job)


@app.post("/jobs/{job_id}/cancel", response_model=JobResponse, dependencies=[Depends(require_token)])
def cancel_job(job_id: str) -> JobResponse:
    _set_cancel(job_id)
    job = store.cancel_job(job_id)
    if not job:
        raise HTTPException(status_code=404, detail="job not found")
    return JobResponse(job=job)


@app.delete("/jobs/{job_id}", response_model=OkResponse, dependencies=[Depends(require_token)])
def delete_job(job_id: str) -> OkResponse:
    if not store.delete_job(job_id):
        raise HTTPException(status_code=404, detail="job not found")
    return OkResponse(ok=True)


# ---------------------------------------------------------------------------
# search / resolve internals
# ---------------------------------------------------------------------------


def _music_search(query: str, limit: int) -> tuple[list[Candidate], list[dict], str]:
    """Songs + album expansion + album/artist metadata, concurrently."""
    with ThreadPoolExecutor(max_workers=3) as pool:
        songs = pool.submit(_search_ytmusic, query, limit)
        expanded = pool.submit(_album_candidates, query)
        meta = pool.submit(_find_albums, query)
        albums, top_artist = meta.result()
        return songs.result() + expanded.result(), albums, top_artist


def _search_both(query: str, limit: int = 20) -> tuple[list[Candidate], list[dict], str]:
    with ThreadPoolExecutor(max_workers=3) as pool:
        music = pool.submit(_music_search, query, limit)
        youtube = pool.submit(_search_yt_dlp, query, limit)
        (songs, albums, top_artist) = music.result()
        return songs + youtube.result(), albums, top_artist


def _album_candidates(query: str, max_albums: int = 2) -> list[Candidate]:
    """Expand top YTM album matches into playable track candidates.

    Songs-only YTM search never returns album tracks, and some albums
    (e.g. Drake's Views, J. Cole's 4 Your Eyez Only) are API-restricted
    (get_album tracks have no videoId). Their audio playlists still
    resolve via yt-dlp, so expand those into candidates tagged with the
    album name — ranking boosts album matches (§9.2).
    """
    try:
        from rapidfuzz import fuzz as _fuzz

        albums, _ = _find_albums(query)
        if not albums:
            return []
        import re as _re

        query_terms = set(_re.findall(r"\w+", query.casefold()))
        gated: list[dict] = []
        for a in albums:
            title = (a.get("title") or "").strip()
            if not title:
                continue
            # The album title itself must appear in the query (full or
            # word-wise). Without this, a song query like "Drake Passionfruit"
            # expands a karaoke album that merely contains the song title and
            # its boosted tracks bury the exact song match.
            title_terms = set(_re.findall(r"\w+", title.casefold()))
            if title.casefold() not in query.casefold() and not (
                title_terms and title_terms <= query_terms
            ):
                continue
            # Same for the album artist: otherwise a knockoff album with a
            # matching title ("4 your Your Eyez Only" by Sadmethod) wins over
            # the real record on Topic bonus alone.
            artist = (a.get("artist") or "").strip()
            if artist:
                artist_terms = set(_re.findall(r"\w+", artist.casefold()))
                if (
                    artist.casefold() not in query.casefold()
                    and not (artist_terms and artist_terms <= query_terms)
                    and _fuzz.WRatio(artist, query) < 70
                ):
                    continue
            if _fuzz.WRatio(query, f"{a['artist']} {a['title']}") < 55:
                continue
            gated.append(a)
        scored = sorted(
            gated,
            key=lambda a: _fuzz.WRatio(query, f"{a['artist']} {a['title']}"),
            reverse=True,
        )
        picked = scored[:max_albums]
        if not picked:
            return []
        with ThreadPoolExecutor(max_workers=max_albums) as pool:
            futs = [pool.submit(_expand_album, a) for a in picked]
            out: list[Candidate] = []
            for f in futs:
                try:
                    out.extend(f.result())
                except Exception as e:
                    log.warning("album expansion failed: %s", e)
            return out
    except Exception as e:
        log.warning("album candidate search failed: %s", e)
        return []


def _find_albums(query: str, limit: int = 5) -> tuple[list[dict], str]:
    """Top YTM album matches plus the query's top artist (for ranking)."""
    try:
        seen: set[str] = set()
        out: list[dict] = []
        top_artist = ""

        def add(r: dict) -> None:
            bid = r.get("browseId")
            if not bid or bid in seen:
                return
            seen.add(bid)
            artists = r.get("artists") or []
            artist = artists[0].get("name", "") if artists else ""
            explicit = r.get("isExplicit")
            out.append(
                {
                    "browseId": bid,
                    "title": r.get("title") or "",
                    "artist": artist,
                    "year": r.get("year"),
                    "explicit": explicit if isinstance(explicit, bool) else None,
                    "thumbnails": [
                        {"url": t.get("url", ""), "width": t.get("width", 0), "height": t.get("height", 0)}
                        for t in (r.get("thumbnails") or [])
                        if t.get("url")
                    ],
                }
            )

        # Unfiltered search ranks the best album first; the albums-only
        # filter can bury it under lookalikes (e.g. Drake's ICEMAN).
        # It also reveals the query's top artist for ranking.
        try:
            for r in discovery.ytmusic_call("search", query, limit=limit) or []:
                if r.get("resultType") == "album":
                    add(r)
                elif r.get("resultType") == "artist" and not top_artist:
                    artists = r.get("artists") or []
                    # artist results carry the name under title, not artists
                    top_artist = r.get("title") or r.get("artist") or (
                        artists[0].get("name", "") if artists else ""
                    )
        except Exception as e:
            log.warning("ytmusicapi unfiltered search failed: %s", e)
        try:
            for r in discovery.ytmusic_call("search", query, filter="albums", limit=limit) or []:
                add(r)
        except Exception as e:
            log.warning("ytmusicapi album search failed: %s", e)
        return out, top_artist
    except Exception as e:
        log.warning("ytmusicapi album search failed: %s", e)
        return [], ""


# Expansion cache: browseId → (expires_at, tracks). Album tracklists barely
# change; never pay the playlist extraction twice in an hour.
_album_cache: dict[str, tuple[float, list[Candidate]]] = {}
_album_cache_lock = threading.Lock()


def _norm_title(t: str) -> str:
    return re.sub(r"\s+", " ", (t or "").casefold().strip())


_BROWSER_UA = (
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 "
    "(KHTML, like Gecko) Chrome/146.0.0.0 Safari/537.36"
)


# Known Premium-gated ATV upload (Drake — Keep The Family Close). If the
# connected account can extract it, it has the Music Premium entitlement.
_PREMIUM_PROBE_VIDEO = "6kZqGhC6dQs"
# Public video used to prove cookies authenticate when the premium probe
# is inconclusive (probe video deleted/region-blocked).
_AUTH_PROBE_VIDEO = "dQw4w9WgXcQ"


def _run_yt_probe(video_id: str) -> tuple[bool, str]:
    """(ok, stderr) for a quiet extraction of one video with current cookies."""
    import subprocess

    try:
        r = subprocess.run(
            ytdlp_cmd("--no-playlist", "-f", "bestaudio", "--simulate", "-q",
                      f"https://www.youtube.com/watch?v={video_id}"),
            capture_output=True, text=True, timeout=90,
        )
        return r.returncode == 0, (r.stderr or "")
    except (OSError, subprocess.TimeoutExpired) as e:
        return False, str(e)


def _cookies_authenticated() -> bool:
    """True when YouTube treats the stored cookies as a signed-in session.

    The account page's page-config carries ``"LOGGED_IN": true`` only for
    authenticated requests — the same signal yt-dlp uses. Without this,
    invalid cookies and a free account are indistinguishable: both get
    the "Music Premium members only" error on gated tracks. Two attempts:
    YouTube serves consent variants at random, so a single miss proves
    nothing (real gated extractions keep working meanwhile)."""
    import time as _time

    import httpx

    jar = downloader.read_cookies_dict()
    if not jar:
        return False
    for attempt in range(3):
        try:
            with httpx.Client(timeout=15, follow_redirects=True,
                              headers={"User-Agent": _BROWSER_UA}) as client:
                resp = client.get("https://www.youtube.com/feed/you", cookies=jar)
            if resp.status_code == 200 and re.search(
                    r'"LOGGED_IN"\s*:\s*true', resp.text):
                return True
        except httpx.HTTPError as exc:
            log.info("auth-cookie check attempt %d failed: %s", attempt + 1, exc)
        if attempt < 2:
            _time.sleep(1.5)
    return False


def _probe_youtube_auth() -> tuple[bool, bool, str]:
    """(verified, premium, message) for the currently stored cookies.

    Verifies the session against YouTube itself, then runs yt-dlp against
    a known Premium-gated track, so the Settings page can state plainly
    whether the file authenticates and whether it carries Premium.

    Sign-in evidence comes from the live managed browser when available
    (what Google considers logged-in right now) and only from the page
    scrape for manual-cookie mode: the page check is consent-variant
    flaky while the gated extraction itself keeps working."""
    if session.browser_enabled():
        managed_login = session.browser_logged_in()
        if managed_login is None:
            # Browser unreachable — fall through to the page check.
            pass
        elif managed_login:
            return _premium_probe()
        else:
            return False, False, (
                "The managed browser signed out — a re-login will be attempted automatically.")
    if not _cookies_authenticated():
        return False, False, "YouTube rejected these cookies — export them again while signed in."
    return _premium_probe()


def _premium_probe() -> tuple[bool, bool, str]:
    ok, err = _run_yt_probe(_PREMIUM_PROBE_VIDEO)
    if ok:
        return True, True, "YouTube Music Premium connected"
    low = err.casefold()
    if "music premium" in low:
        return True, False, "Signed in, but this account has no YouTube Music Premium entitlement."
    if "unavailable" in low or "private" in low or "removed" in low:
        ok2, err2 = _run_yt_probe(_AUTH_PROBE_VIDEO)
        if ok2:
            return True, False, "Signed in (Premium entitlement could not be verified)."
        return False, False, (err2.strip().splitlines()[-1][:200] if err2.strip()
                              else "YouTube probe failed.")
    return True, False, "Signed in; Premium entitlement could not be verified right now."


def _expand_album(album: dict) -> list[Candidate]:
    """Resolve one YTM album to playable tracks via its audio playlist."""
    import json
    import subprocess

    browse_id = album["browseId"]
    with _album_cache_lock:
        hit = _album_cache.get(browse_id)
        if hit and hit[0] > time.monotonic():
            return [c.model_copy() for c in hit[1]]

    info = discovery.ytmusic_call("get_album", browse_id)
    playlist_id = info.get("audioPlaylistId")
    if not playlist_id:
        return []
    album_title = info.get("title") or album.get("title") or ""
    artists = info.get("artists") or []
    album_artist = artists[0].get("name", "") if artists else (album.get("artist") or "")
    explicit = info.get("isExplicit")
    album_explicit = explicit if isinstance(explicit, bool) else None
    # Per-track explicit flags from YTM's own tracklist.
    track_explicit: dict[str, bool] = {}
    for t in info.get("tracks") or []:
        nt = _norm_title(t.get("title") or "")
        if not nt:
            continue
        if isinstance(t.get("isExplicit"), bool):
            track_explicit[nt] = t["isExplicit"]
    album_thumbs = [
        Thumbnail(url=t.get("url", ""), width=t.get("width", 0), height=t.get("height", 0))
        for t in (info.get("thumbnails") or [])
        if t.get("url")
    ]
    if shutil.which("yt-dlp") is None:
        return []
    cmd = [
        *ytdlp_cmd(),
        "--dump-single-json",
        "--flat-playlist",
        f"https://music.youtube.com/playlist?list={playlist_id}",
    ]
    out = subprocess.run(cmd, capture_output=True, text=True, timeout=45)
    if out.returncode != 0:
        log.warning("album playlist extract failed for %s: %s", album_title, out.stderr[-300:])
        return []
    data = json.loads(out.stdout)
    cands: list[Candidate] = []
    for position, e in enumerate(data.get("entries") or [], 1):
        if not e:
            continue
        vid = e.get("id")
        if not vid:
            url = e.get("url") or ""
            m = re.search(r"v=([\w-]{6,})", url)
            if m:
                vid = m.group(1)
        if not vid:
            continue
        title = e.get("title") or ""
        thumbs = [
            Thumbnail(url=t.get("url", ""), width=t.get("width", 0), height=t.get("height", 0))
            for t in (e.get("thumbnails") or [])
            if t.get("url")
        ] or album_thumbs
        try:
            duration = float(e.get("duration") or 0)
        except (TypeError, ValueError):
            duration = 0.0
        vc = e.get("view_count")
        cands.append(
            Candidate(
                videoId=vid,
                source="ytmusic",
                title=title,
                channel=e.get("channel") or e.get("uploader") or album_artist,
                album=album_title,
                track=e.get("playlist_index") or position,
                durationSec=duration,
                views=int(vc) if isinstance(vc, (int, float)) else None,
                thumbnails=thumbs,
                badges=["has-metadata"],
                explicit=track_explicit.get(_norm_title(title), album_explicit),
            )
        )
    log.info("album expansion %r → %d tracks", album_title, len(cands))
    with _album_cache_lock:
        if len(_album_cache) >= 32:
            _album_cache.pop(next(iter(_album_cache)))
        _album_cache[browse_id] = (time.monotonic() + 3600, [c.model_copy() for c in cands])
    return cands


def _search_ytmusic(query: str, limit: int) -> list[Candidate]:
    try:
        results = discovery.ytmusic_call("search", query, filter="songs", limit=limit)
        cands: list[Candidate] = []
        for r in results or []:
            vid = r.get("videoId")
            if not vid:
                continue
            thumbs = [
                Thumbnail(url=t.get("url", ""), width=t.get("width", 0), height=t.get("height", 0))
                for t in (r.get("thumbnails") or [])
                if t.get("url")
            ]
            dur = r.get("duration_seconds") or r.get("duration") or 0
            if isinstance(dur, str):
                try:
                    parts = [int(x) for x in dur.split(":")]
                    dur = sum(p * (60 ** (len(parts) - i - 1)) for i, p in enumerate(parts))
                except Exception:
                    dur = 0
            artists = r.get("artists") or []
            channel = artists[0].get("name", "") if artists else (r.get("artist") or "")
            views = parse_views(r.get("views"))
            alb = r.get("album") or {}
            cands.append(
                Candidate(
                    videoId=vid,
                    source="ytmusic",
                    title=r.get("title") or "",
                    channel=channel or "",
                    album=alb.get("name") or None,
                    durationSec=float(dur or 0),
                    views=views,
                    thumbnails=thumbs,
                    badges=["has-metadata"],
                    explicit=r.get("isExplicit") if isinstance(r.get("isExplicit"), bool) else None,
                )
            )
        return cands
    except Exception as e:
        log.warning("ytmusicapi search failed: %s", e)
        return []


def _search_yt_dlp(query: str, limit: int) -> list[Candidate]:
    """Plain YouTube ytsearch — covers content YT Music misses (§3.4, §8.3)."""
    try:
        import json
        import subprocess

        if shutil.which("yt-dlp") is None:
            return []
        cmd = ytdlp_cmd(
            f"ytsearch{limit}:{query}",
            "--dump-single-json",
            "--flat-playlist",
            "--playlist-items", f"1-{limit}",
        )
        out = subprocess.run(cmd, capture_output=True, text=True, timeout=18)
        if out.returncode != 0:
            log.warning("yt-dlp search failed: %s", out.stderr[-300:])
            return []
        data = json.loads(out.stdout)
        entries = data.get("entries") or []
        cands: list[Candidate] = []
        for e in entries:
            vid = e.get("id")
            if not vid:
                continue
            thumbs = [
                Thumbnail(url=t.get("url", ""), width=t.get("width", 0), height=t.get("height", 0))
                for t in (e.get("thumbnails") or [])
                if t.get("url")
            ]
            vc = e.get("view_count")
            cands.append(
                Candidate(
                    videoId=vid,
                    source="youtube",
                    title=e.get("title") or "",
                    channel=e.get("channel") or e.get("uploader") or "",
                    durationSec=float(e.get("duration") or 0),
                    views=int(vc) if isinstance(vc, (int, float)) else None,
                    thumbnails=thumbs,
                )
            )
        return cands
    except Exception as e:
        log.warning("yt-dlp search error: %s", e)
        return []


_YT_URL = re.compile(
    r"^https?://(?:www\.|m\.|music\.)?(?:youtube\.com|youtu\.be)/", re.I
)


def _resolve_url(url: str) -> tuple[str, list[Candidate], str | None]:
    if not _YT_URL.match(url):
        raise HTTPException(status_code=400, detail="not a YouTube URL")

    is_playlist = bool(re.search(r"[?&]list=|/playlist|/browse/", url))
    is_albumish = "/browse/" in url and "music.youtube.com" in url

    try:
        info = downloader.extract_info(url)
    except DownloadError as e:
        raise HTTPException(status_code=502, detail=str(e))

    # Single video
    if info.get("_type") != "playlist" and info.get("id") and not is_playlist:
        c = _info_to_candidate(info, source="youtube")
        return "video", [c], info.get("title")

    # Playlist / album
    entries = info.get("entries") or []
    title = info.get("title")
    kind = "album" if is_albumish else "playlist"
    cands = []
    for e in entries:
        if not e:
            continue
        if e.get("id") is None and e.get("url"):
            # sometimes flat entries only have url
            m = re.search(r"v=([\w-]+)", e.get("url", ""))
            if m:
                e = {**e, "id": m.group(1)}
        if not e.get("id"):
            continue
        cands.append(_info_to_candidate(e, source="youtube"))
    if not cands and info.get("id"):
        cands = [_info_to_candidate(info, source="youtube")]
        kind = "video"
    return kind, cands, title


def _info_to_candidate(info: dict, *, source: str) -> Candidate:
    thumbs_raw = info.get("thumbnails") or []
    thumbs = [
        Thumbnail(url=t.get("url", ""), width=t.get("width", 0), height=t.get("height", 0))
        for t in thumbs_raw
        if t.get("url")
    ]
    channel = (
        info.get("channel")
        or info.get("uploader")
        or (info.get("artist") or "")
        or ""
    )
    return Candidate(
        videoId=info.get("id") or "",
        source=source,  # type: ignore[arg-type]
        title=info.get("title") or "",
        channel=channel,
        durationSec=float(info.get("duration") or 0),
        thumbnails=thumbs,
        badges=[],
    )

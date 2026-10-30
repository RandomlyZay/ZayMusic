"""yt-dlp invocation — native codec, no re-encode (§3.3, §9.3)."""

from __future__ import annotations

import json
import logging
import os
import re
import shutil
import subprocess
import threading
import time
from dataclasses import dataclass, field
from pathlib import Path
from queue import Empty, Queue
from typing import Callable, Mapping

log = logging.getLogger("zayfetch.downloader")

ProgressFn = Callable[[float, str], None]


class DownloadError(Exception):
    pass


@dataclass
class DownloadedMeta:
    video_id: str
    title: str
    artist: str = ""
    album: str = ""
    duration: float = 0.0
    ext: str = ""
    path: Path | None = None
    thumbnail: str = ""
    extra: dict = field(default_factory=dict)


def ytdlp_version() -> str:
    try:
        out = subprocess.run(
            ["yt-dlp", "--version"], capture_output=True, text=True, timeout=15
        )
        return out.stdout.strip() or "unknown"
    except Exception:
        return "unavailable"


def _staging_dir() -> Path:
    p = Path(os.environ.get("STAGING_DIR", "/data/staging"))
    p.mkdir(parents=True, exist_ok=True)
    return p


COOKIES_REQUIRED_MESSAGE = (
    "YouTube Music Premium is not connected. Open Settings → YouTube Music "
    "to connect your account and enable downloads."
)
_COOKIES_DEFAULT_NAME = "cookies/youtube.txt"
# PO Token provider sidecar (bgutil). Empty disables the token argument.
POT_PROVIDER_URL = os.environ.get("POT_PROVIDER_URL", "http://pot-provider:4416").strip()


def cookies_path() -> Path:
    """Where the authenticated YouTube cookies live.

    ``YTDLP_COOKIES_FILE`` overrides; otherwise the zayfetch data volume
    (the in-app Settings connect flow writes here)."""
    override = os.environ.get("YTDLP_COOKIES_FILE", "").strip()
    if override:
        return Path(override)
    data = Path(os.environ.get("DATA_DIR", "/data"))
    return data / _COOKIES_DEFAULT_NAME


def cookies_status_path() -> Path:
    return cookies_path().with_name("youtube.status.json")


def cookies_file() -> str:
    """Path to usable cookies.txt, or empty when unset/missing."""
    path = cookies_path()
    return str(path) if path.is_file() else ""


def write_cookies(text: str) -> Path:
    """Atomically store a cookies.txt (mode 600). Raises ValueError when
    the text can't be understood as YouTube sign-in cookies."""
    body = normalize_cookies(text)
    path = cookies_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(".tmp")
    temporary.write_text(body, encoding="utf-8")
    os.chmod(temporary, 0o600)
    os.replace(temporary, path)
    return path


_AUTH_COOKIE_RE = re.compile(
    r"(^|__secure-.*)(sid|ssid|sapisid|apisid|hsid|login_info)$", re.I
)
_COOKIE_NAME_RE = r"[\w!#$%&'*+\-.^`|~]+"


def normalize_cookies(text: str) -> str:
    """Return Netscape cookies.txt for any of the formats a user can
    realistically produce:

    - an extension export (already Netscape),
    - a DevTools "Copy as cURL" command (bash or PowerShell),
    - a raw ``Cookie:`` request-header value.

    Raises ValueError with an actionable message when nothing usable is
    found."""
    body = (text or "").strip()
    if not body:
        raise ValueError("Paste cookies.txt contents (or a Copy as cURL command) first.")
    if _looks_like_netscape(body):
        return body + "\n"
    header = _cookie_header_from_curl(body)
    if header is None:
        header = _cookie_header_from_raw(body)
    if header is None:
        raise ValueError(
            "Could not read cookies from that. Export a Netscape cookies.txt, "
            "paste a “Copy as cURL” command, or paste the Cookie request header."
        )
    pairs = _cookie_pairs(header)
    if not pairs:
        raise ValueError("No cookies were found in that text.")
    if not any(_AUTH_COOKIE_RE.search(name) for name in pairs):
        raise ValueError(
            "Those cookies do not include a YouTube sign-in. Make sure you are "
            "signed in to music.youtube.com, then copy again."
        )
    lines = ["# Netscape HTTP Cookie File", "# converted by ZayMusic"]
    for name, value in pairs.items():
        lines.append(f".youtube.com\tTRUE\t/\tTRUE\t0\t{name}\t{value}")
    return "\n".join(lines) + "\n"


def _cookie_header_from_curl(text: str) -> str | None:
    """Cookie header inside a copied curl command (bash or PowerShell)."""
    patterns = (
        r"(?:-H|--header)\s+['\"](?:cookie|Cookie):\s*([^'\"]+)['\"]",
        r"(?:-b|--cookie)\s+['\"]([^'\"]+)['\"]",
        r"(?:-H|--header)\s+\"(?:cookie|Cookie):\s*([^\"]+)\"",
    )
    for pattern in patterns:
        m = re.search(pattern, text)
        if m:
            return m.group(1).strip()
    return None


def _cookie_header_from_raw(text: str) -> str | None:
    """A bare ``name=value; name2=value2`` header value."""
    candidate = text
    m = re.match(r"(?i)^cookie:\s*(.+)$", candidate, re.S)
    if m:
        candidate = m.group(1)
    candidate = candidate.replace("\r", " ").replace("\n", " ").strip()
    if "=" not in candidate:
        return None
    segments = [seg.strip() for seg in candidate.split(";") if seg.strip()]
    pair_re = re.compile(rf"^{_COOKIE_NAME_RE}=[^;]*$")
    if not segments or not all(pair_re.match(seg) for seg in segments):
        return None
    return candidate


def _cookie_pairs(header: str) -> dict[str, str]:
    pairs: dict[str, str] = {}
    for segment in header.split(";"):
        segment = segment.strip()
        if not segment or "=" not in segment:
            continue
        name, _, value = segment.partition("=")
        name, value = name.strip(), value.strip().strip('"')
        if name and re.fullmatch(_COOKIE_NAME_RE, name):
            pairs[name] = value
    return pairs


def remove_cookies() -> bool:
    path = cookies_path()
    existed = path.is_file()
    path.unlink(missing_ok=True)
    cookies_status_path().unlink(missing_ok=True)
    return existed


def read_cookies_status() -> dict:
    import json

    path = cookies_status_path()
    if not path.is_file():
        return {}
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
        return data if isinstance(data, dict) else {}
    except (OSError, ValueError):
        return {}


def write_cookies_status(status: dict) -> None:
    import json

    try:
        path = cookies_status_path()
        path.parent.mkdir(parents=True, exist_ok=True)
        temporary = path.with_suffix(".tmp")
        temporary.write_text(json.dumps(status), encoding="utf-8")
        os.chmod(temporary, 0o600)
        os.replace(temporary, path)
    except OSError as e:
        log.info("could not persist cookies status: %s", e)


def _looks_like_netscape(text: str) -> bool:
    """Netscape cookies.txt, as exported by browser extensions:
    tab-separated domain/flag/path/secure/expiry/name/value lines."""
    if "# netscape http cookie file" in text.casefold():
        return True
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        parts = line.split("\t")
        if len(parts) >= 7 and "youtube" in parts[0].casefold():
            return True
    return False


def read_cookies_dict() -> dict[str, str]:
    """Name→value for the Google/YouTube cookies in the stored file."""
    out: dict[str, str] = {}
    path = cookies_path()
    try:
        lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return out
    for line in lines:
        line = line.strip()
        if line.startswith("#HttpOnly_"):
            line = line[len("#HttpOnly_"):]
        elif not line or line.startswith("#"):
            continue
        parts = line.split("\t")
        if len(parts) < 7:
            continue
        domain = parts[0].casefold()
        if "youtube" not in domain and "google" not in domain and "googlevideo" not in domain:
            continue
        name, value = parts[5].strip(), parts[6].strip()
        if name:
            out[name] = value
    return out


# --- managed browser session + OAuth helpers (§5.2) ------------------------

# yt-dlp's own sign-in test (extractor/youtube/_base.py): LOGIN_INFO plus one
# of the SAPISID cookies. Mirrored exactly so "signed in" means the same here.
_LOGIN_INFO = "LOGIN_INFO"
_SAPISID_NAMES = ("SAPISID", "__Secure-1PSAPISID", "__Secure-3PSAPISID")
_AUTH_COOKIE_NAMES = frozenset((_LOGIN_INFO, *_SAPISID_NAMES))
_COOKIE_DOMAINS = ("youtube.com", "google.com", "googlevideo.com")


def load_cookiejar(path: Path | None = None):
    """Load the stored cookies.txt into yt-dlp's jar.

    Uses ``YoutubeDLCookieJar`` so ``#HttpOnly_`` lines and ``expires=0``
    session cookies survive the round-trip. Never raises: an unreadable file
    yields an empty jar."""
    from yt_dlp.cookies import YoutubeDLCookieJar

    target = path or cookies_path()
    jar = YoutubeDLCookieJar(str(target))
    try:
        if target.is_file():
            jar.load()
    except Exception as e:  # noqa: BLE001 — a bad file must not take the app down
        log.info("could not load cookie jar %s: %s", target, e)
    return jar


def cookiejar_to_netscape(jar) -> str:
    import io

    buf = io.StringIO()
    jar.save(buf, ignore_discard=True, ignore_expires=True)
    return buf.getvalue()


def write_netscape(body: str) -> Path:
    """Atomically store Netscape cookies (mode 600).

    Same durability rules as ``write_cookies`` but for text that is already
    known-good (a live export from the managed browser)."""
    path = cookies_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(".tmp")
    temporary.write_text(body, encoding="utf-8")
    os.chmod(temporary, 0o600)
    os.replace(temporary, path)
    return path


def cookies_from_cdp(payload: list[dict]) -> str:
    """Netscape text for a CDP ``context.cookies()`` payload.

    ``[{name, value, domain, path, expires, httpOnly, secure}]`` -> cookies.txt.
    """
    lines = ["# Netscape HTTP Cookie File", "# exported from the managed browser"]
    for item in payload or []:
        if not isinstance(item, dict):
            continue
        name = str(item.get("name") or "")
        domain = str(item.get("domain") or "")
        if not name or not domain:
            continue
        value = str(item.get("value") or "")
        path = str(item.get("path") or "/")
        secure = "TRUE" if item.get("secure") else "FALSE"
        include_sub = "TRUE" if domain.startswith(".") else "FALSE"
        raw_expires = item.get("expires")
        try:
            expires = int(raw_expires) if raw_expires not in (None, -1, "") else 0
        except (TypeError, ValueError):
            expires = 0
        if expires < 0:
            expires = 0
        prefix = "#HttpOnly_" if item.get("httpOnly") else ""
        lines.append(
            f"{prefix}{domain}\t{include_sub}\t{path}\t{secure}\t{expires}\t{name}\t{value}"
        )
    return "\n".join(lines) + "\n"


def has_login_session(cookies: Mapping[str, str]) -> bool:
    """True when the cookies include a YouTube sign-in.

    Mirrors yt-dlp's ``_has_auth_cookies``: ``LOGIN_INFO`` and one of
    ``SAPISID`` / ``__Secure-1PSAPISID`` / ``__Secure-3PSAPISID``."""
    names = set(cookies)
    if _LOGIN_INFO not in names:
        return False
    return any(name in names for name in _SAPISID_NAMES)


def session_expiry_info() -> dict:
    """Soonest-expiring auth cookie in the stored file.

    ``{'expiresAt': epoch|None, 'expiresInDays': float|None}``. None when no
    auth cookie carries an expiry (all session cookies)."""
    now = time.time()
    expiries: list[float] = []
    try:
        for cookie in load_cookiejar():
            if cookie.name in _AUTH_COOKIE_NAMES and cookie.expires:
                expiries.append(float(cookie.expires))
    except Exception as e:  # noqa: BLE001
        log.info("could not read cookie expiry: %s", e)
    if not expiries:
        return {"expiresAt": None, "expiresInDays": None}
    soonest = min(expiries)
    return {"expiresAt": soonest, "expiresInDays": (soonest - now) / 86400.0}


def oauth_path() -> Path:
    data = Path(os.environ.get("DATA_DIR", "/data"))
    return data / "youtube" / "oauth.json"


def oauth_device_path() -> Path:
    return oauth_path().with_name("oauth.device.json")


def oauth_configured() -> bool:
    """Both OAuth env vars present. Client id/secret are user-supplied."""
    return bool(oauth_client_id() and oauth_client_secret())


def oauth_client_id() -> str:
    return os.environ.get("YTM_OAUTH_CLIENT_ID", "").strip()


def oauth_client_secret() -> str:
    return os.environ.get("YTM_OAUTH_CLIENT_SECRET", "").strip()


def write_oauth_token(token: dict) -> Path:
    """Persist an OAuth token dict (mode 600) via the atomic temp/replace pattern."""
    path = oauth_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(".tmp")
    temporary.write_text(json.dumps(token), encoding="utf-8")
    os.chmod(temporary, 0o600)
    os.replace(temporary, path)
    return path


def remove_oauth() -> bool:
    path = oauth_path()
    existed = path.is_file()
    path.unlink(missing_ok=True)
    remove_device_code()
    return existed


def write_device_code(device_code: str) -> None:
    """Persist the pending device code (mode 600) between start and poll.

    Never log the value."""
    path = oauth_device_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(".tmp")
    temporary.write_text(
        json.dumps({"deviceCode": device_code, "createdAt": time.time()}), encoding="utf-8"
    )
    os.chmod(temporary, 0o600)
    os.replace(temporary, path)


def read_device_code() -> str:
    path = oauth_device_path()
    if not path.is_file():
        return ""
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return ""
    return str(data.get("deviceCode") or "") if isinstance(data, dict) else ""


def remove_device_code() -> bool:
    path = oauth_device_path()
    existed = path.is_file()
    path.unlink(missing_ok=True)
    return existed


def refresh_cookies_from_browser() -> bool:
    """When browser mode is on, export live cookies and store them.

    Called immediately before every authenticated yt-dlp download so the jar
    reflects the browser's current Google session (§5.3 freshness guarantee).
    Returns True when the stored file was refreshed. Never raises."""
    try:
        from . import session
    except Exception:  # noqa: BLE001
        return False
    if not session.browser_enabled():
        return False
    try:
        text = session.export_cookies_from_browser()
        if not text:
            return False
        write_netscape(text)
        return True
    except Exception as e:  # noqa: BLE001
        log.info("could not refresh cookies from the browser: %s", e)
        return False


def ytdlp_cmd(*args: str, with_cookies: bool = True) -> list[str]:
    """yt-dlp argv with the standard self-hosted hardening:

    - Deno is used automatically when installed (yt-dlp's recommended JS
      runtime for the EJS challenge solver). If only Node is present it
      is passed explicitly.
    - With an account connected: ``--cookies`` plus the ``web_music``
      client and the PO Token provider. Authenticated sessions are served
      SABR-only formats by the web/mweb clients; ``web_music`` with a GVS
      PO token is the combination that actually returns audio formats.
    - ``with_cookies=False`` explicitly skips account authentication.
      Downloads and playback never drop a connected account on failure.
    - Chrome impersonation when curl_cffi is installed. Without curl_cffi
      the flag would hard-fail every call, so it is only added when
      importable.
    """
    cmd = ["yt-dlp"]
    if not shutil.which("deno") and shutil.which("node"):
        cmd += ["--js-runtimes", "node"]
    if with_cookies and cookies_file():
        cmd += ["--cookies", cookies_file()]
        cmd += ["--extractor-args", "youtube:player_client=web_music"]
        if POT_PROVIDER_URL:
            cmd += ["--extractor-args", f"youtubepot-bgutilhttp:base_url={POT_PROVIDER_URL}"]
    try:
        import curl_cffi  # noqa: F401

        cmd += ["--impersonate", "chrome"]
    except ImportError:
        pass
    return cmd + list(args)


def build_cmd(video_id: str, staging: Path, *, playlist: bool = False,
              with_cookies: bool = True) -> list[str]:
    # Freshness guarantee (§5.3): when the managed browser owns the session,
    # re-export its cookies seconds before yt-dlp reads them. No-op otherwise.
    if with_cookies:
        refresh_cookies_from_browser()
    url = f"https://www.youtube.com/watch?v={video_id}"
    cmd = ytdlp_cmd(
        "-f", "bestaudio/best",
        "--socket-timeout", "30",
        "--retries", "3",
        "--fragment-retries", "3",
        "--newline",
        "--embed-metadata",
        "--write-info-json",
        "--no-playlist" if not playlist else "--yes-playlist",
        "-o", str(staging / "%(id)s.%(ext)s"),
        url,
        with_cookies=with_cookies,
    )
    return cmd


_PCT_RE = re.compile(r"(\d{1,3}(?:\.\d+)?)%")


def download(
    video_id: str,
    *,
    staging: Path | None = None,
    playlist: bool = False,
    on_progress: ProgressFn | None = None,
    cancel_check: Callable[[], bool] | None = None,
    with_cookies: bool = True,
) -> DownloadedMeta:
    """Run yt-dlp, parse progress, return metadata. Never re-encodes (§3.3)."""
    if not re.fullmatch(r"[A-Za-z0-9_-]{6,64}", video_id):
        raise DownloadError("invalid YouTube video id")

    staging = staging or _staging_dir()
    staging.mkdir(parents=True, exist_ok=True)
    work = staging / video_id
    if work.exists():
        shutil.rmtree(work)
    work.mkdir(parents=True, exist_ok=True)

    if shutil.which("yt-dlp") is None:
        raise DownloadError("yt-dlp binary not found in container")

    cmd = build_cmd(video_id, work, playlist=playlist, with_cookies=with_cookies)
    proc = subprocess.Popen(
        cmd,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
    )
    last_line = ""
    lines: Queue[str | None] = Queue()

    def read_output() -> None:
        assert proc.stdout is not None
        for output_line in proc.stdout:
            lines.put(output_line)
        lines.put(None)

    reader = threading.Thread(target=read_output, daemon=True)
    reader.start()
    started_at = time.monotonic()
    try:
        output_finished = False
        while not output_finished:
            if time.monotonic() - started_at > 1800:
                proc.kill()
                raise DownloadError("download timed out after 30 minutes")
            if cancel_check and cancel_check():
                proc.kill()
                raise DownloadError("cancelled")
            try:
                output_line = lines.get(timeout=0.25)
            except Empty:
                continue
            if output_line is None:
                output_finished = True
                continue

            line = output_line.rstrip("\n")
            last_line = line
            if on_progress:
                m = _PCT_RE.search(line)
                if m:
                    try:
                        on_progress(float(m.group(1)) / 100.0, "download")
                    except ValueError:
                        pass
        code = proc.wait()
    finally:
        if proc.poll() is None:
            proc.kill()

    if code != 0:
        raise DownloadError(f"yt-dlp exit {code}: {last_line[-500:]}")

    # Find the downloaded file (skip .meta sidecar)
    files = [
        p
        for p in work.iterdir()
        if p.is_file()
        and p.suffix != ".meta"
        and not p.name.endswith((".part", ".info.json"))
    ]
    if not files:
        # single video sometimes lands directly in staging when --no-playlist
        files = [
            p
            for p in staging.iterdir()
            if p.is_file()
            and p.suffix != ".meta"
            and not p.name.endswith((".part", ".info.json"))
            and p.name.startswith(video_id)
        ]
    if not files:
        raise DownloadError(f"downloaded file not found for {video_id}")

    # Prefer the largest non-meta file
    path = max(files, key=lambda p: p.stat().st_size)

    meta = _parse_info_json(work / f"{video_id}.info.json") or DownloadedMeta(
        video_id=video_id, title=path.stem
    )
    meta.path = path
    meta.ext = path.suffix.lstrip(".") or path.name.split(".")[-1]
    if not meta.title:
        meta.title = path.stem
    return meta


def _parse_info_json(info_path: Path) -> DownloadedMeta | None:
    if not info_path.exists():
        return None
    try:
        info = json.loads(info_path.read_text(encoding="utf-8", errors="replace"))
    except (json.JSONDecodeError, OSError):
        return None
    if not isinstance(info, dict):
        return None
    try:
        duration = float(info.get("duration") or 0)
    except (TypeError, ValueError):
        duration = 0.0
    thumbs = info.get("thumbnails") or []
    thumbnail = info.get("thumbnail") or next(
        (t.get("url") for t in reversed(thumbs) if isinstance(t, dict) and t.get("url")),
        "",
    )
    return DownloadedMeta(
        video_id=str(info.get("id") or info_path.name.removesuffix(".info.json")),
        title=str(info.get("track") or info.get("title") or ""),
        artist=str(info.get("artist") or info.get("creator") or ""),
        album=str(info.get("album") or ""),
        duration=duration,
        ext=str(info.get("ext") or ""),
        thumbnail=str(thumbnail or ""),
        extra={"acodec": str(info.get("acodec") or "")},
    )


def normalize_for_library(meta: DownloadedMeta) -> DownloadedMeta:
    """Remux native WebM audio into containers Navidrome and Mutagen can read."""
    if not meta.path or meta.path.suffix.lower() != ".webm":
        return meta

    codec = str(meta.extra.get("acodec") or "").lower()
    containers = {"opus": ("opus", "opus"), "vorbis": ("ogg", "ogg")}
    if codec not in containers:
        raise DownloadError(f"unsupported WebM audio codec: {codec or 'unknown'}")

    extension, muxer = containers[codec]
    target = meta.path.with_suffix(f".{extension}")
    temporary = meta.path.with_name(f"{meta.path.stem}.normalized.{extension}")
    cmd = [
        "ffmpeg",
        "-nostdin",
        "-hide_banner",
        "-loglevel", "error",
        "-y",
        "-i", str(meta.path),
        "-map", "0:a:0",
        "-vn",
        "-c:a", "copy",
        "-map_metadata", "0",
        "-f", muxer,
        str(temporary),
    ]
    try:
        result = subprocess.run(cmd, capture_output=True, text=True, timeout=300)
    except (OSError, subprocess.TimeoutExpired) as exc:
        temporary.unlink(missing_ok=True)
        raise DownloadError(f"could not remux WebM audio: {exc}") from exc
    if result.returncode != 0 or not temporary.is_file() or temporary.stat().st_size == 0:
        temporary.unlink(missing_ok=True)
        detail = (result.stderr or "ffmpeg produced no audio file").strip()[-500:]
        raise DownloadError(f"could not remux WebM audio: {detail}")

    os.replace(temporary, target)
    meta.path.unlink(missing_ok=True)
    meta.path = target
    meta.ext = extension
    return meta


def extract_info(url: str) -> dict:
    """Resolve a URL to playlist/video info without downloading."""
    if shutil.which("yt-dlp") is None:
        raise DownloadError("yt-dlp binary not found")
    cmd = ytdlp_cmd(
        "--dump-single-json",
        "--no-playlist",
        "--flat-playlist",
        "--playlist-items", "1-50",
        url,
    )
    # For explicit playlist URLs, yt-dlp wants the playlist
    if "list=" in url or "/playlist" in url or "/browse/" in url:
        cmd = ytdlp_cmd(
            "--dump-single-json",
            "--flat-playlist",
            "--playlist-items", "1-50",
            url,
        )
    out = subprocess.run(cmd, capture_output=True, text=True, timeout=60)
    if out.returncode != 0:
        raise DownloadError(f"yt-dlp extract failed: {out.stderr[-500:]}")
    return json.loads(out.stdout)

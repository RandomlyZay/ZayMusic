"""Metadata writing with mutagen (§9.4). MusicBrainz repair is optional/last."""

from __future__ import annotations

import logging
import os
import re
import shutil
import tempfile
import threading
import time
from pathlib import Path

from .models import Overrides

log = logging.getLogger("zayfetch.tagger")


def apply_tags(
    path: Path,
    *,
    title: str,
    artist: str,
    album: str = "",
    track: int | None = None,
    year: int | None = None,
    genre: str = "",
    albumartist: str = "",
    overrides: Overrides | None = None,
) -> dict:
    """Write tags. Returns the final tag dict used."""
    if overrides:
        title = overrides.title or title
        artist = overrides.artist or artist
        album = overrides.album or album
        if overrides.track is not None:
            track = overrides.track
        if overrides.year is not None:
            year = overrides.year

    tags = {
        "title": title,
        "artist": artist or "Unknown Artist",
        "album": album or "Unknown Album",
        "tracknumber": str(track) if track else "",
        "date": str(year) if year else "",
        "genre": genre,
        "albumartist": albumartist or artist or "Unknown Artist",
    }
    tags = {k: v for k, v in tags.items() if v}

    try:
        _write_mutagen(path, tags)
    except Exception as e:
        log.warning("mutagen write failed for %s: %s", path, e)

    return tags


def _write_mutagen(path: Path, tags: dict[str, str]) -> None:
    import mutagen

    audio = mutagen.File(str(path), easy=True)
    if audio is None:
        raise ValueError(f"unsupported file for tagging: {path}")
    if audio.tags is None:
        audio.add_tags()
    for k, v in tags.items():
        if v is None or v == "":
            continue
        # easy interface: tracknumber/date are standard keys
        audio.tags[k] = v if k not in ("tracknumber", "date") else str(v)
    audio.save()



def embed_cover(path: Path, cover_path: Path | None) -> bool:
    """Embed cover art if mutagen supports the container."""
    if not cover_path or not cover_path.exists():
        return False
    try:
        import mutagen
        from mutagen.flac import FLAC
        from mutagen.id3 import APIC, ID3
        from mutagen.mp4 import MP4Cover

        data = cover_path.read_bytes()
        suffix = path.suffix.lower()

        if suffix == ".flac":
            audio = FLAC(str(path))
            audio["picture"] = _flac_pic(data)
            audio.save()
            return True
        if suffix in (".mp3", ".mp2"):
            audio = ID3(str(path))
            audio.add(APIC(encoding=3, mime="image/jpeg", type=3, desc="Cover", data=data))
            audio.save()
            return True
        if suffix in (".m4a", ".mp4"):
            audio = mutagen.File(str(path))
            if audio is not None and audio.tags is not None:
                audio.tags["covr"] = [MP4Cover(data, imageformat=MP4Cover.FORMAT_JPEG)]
                audio.save()
                return True
        if suffix == ".opus" or suffix == ".ogg":
            # vorbis comment pictures are non-trivial; skip silently
            return False
        return False
    except Exception:
        return False


def _flac_pic(data: bytes):
    from mutagen.flac import Picture

    p = Picture()
    p.type = 3
    p.mime = "image/jpeg"
    p.data = data
    return p


def download_thumbnail(url: str, dest_dir: Path) -> Path | None:
    if not url:
        return None
    try:
        import httpx

        dest = dest_dir / "cover.jpg"
        r = httpx.get(url, timeout=20, follow_redirects=True)
        r.raise_for_status()
        dest.write_bytes(r.content)
        return dest
    except Exception:
        return None


# ---------------------------------------------------------------------------
# Manual tag edits on library files (POST /music/{id}/tags)
# ---------------------------------------------------------------------------

EDITABLE_TAGS = ("title", "artist", "album", "track", "year", "genre")
_TEXT_TAGS = ("title", "artist", "album", "genre")
_edit_lock = threading.Lock()


class UnsupportedTagFormat(ValueError):
    """The file is not a container this editor writes (e.g. WMA, APE)."""


class InvalidTagValue(ValueError):
    """A value this container cannot store as typed (e.g. ID3 genre "17")."""


class TagEditError(RuntimeError):
    """Writing or verifying the edited copy failed; the original is untouched."""


def _leading_int(value: object) -> int | None:
    match = re.match(r"\s*(\d+)", str(value or ""))
    number = int(match.group(1)) if match else 0
    return number or None


def _with_total(old: str, number: int) -> str:
    """Replace the track number, keeping an existing ``/total``."""
    total = old.partition("/")[2].strip()
    return f"{number}/{total}" if total.isdigit() else str(number)


class _ID3Tags:
    """MP3, WAV and AIFF all carry an ID3v2 tag through mutagen."""

    frames = {"title": "TIT2", "artist": "TPE1", "album": "TALB", "track": "TRCK",
              "year": "TDRC", "genre": "TCON"}

    def __init__(self, audio) -> None:
        self.audio = audio
        if audio.tags is None:
            audio.add_tags()
        self.tags = audio.tags

    def _text(self, frame_id: str) -> list[str]:
        frame = self.tags.get(frame_id)
        if frame is None:
            return []
        if frame_id == "TCON":
            return [g for g in frame.genres if g]
        return [str(t) for t in frame.text if str(t)]

    def read(self) -> dict:
        values = {name: "; ".join(self._text(fid)) for name, fid in self.frames.items()}
        return _normalized(values)

    def write(self, name: str, value: str | int | None) -> None:
        from mutagen import id3

        frame_id = self.frames[name]
        if value is None or value == "":
            self.tags.delall(frame_id)
            return
        if name == "track":
            text = _with_total(str(self.tags.get("TRCK") or ""), int(value))
        else:
            text = str(value)
        if name == "genre" and id3.TCON(encoding=3, text=[text]).genres != [text]:
            # ID3 reads bare numbers and "CR"/"RX" as ID3v1 genre codes.
            raise InvalidTagValue("This genre cannot be stored in this file type")
        self.tags.setall(frame_id, [getattr(id3, frame_id)(encoding=3, text=[text])])

    def save(self) -> None:
        # Keep the file's ID3 revision; v2.3 tags stay readable by old players.
        version = 3 if self.tags.version[:2] == (2, 3) else 4
        self.audio.save(v2_version=version)


class _VorbisTags:
    """FLAC and Ogg (Vorbis, Opus, FLAC, Speex) Vorbis comments."""

    keys = {"title": "title", "artist": "artist", "album": "album",
            "track": "tracknumber", "year": "date", "genre": "genre"}

    def __init__(self, audio) -> None:
        self.audio = audio
        if audio.tags is None:
            audio.add_tags()
        self.tags = audio.tags

    def read(self) -> dict:
        values = {name: "; ".join(v for v in self.tags.get(key, []) if v)
                  for name, key in self.keys.items()}
        return _normalized(values)

    def write(self, name: str, value: str | int | None) -> None:
        key = self.keys[name]
        if value is None or value == "":
            if key in self.tags:
                del self.tags[key]
            return
        if name == "track":
            old = (self.tags.get(key) or [""])[0]
            self.tags[key] = [_with_total(old, int(value))]
        else:
            self.tags[key] = [str(value)]

    def save(self) -> None:
        self.audio.save()


class _MP4Tags:
    keys = {"title": "\xa9nam", "artist": "\xa9ART", "album": "\xa9alb",
            "year": "\xa9day", "genre": "\xa9gen"}

    def __init__(self, audio) -> None:
        self.audio = audio
        if audio.tags is None:
            audio.add_tags()
        self.tags = audio.tags

    def read(self) -> dict:
        values: dict[str, object] = {
            name: "; ".join(str(v) for v in self.tags.get(key, []) if str(v))
            for name, key in self.keys.items()
        }
        trkn = self.tags.get("trkn") or []
        values["track"] = trkn[0][0] if trkn else None
        return _normalized(values)

    def write(self, name: str, value: str | int | None) -> None:
        if name == "track":
            if value is None:
                self.tags.pop("trkn", None)
                return
            old = self.tags.get("trkn") or [(0, 0)]
            self.tags["trkn"] = [(int(value), old[0][1] if len(old[0]) > 1 else 0)]
            return
        # mutagen loads a numeric "gnre" atom as ©gen and saves it back that way.
        key = self.keys[name]
        if value is None or value == "":
            self.tags.pop(key, None)
        else:
            self.tags[key] = [str(value)]

    def save(self) -> None:
        self.audio.save()


def _normalized(values: dict) -> dict:
    out = {name: str(values.get(name) or "").strip() for name in _TEXT_TAGS}
    out["track"] = _leading_int(values.get("track"))
    out["year"] = _leading_int(values.get("year"))
    if out["year"] is not None and not 1000 <= out["year"] <= 9999:
        out["year"] = None
    return out


def _tag_adapter(path: Path):
    import mutagen
    from mutagen.aiff import AIFF
    from mutagen.flac import FLAC
    from mutagen.mp3 import MP3
    from mutagen.mp4 import MP4
    from mutagen.oggflac import OggFLAC
    from mutagen.oggopus import OggOpus
    from mutagen.oggspeex import OggSpeex
    from mutagen.oggvorbis import OggVorbis
    from mutagen.wave import WAVE

    try:
        audio = mutagen.File(str(path))
    except Exception as exc:  # noqa: BLE001 — mutagen raises many parse errors
        raise UnsupportedTagFormat("This file could not be read as audio") from exc
    if isinstance(audio, (MP3, WAVE, AIFF)):
        return _ID3Tags(audio)
    if isinstance(audio, (FLAC, OggVorbis, OggOpus, OggFLAC, OggSpeex)):
        return _VorbisTags(audio)
    if isinstance(audio, MP4):
        return _MP4Tags(audio)
    raise UnsupportedTagFormat("Tag editing is not supported for this file type")


def read_song_tags(path: Path) -> dict:
    return _tag_adapter(path).read()


def _stream_signature(path: Path) -> tuple:
    """Codec/stream facts that a tag rewrite must never change."""
    adapter = _tag_adapter(path)
    info = adapter.audio.info
    return (
        type(adapter.audio).__name__,
        round(float(getattr(info, "length", 0) or 0), 2),
        getattr(info, "sample_rate", None),
        getattr(info, "channels", None),
    )


def edit_song_tags(path: Path, edits: dict[str, str | int | None]) -> tuple[dict, bool]:
    """Apply a partial tag edit to one library file, atomically.

    The original is copied to a hidden sibling, edited and re-read there,
    and only an edit that verifies (tags as requested, same codec/stream)
    replaces it via ``os.replace``. Fields not in ``edits`` and every
    other tag — ReplayGain, artwork, MusicBrainz IDs — are left as found.
    Returns ``(tags read back from disk, changed)``."""
    unknown = set(edits) - set(EDITABLE_TAGS)
    if unknown:
        raise ValueError(f"unsupported tag fields: {sorted(unknown)}")
    wanted = {name: (str(value or "").strip() if name in _TEXT_TAGS else value)
              for name, value in edits.items()}
    with _edit_lock:
        current = read_song_tags(path)
        # Skipping unchanged fields keeps e.g. a full "2019-05-03" date.
        pending = {name: value for name, value in wanted.items() if current[name] != value}
        if not pending:
            return current, False
        signature = _stream_signature(path)
        fd, tmp_name = tempfile.mkstemp(dir=path.parent, prefix=".zaytags-", suffix=path.suffix)
        os.close(fd)
        tmp = Path(tmp_name)
        try:
            shutil.copy2(path, tmp)
            _match_owner(path, tmp)
            adapter = _tag_adapter(tmp)
            for name, value in pending.items():
                adapter.write(name, value)
            adapter.save()
            final = read_song_tags(tmp)
            if final != {**current, **pending} or _stream_signature(tmp) != signature:
                raise TagEditError("edited copy did not verify")
            # A fresh mtime lets Navidrome's quick scan notice the change.
            now = time.time()
            os.utime(tmp, (now, now))
            with open(tmp, "rb") as handle:
                os.fsync(handle.fileno())
            os.replace(tmp, path)
            _fsync_dir(path.parent)
            return final, True
        except (UnsupportedTagFormat, InvalidTagValue, TagEditError):
            raise
        except Exception as exc:  # noqa: BLE001 — any failure leaves the original intact
            raise TagEditError(str(exc)) from exc
        finally:
            tmp.unlink(missing_ok=True)


def _match_owner(original: Path, copy: Path) -> None:
    stat = original.stat()
    try:
        os.chown(copy, stat.st_uid, stat.st_gid)
    except OSError:
        pass


def _fsync_dir(directory: Path) -> None:
    try:
        fd = os.open(directory, os.O_RDONLY)
    except OSError:
        return
    try:
        os.fsync(fd)
    except OSError:
        pass
    finally:
        os.close(fd)

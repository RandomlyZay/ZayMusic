"""Staging → /music move with canonical path layout (§9.5)."""

from __future__ import annotations

import errno
import os
import re
import shutil
from pathlib import Path

_INVALID = re.compile(r'[/\\:*?"<>|]')
_WS = re.compile(r"\s+")

UNKNOWN_ARTIST = "_Unknown Artist"
UNKNOWN_ALBUM = "_Unknown Album"


def sanitize_component(name: str, max_len: int = 120) -> str:
    """Strip path-hostile chars, collapse whitespace, trim dots/spaces, cap length."""
    if not name:
        return ""
    s = _INVALID.sub("", name)
    s = _WS.sub(" ", s).strip()
    s = s.strip(". ")
    if len(s) > max_len:
        s = s[:max_len].rstrip(". ")
    return s


def canonical_path(
    music_root: Path | str,
    *,
    artist: str,
    album: str,
    track: int | None,
    title: str,
    ext: str,
) -> Path:
    """{Artist}/{Album}/{NN} - {Title}.{ext}"""
    root = Path(music_root)
    a = sanitize_component(artist) or UNKNOWN_ARTIST
    al = sanitize_component(album) or UNKNOWN_ALBUM
    t = sanitize_component(title) or "Unknown Title"
    nn = f"{track:02d}" if track else "00"
    e = ext.lstrip(".") or "bin"
    return root / a / al / f"{nn} - {t}.{e}"


def move_atomic(src: Path, dest: Path) -> Path:
    """os.replace into place so Navidrome never scans a partial file (§9.5)."""
    dest.parent.mkdir(parents=True, exist_ok=True)
    try:
        os.replace(src, dest)
        return dest
    except OSError as exc:
        # Bind mounts can reject rename with EXDEV even when st_dev reports the
        # same underlying filesystem. Fall back only for that specific case.
        if exc.errno != errno.EXDEV:
            raise

    tmp = dest.with_name(dest.name + ".part")
    try:
        shutil.copy2(src, tmp)
        os.replace(tmp, dest)
    finally:
        tmp.unlink(missing_ok=True)
    src.unlink(missing_ok=True)
    return dest


def move_to_library(
    src: Path,
    music_root: Path | str,
    *,
    artist: str,
    album: str,
    track: int | None,
    title: str,
    ext: str,
    cover_path: Path | None = None,
) -> Path:
    dest = canonical_path(
        music_root, artist=artist, album=album, track=track, title=title, ext=ext
    )
    moved = move_atomic(src, dest)
    if cover_path and cover_path.is_file():
        cover = moved.parent / "cover.jpg"
        if not cover.exists():
            shutil.copy2(cover_path, cover)
    return moved

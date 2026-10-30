"""mover path sanitization tests (§15)."""

import errno
import os
from pathlib import Path

from zayfetch.mover import (
    UNKNOWN_ALBUM,
    UNKNOWN_ARTIST,
    canonical_path,
    sanitize_component,
    move_atomic,
)


def test_sanitize_strips_invalid():
    assert sanitize_component('AC/DC: "Back in" <Black>?') == "ACDC Back in Black"


def test_sanitize_collapses_ws_and_trims():
    assert sanitize_component("  Hello   World  ") == "Hello World"
    assert sanitize_component("...dots...") == "dots"


def test_sanitize_caps_length():
    s = "x" * 200
    assert len(sanitize_component(s)) == 120


def test_sanitize_empty():
    assert sanitize_component("") == ""
    assert sanitize_component(None or "") == ""


def test_canonical_path_layout(tmp_path: Path):
    p = canonical_path(
        tmp_path,
        artist="Radiohead",
        album="OK Computer",
        track=3,
        title="Lucky",
        ext="opus",
    )
    assert p == tmp_path / "Radiohead" / "OK Computer" / "03 - Lucky.opus"


def test_canonical_path_unknowns(tmp_path: Path):
    p = canonical_path(
        tmp_path, artist="", album="", track=None, title="", ext="m4a"
    )
    assert p.parent.name == UNKNOWN_ALBUM
    assert p.parent.parent.name == UNKNOWN_ARTIST
    assert p.name.startswith("00 - ")


def test_track_zero_pads(tmp_path: Path):
    p = canonical_path(
        tmp_path, artist="A", album="B", track=7, title="T", ext="opus"
    )
    assert p.name.startswith("07 - ")


def test_move_atomic(tmp_path: Path):
    src = tmp_path / "staging" / "x.opus"
    src.parent.mkdir()
    src.write_bytes(b"audio")
    dest = tmp_path / "music" / "A" / "B" / "01 - T.opus"
    mover_dest = move_atomic(src, dest)
    assert mover_dest.exists()
    assert mover_dest.read_bytes() == b"audio"
    assert not src.exists()


def test_move_atomic_falls_back_on_cross_device_rename(tmp_path: Path, monkeypatch):
    src = tmp_path / "staging" / "x.webm"
    src.parent.mkdir()
    src.write_bytes(b"audio")
    dest = tmp_path / "music" / "A" / "B" / "01 - T.webm"
    replace = os.replace
    calls = 0

    def replace_once_cross_device(source, target):
        nonlocal calls
        calls += 1
        if calls == 1:
            raise OSError(errno.EXDEV, "cross-device link")
        replace(source, target)

    monkeypatch.setattr("zayfetch.mover.os.replace", replace_once_cross_device)

    mover_dest = move_atomic(src, dest)

    assert mover_dest.read_bytes() == b"audio"
    assert not src.exists()
    assert not dest.with_name(dest.name + ".part").exists()
    assert calls == 2


def test_move_copies_external_cover_art_for_formats_without_embedded_art(tmp_path: Path):
    src = tmp_path / "staging" / "x.webm"
    src.parent.mkdir()
    src.write_bytes(b"audio")
    cover = tmp_path / "staging" / "cover.jpg"
    cover.write_bytes(b"image")

    from zayfetch.mover import move_to_library

    dest = move_to_library(
        src,
        tmp_path / "music",
        artist="Artist",
        album="Album",
        track=None,
        title="Track",
        ext="webm",
        cover_path=cover,
    )

    assert dest.exists()
    assert (dest.parent / "cover.jpg").read_bytes() == b"image"

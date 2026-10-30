from pathlib import Path

import pytest
from fastapi import HTTPException
from fastapi.testclient import TestClient

from zayfetch import main


def test_resolve_library_file_rejects_paths_outside_music_root(tmp_path: Path, monkeypatch):
    music_dir = tmp_path / "music"
    music_dir.mkdir()
    outside = tmp_path / "outside.ogg"
    outside.write_bytes(b"audio")
    monkeypatch.setattr(main, "MUSIC_DIR", music_dir)

    with pytest.raises(HTTPException) as exc:
        main._resolve_library_file("../outside.ogg")

    assert exc.value.status_code == 400
    assert outside.exists()


def test_resolve_library_file_rejects_symlinks_outside_music_root(tmp_path: Path, monkeypatch):
    music_dir = tmp_path / "music"
    music_dir.mkdir()
    outside = tmp_path / "outside.ogg"
    outside.write_bytes(b"audio")
    (music_dir / "linked.ogg").symlink_to(outside)
    monkeypatch.setattr(main, "MUSIC_DIR", music_dir)

    with pytest.raises(HTTPException) as exc:
        main._resolve_library_file("linked.ogg")

    assert exc.value.status_code == 400
    assert outside.exists()


def test_resolve_library_file_recovers_numeric_track_prefix(tmp_path: Path, monkeypatch):
    music_dir = tmp_path / "music"
    track = music_dir / "artist" / "album" / "00 - Song Title.ogg"
    track.parent.mkdir(parents=True)
    track.write_bytes(b"audio")
    monkeypatch.setattr(main, "MUSIC_DIR", music_dir)

    assert main._resolve_library_file("artist/album/Song Title.ogg") == track.resolve()


def test_resolve_library_file_recovers_stale_tag_track_number(tmp_path: Path, monkeypatch):
    """Peer files keep their original tags: on disk "00 - Change.flac" but
    Navidrome synthesizes "93 - Change.flac" from TRACKNUMBER=93. Stripping
    both sides still resolves the same recording."""
    music_dir = tmp_path / "music"
    track = music_dir / "J. Cole" / "4 Your Eyez Only" / "00 - Change.flac"
    track.parent.mkdir(parents=True)
    track.write_bytes(b"audio")
    monkeypatch.setattr(main, "MUSIC_DIR", music_dir)

    assert main._resolve_library_file("J. Cole/4 Your Eyez Only/93 - Change.flac") == track.resolve()


def test_resolve_library_file_rejects_ambiguous_numeric_prefixes(tmp_path: Path, monkeypatch):
    music_dir = tmp_path / "music"
    album = music_dir / "artist" / "album"
    album.mkdir(parents=True)
    (album / "00 - Song Title.ogg").write_bytes(b"audio one")
    (album / "01 - Song Title.ogg").write_bytes(b"audio two")
    monkeypatch.setattr(main, "MUSIC_DIR", music_dir)

    with pytest.raises(HTTPException) as exc:
        main._resolve_library_file("artist/album/Song Title.ogg")

    assert exc.value.status_code == 409


def test_delete_music_removes_file_and_requests_rescan(tmp_path: Path, monkeypatch):
    music_dir = tmp_path / "music"
    track = music_dir / "artist" / "track.ogg"
    track.parent.mkdir(parents=True)
    track.write_bytes(b"audio")
    monkeypatch.setattr(main, "MUSIC_DIR", music_dir)
    monkeypatch.setattr(main, "_navidrome_song_path", lambda _song_id: "artist/track.ogg")
    scans: list[bool] = []
    monkeypatch.setattr(main, "_start_navidrome_scan", lambda: scans.append(True))

    response = TestClient(main.app).delete(
        "/music/test-song-id", headers={"X-Zay-Token": main.TOKEN}
    )

    assert response.status_code == 200
    assert response.json() == {"ok": True}
    assert not track.exists()
    assert scans == [True]

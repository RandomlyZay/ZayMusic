"""POST /music/{song_id}/tags — manual tag edits on library files.

Every file here is generated silence in a temp dir; Navidrome lookups and
rescans are faked, so nothing touches a real library or server."""

import shutil
import subprocess
import wave
from pathlib import Path

import mutagen
import pytest
from fastapi import HTTPException
from fastapi.testclient import TestClient
from mutagen.id3 import APIC, TALB, TCON, TDRC, TIT2, TPE1, TRCK, TXXX
from mutagen.wave import WAVE

from zayfetch import main, tagger

HAS_FFMPEG = shutil.which("ffmpeg") is not None
COVER = b"\xff\xd8\xff\xe0fake-jpeg-cover"


def silent_wav(path: Path, seconds: float = 1.0) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    with wave.open(str(path), "wb") as out:
        out.setnchannels(2)
        out.setsampwidth(2)
        out.setframerate(44100)
        out.writeframes(b"\x00\x00" * 2 * int(44100 * seconds))
    return path


def wav_frames(path: Path) -> bytes:
    with wave.open(str(path), "rb") as audio:
        return audio.readframes(audio.getnframes())


def tagged_wav(path: Path) -> Path:
    silent_wav(path)
    audio = WAVE(str(path))
    audio.add_tags()
    audio.tags.add(TIT2(encoding=3, text=["Old Title"]))
    audio.tags.add(TPE1(encoding=3, text=["Old Artist"]))
    audio.tags.add(TALB(encoding=3, text=["Old Album"]))
    audio.tags.add(TRCK(encoding=3, text=["3/12"]))
    audio.tags.add(TDRC(encoding=3, text=["2019-05-03"]))
    audio.tags.add(TCON(encoding=3, text=["Soul"]))
    audio.tags.add(TXXX(encoding=3, desc="REPLAYGAIN_TRACK_GAIN", text=["-6.50 dB"]))
    audio.tags.add(APIC(encoding=3, mime="image/jpeg", type=3, desc="Cover", data=COVER))
    audio.save()
    return path


@pytest.fixture
def library(tmp_path: Path, monkeypatch):
    """A fake MUSIC_DIR whose one song id maps to whatever path a test sets."""
    music = tmp_path / "music"
    music.mkdir()
    state = {"path": "Old Artist/Old Album/03 - Old Title.wav", "lookups": 0, "scans": 0}

    def song_path(song_id: str) -> str:
        state["lookups"] += 1
        if song_id != "song-1":
            raise HTTPException(status_code=404, detail="Song not found")
        return state["path"]

    def scan() -> None:
        state["scans"] += 1

    monkeypatch.setattr(main, "MUSIC_DIR", music)
    monkeypatch.setattr(main, "_navidrome_song_path", song_path)
    monkeypatch.setattr(main, "_start_navidrome_scan", scan)
    monkeypatch.setattr(main, "TOKEN", "test-token")
    state["music"] = music
    state["track"] = tagged_wav(music / state["path"])
    return state


def post(edits, song_id: str = "song-1", **kwargs):
    return TestClient(main.app).post(
        f"/music/{song_id}/tags", json=edits, headers={"X-Zay-Token": "test-token"}, **kwargs
    )


def leftovers(music: Path) -> list[Path]:
    return [p for p in music.rglob(".zaytags-*")]


# --- endpoint semantics ------------------------------------------------------

def test_edits_tags_preserving_audio_and_unrelated_tags(library):
    track: Path = library["track"]
    frames = wav_frames(track)

    response = post({"title": "New Title", "artist": "New Artist", "album": "New Album",
                     "track": 5, "year": 2021, "genre": "Jazz"})

    assert response.status_code == 200
    assert response.json() == {
        "ok": True, "id": "song-1", "changed": True, "title": "New Title",
        "artist": "New Artist", "album": "New Album", "track": 5, "year": 2021,
        "genre": "Jazz",
    }
    assert library["scans"] == 1
    assert wav_frames(track) == frames
    tags = WAVE(str(track)).tags
    assert tags["TRCK"].text == ["5/12"]  # track total survives
    assert tags["TXXX:REPLAYGAIN_TRACK_GAIN"].text == ["-6.50 dB"]
    assert tags.getall("APIC")[0].data == COVER
    assert leftovers(library["music"]) == []


def test_partial_edit_leaves_omitted_fields_and_full_date(library):
    response = post({"title": "Only Title", "year": "2019"})

    assert response.status_code == 200
    body = response.json()
    assert (body["title"], body["artist"], body["album"]) == ("Only Title", "Old Artist", "Old Album")
    assert (body["track"], body["year"], body["genre"]) == (3, 2019, "Soul")
    # The year did not change, so the precise release date is kept.
    assert str(WAVE(str(library["track"])).tags["TDRC"].text[0]) == "2019-05-03"


def test_null_and_zero_numbers_clear_instead_of_writing_zero(library):
    response = post({"track": None, "year": 0})

    assert response.status_code == 200
    assert (response.json()["track"], response.json()["year"]) == (None, None)
    tags = WAVE(str(library["track"])).tags
    assert "TRCK" not in tags and "TDRC" not in tags


def test_blank_strings_clear_text_fields(library):
    response = post({"artist": "  ", "album": "", "genre": None})

    assert response.status_code == 200
    body = response.json()
    assert (body["artist"], body["album"], body["genre"]) == ("", "", "")
    assert body["title"] == "Old Title"
    tags = WAVE(str(library["track"])).tags
    assert not {"TPE1", "TALB", "TCON"} & set(tags.keys())


def test_unchanged_values_skip_the_write_and_the_scan(library):
    track: Path = library["track"]
    before = (track.read_bytes(), track.stat().st_mtime_ns)

    response = post({"title": " Old Title ", "track": "3", "genre": "Soul"})

    assert response.status_code == 200
    assert response.json()["changed"] is False
    assert (track.read_bytes(), track.stat().st_mtime_ns) == before
    assert library["scans"] == 0


def test_numeric_strings_and_unicode_are_accepted(library):
    response = post({"title": "Café — Ünïcødé 夜", "track": " 7 ", "year": "1999"})

    assert response.status_code == 200
    assert response.json()["title"] == "Café — Ünïcødé 夜"
    assert (response.json()["track"], response.json()["year"]) == (7, 1999)


@pytest.mark.parametrize("edits, message", [
    ({}, "No tag changes were provided"),
    ({"title": ""}, "Title cannot be empty"),
    ({"title": None}, "Title cannot be empty"),
    ({"title": 5}, "title must be text"),
    ({"artist": "bad\x00name"}, "artist contains control characters"),
    ({"album": "x" * 513}, "album is too long"),
    ({"track": "3a"}, "track must be a whole number"),
    ({"track": "3/12"}, "track must be a whole number"),
    ({"track": 3.5}, "track must be a whole number"),
    ({"track": True}, "track must be a whole number"),
    ({"track": -1}, "track must be between 1 and 999"),
    ({"year": 99999}, "year must be between 1000 and 9999"),
    ({"year": "１９９９"}, "year must be a whole number"),
    ({"path": "/etc/passwd"}, "path cannot be edited"),
    ({"title": "Fine", "albumArtist": "Nope"}, "albumArtist cannot be edited"),
])
def test_malformed_fields_are_rejected_before_any_lookup(library, edits, message):
    before = library["track"].read_bytes()

    response = post(edits)

    assert response.status_code == 422
    assert response.json() == {"detail": message}
    assert library["lookups"] == 0 and library["scans"] == 0
    assert library["track"].read_bytes() == before


def test_non_object_body_is_rejected(library):
    assert post(["title", "x"]).json() == {"detail": "Send the tag changes as a JSON object"}
    assert post(None).status_code == 422
    assert library["lookups"] == 0


@pytest.mark.parametrize("genre", ["17", "CR"])
def test_id3_genre_codes_are_rejected_untouched(library, genre):
    before = library["track"].read_bytes()

    response = post({"genre": genre})

    assert response.status_code == 422
    assert response.json() == {"detail": "This genre cannot be stored in this file type"}
    assert library["track"].read_bytes() == before
    assert leftovers(library["music"]) == [] and library["scans"] == 0


def test_requires_the_service_token(library):
    response = TestClient(main.app).post("/music/song-1/tags", json={"title": "X"})

    assert response.status_code == 401
    assert library["lookups"] == 0


def test_overlong_song_id_is_rejected(library):
    assert post({"title": "X"}, song_id="a" * 257).status_code == 400
    assert library["lookups"] == 0


# --- path safety and missing songs ---------------------------------------------

def test_missing_song_returns_404(library):
    response = post({"title": "X"}, song_id="nope")

    assert response.status_code == 404
    assert library["scans"] == 0


def test_real_lookup_maps_subsonic_not_found_to_404(library, monkeypatch):
    monkeypatch.undo()
    monkeypatch.setattr(main, "TOKEN", "test-token")
    monkeypatch.setattr(main, "_start_navidrome_scan", lambda: pytest.fail("no scan"))

    class NotFound:
        def raise_for_status(self):
            return None

        def json(self):
            return {"subsonic-response": {"status": "failed",
                                          "error": {"code": 70, "message": "not found"}}}

    monkeypatch.setattr(main.httpx, "get", lambda *a, **k: NotFound())

    response = post({"title": "X"})

    assert response.status_code == 404
    assert response.json() == {"detail": "Song not found"}


def test_missing_file_on_disk_returns_404(library):
    library["path"] = "Old Artist/Old Album/99 - Gone.wav"

    assert post({"title": "X"}).status_code == 404
    assert library["scans"] == 0


def test_traversal_outside_music_dir_is_refused(library):
    outside = tagged_wav(library["music"].parent / "outside.wav")
    before = outside.read_bytes()
    library["path"] = "../outside.wav"

    response = post({"title": "Hijacked"})

    assert response.status_code == 400
    assert outside.read_bytes() == before
    assert library["scans"] == 0


def test_absolute_path_outside_music_dir_is_refused(library):
    outside = tagged_wav(library["music"].parent / "abs.wav")
    library["path"] = str(outside)

    assert post({"title": "Hijacked"}).status_code == 400
    assert WAVE(str(outside)).tags["TIT2"].text == ["Old Title"]


def test_symlink_escaping_music_dir_is_refused(library):
    outside = tagged_wav(library["music"].parent / "linked-target.wav")
    (library["music"] / "linked.wav").symlink_to(outside)
    library["path"] = "linked.wav"

    assert post({"title": "Hijacked"}).status_code == 400
    assert WAVE(str(outside)).tags["TIT2"].text == ["Old Title"]


def test_unsupported_format_returns_415_untouched(library):
    odd = library["music"] / "Old Artist" / "Old Album" / "04 - Odd.wma"
    odd.write_bytes(b"not really audio" * 64)
    library["path"] = "Old Artist/Old Album/04 - Odd.wma"

    response = post({"title": "X"})

    assert response.status_code == 415
    assert odd.read_bytes() == b"not really audio" * 64
    assert leftovers(library["music"]) == [] and library["scans"] == 0


# --- atomicity -----------------------------------------------------------------

def test_failed_save_leaves_original_and_no_temp_file(library, monkeypatch):
    track: Path = library["track"]
    before = track.read_bytes()

    def broken_save(self):
        # Corrupt the working copy mid-write, then fail.
        Path(self.audio.filename).write_bytes(b"half-written")
        raise OSError("disk full")

    monkeypatch.setattr(tagger._ID3Tags, "save", broken_save)

    response = post({"title": "Never Written"})

    assert response.status_code == 500
    assert response.json() == {"detail": "Could not update song tags"}
    assert track.read_bytes() == before
    assert leftovers(library["music"]) == [] and library["scans"] == 0


def test_copy_that_does_not_verify_is_discarded(library, monkeypatch):
    track: Path = library["track"]
    before = track.read_bytes()
    real_write = tagger._ID3Tags.write

    def wrong_write(self, name, value):
        real_write(self, name, "Something Else" if name == "title" else value)

    monkeypatch.setattr(tagger._ID3Tags, "write", wrong_write)

    assert post({"title": "Requested"}).status_code == 500
    assert track.read_bytes() == before
    assert leftovers(library["music"]) == []


def test_failed_replace_keeps_original(library, monkeypatch):
    track: Path = library["track"]
    before = track.read_bytes()

    def no_replace(src, dst):
        raise PermissionError("read-only library")

    monkeypatch.setattr(tagger.os, "replace", no_replace)

    assert post({"title": "Requested"}).status_code == 500
    assert track.read_bytes() == before
    assert leftovers(library["music"]) == []


def test_edit_keeps_file_mode(library):
    track: Path = library["track"]
    track.chmod(0o644)

    assert post({"title": "Mode Kept"}).status_code == 200
    assert track.stat().st_mode & 0o777 == 0o644


# --- other containers (ffmpeg-generated silence) -------------------------------

FORMATS = {
    "flac": ["-c:a", "flac"],
    "ogg": ["-c:a", "libvorbis"],
    "opus": ["-c:a", "libopus"],
    "mp3": ["-c:a", "libmp3lame", "-id3v2_version", "3"],
    "m4a": ["-c:a", "aac"],
}


def ffmpeg_silence(path: Path, ext: str) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        ["ffmpeg", "-nostdin", "-loglevel", "error", "-f", "lavfi",
         "-i", "anullsrc=r=48000:cl=stereo", "-t", "1", *FORMATS[ext], "-y", str(path)],
        check=True,
    )
    return path


def seed_tags(path: Path) -> None:
    """Initial tags, a track total, ReplayGain and (where simple) artwork."""
    audio = mutagen.File(str(path))
    if audio.tags is None:
        audio.add_tags()
    tags = audio.tags
    if path.suffix == ".m4a":
        from mutagen.mp4 import MP4Cover, MP4FreeForm

        tags.update({"\xa9nam": ["Old Title"], "\xa9ART": ["Old Artist"],
                     "\xa9alb": ["Old Album"], "\xa9day": ["2019-05-03T07:00:00Z"],
                     "\xa9gen": ["Soul"], "trkn": [(3, 12)],
                     "covr": [MP4Cover(COVER, imageformat=MP4Cover.FORMAT_JPEG)],
                     "----:com.apple.iTunes:replaygain_track_gain": [MP4FreeForm(b"-6.50 dB")]})
    elif path.suffix == ".mp3":
        for frame in (TIT2(encoding=3, text=["Old Title"]), TPE1(encoding=3, text=["Old Artist"]),
                      TALB(encoding=3, text=["Old Album"]), TRCK(encoding=3, text=["3/12"]),
                      TDRC(encoding=3, text=["2019-05-03"]), TCON(encoding=3, text=["Soul"]),
                      TXXX(encoding=3, desc="REPLAYGAIN_TRACK_GAIN", text=["-6.50 dB"]),
                      APIC(encoding=3, mime="image/jpeg", type=3, desc="Cover", data=COVER)):
            tags.add(frame)
    else:
        tags.update({"title": ["Old Title"], "artist": ["Old Artist"], "album": ["Old Album"],
                     "tracknumber": ["3/12"], "date": ["2019-05-03"], "genre": ["Soul"],
                     "replaygain_track_gain": ["-6.50 dB"]})
        if path.suffix == ".flac":
            audio.add_picture(tagger._flac_pic(COVER))
    if path.suffix == ".mp3":
        audio.save(v2_version=3)
    else:
        audio.save()


def replaygain(path: Path) -> str:
    audio = mutagen.File(str(path))
    tags = audio.tags
    if path.suffix == ".m4a":
        return bytes(tags["----:com.apple.iTunes:replaygain_track_gain"][0]).decode()
    if path.suffix == ".mp3":
        return tags["TXXX:REPLAYGAIN_TRACK_GAIN"].text[0]
    return tags["replaygain_track_gain"][0]


@pytest.mark.skipif(not HAS_FFMPEG, reason="ffmpeg generates the non-WAV fixtures")
@pytest.mark.parametrize("ext", sorted(FORMATS))
def test_roundtrip_each_supported_container(tmp_path: Path, ext: str):
    path = ffmpeg_silence(tmp_path / f"song.{ext}", ext)
    seed_tags(path)
    stream = tagger._stream_signature(path)
    assert tagger.read_song_tags(path) == {
        "title": "Old Title", "artist": "Old Artist", "album": "Old Album",
        "track": 3, "year": 2019, "genre": "Soul",
    }

    final, changed = tagger.edit_song_tags(path, {
        "title": "Nouveau Titre", "artist": "", "album": "New Album",
        "track": 4, "year": 2022, "genre": "Jazz",
    })

    assert changed is True
    expected = {"title": "Nouveau Titre", "artist": "", "album": "New Album",
                "track": 4, "year": 2022, "genre": "Jazz"}
    assert final == expected
    assert tagger.read_song_tags(path) == expected
    assert tagger._stream_signature(path) == stream
    assert replaygain(path) == "-6.50 dB"
    audio = mutagen.File(str(path))
    if ext == "flac":
        assert audio.pictures[0].data == COVER
        assert audio.tags["tracknumber"] == ["4/12"]
    elif ext == "m4a":
        assert bytes(audio.tags["covr"][0]) == COVER
        assert audio.tags["trkn"] == [(4, 12)]
    elif ext == "mp3":
        assert audio.tags.version[:2] == (2, 3)
        assert audio.tags.getall("APIC")[0].data == COVER
        assert audio.tags["TRCK"].text == ["4/12"]
    else:
        assert audio.tags["tracknumber"] == ["4/12"]
    assert list(tmp_path.glob(".zaytags-*")) == []


@pytest.mark.skipif(not HAS_FFMPEG, reason="ffmpeg generates the non-WAV fixtures")
@pytest.mark.parametrize("ext", sorted(FORMATS))
def test_clearing_numbers_in_each_container(tmp_path: Path, ext: str):
    path = ffmpeg_silence(tmp_path / f"song.{ext}", ext)
    seed_tags(path)

    final, _ = tagger.edit_song_tags(path, {"track": None, "year": None, "genre": ""})

    assert (final["track"], final["year"], final["genre"]) == (None, None, "")
    assert final["title"] == "Old Title"
    assert replaygain(path) == "-6.50 dB"


def test_untagged_wav_gains_a_tag(tmp_path: Path):
    path = silent_wav(tmp_path / "bare.wav")
    frames = wav_frames(path)

    final, changed = tagger.edit_song_tags(path, {"title": "Fresh", "track": 1})

    assert changed and final["title"] == "Fresh" and final["track"] == 1
    assert wav_frames(path) == frames


def test_rejects_unknown_fields_in_direct_calls(tmp_path: Path):
    path = silent_wav(tmp_path / "bare.wav")

    with pytest.raises(ValueError):
        tagger.edit_song_tags(path, {"comment": "nope"})

"""MusicBrainz repair: clean titles, recover artist+album, never invent data."""

import types

import pytest

from zayfetch import musicbrainz_repair as repair


@pytest.fixture(autouse=True)
def no_sleep(monkeypatch):
    import time

    monkeypatch.setattr(time, "sleep", lambda *a: None)


def _mb_response(hits):
    return {"recording-list": hits}


def _hit(title, artist, album="", score="100"):
    rec = {
        "title": title,
        "ext:score": score,
        "artist-credit": [{"name": artist}],
        "release-list": [{"title": album}] if album else [],
    }
    return rec


def test_clean_title_strips_upload_decorations():
    assert repair.clean_title("J.Cole Change Official Audio") == "J.Cole Change"
    assert repair.clean_title("Hotline Bling (Lyrics)") == "Hotline Bling"
    assert repair.clean_title("Drake - Hotline Bling (Official Video)") == "Drake - Hotline Bling"
    # performance variants are meaningful — keep them
    assert repair.clean_title("Song (Live)") == "Song (Live)"
    assert repair.clean_title("Song (Remix)") == "Song (Remix)"
    assert repair.clean_title("Song (Slowed + Reverb)") == "Song (Slowed + Reverb)"


def test_repair_random_uploader_row(monkeypatch):
    """The Kal row: title names the artist, channel is useless."""
    calls = []

    def fake_search(**kwargs):
        calls.append(kwargs)
        if kwargs.get("artist") == "Kal":
            return _mb_response([_hit("J.Cole", "EEST", "Addendum vol.1")])
        if kwargs.get("artist", "").lower() in ("j.cole", "j cole"):
            return _mb_response([_hit("Change", "J. Cole", "4 Your Eyez Only")])
        return _mb_response([])

    import musicbrainzngs

    monkeypatch.setattr(musicbrainzngs, "search_recordings", fake_search)
    fixed = repair.repair_metadata(title="J.Cole Change Official Audio", artist="Kal")
    assert fixed.get("artist") == "J. Cole"
    assert fixed.get("title") == "Change"
    assert fixed.get("album") == "4 Your Eyez Only"


def test_repair_leaves_good_data_alone(monkeypatch):
    def fake_search(**kwargs):
        return _mb_response([_hit("Hotline Bling", "Drake", "Views")])

    import musicbrainzngs

    monkeypatch.setattr(musicbrainzngs, "search_recordings", fake_search)
    fixed = repair.repair_metadata(title="Hotline Bling", artist="Drake", album="Views")
    assert fixed == {}


def test_repair_rejects_unrelated_same_title(monkeypatch):
    """Same-titled song by someone else must not steal the tags."""

    def fake_search(**kwargs):
        return _mb_response([_hit("Change", "Taylor Swift", "Fearless")])

    import musicbrainzngs

    monkeypatch.setattr(musicbrainzngs, "search_recordings", fake_search)
    fixed = repair.repair_metadata(title="Change", artist="SomeChannel")
    assert fixed == {}


def test_repair_never_raises(monkeypatch):
    import musicbrainzngs

    def boom(**kwargs):
        raise RuntimeError("mb is down")

    monkeypatch.setattr(musicbrainzngs, "search_recordings", boom)
    assert repair.repair_metadata(title="X", artist="Y") == {}

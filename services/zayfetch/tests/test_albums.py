"""Album search expansion: album queries must surface playable tracks."""

import json
import types

from zayfetch import main
from zayfetch.models import Candidate


def test_album_candidates_skips_album_not_named_by_query(monkeypatch):
    """A song query must not expand a karaoke album that merely contains
    the song title — its boosted tracks would bury the exact song."""
    monkeypatch.setattr(
        main,
        "_find_albums",
        lambda q, limit=5: ([
            {"browseId": "MPREb_kar", "title": "Passionfruit (Karaoke Version)", "artist": "Release"},
        ], ""),
    )
    called = []
    monkeypatch.setattr(main, "_expand_album", lambda a: called.append(a) or [])
    assert main._album_candidates("Drake Passionfruit") == []
    assert called == []


def test_album_candidates_skips_wrong_artist_same_title(monkeypatch):
    """A knockoff album with a matching title but a different artist
    (e.g. "4 your Your Eyez Only" by Sadmethod) must not expand."""
    monkeypatch.setattr(
        main,
        "_find_albums",
        lambda q, limit=5: ([
            {"browseId": "MPREb_knock", "title": "4 your Your Eyez Only", "artist": "Sadmethod"},
        ], ""),
    )
    called = []
    monkeypatch.setattr(main, "_expand_album", lambda a: called.append(a) or [])
    assert main._album_candidates("J Cole 4 Your Eyez Only") == []
    assert called == []


def test_album_candidates_gates_on_weak_match(monkeypatch):
    monkeypatch.setattr(
        main,
        "_find_albums",
        lambda q, limit=5: ([
            {"browseId": "MPREb_xxx", "title": "Unrelated Record", "artist": "Someone Else"},
        ], ""),
    )
    called = []
    monkeypatch.setattr(main, "_expand_album", lambda a: called.append(a) or [])
    assert main._album_candidates("Drake Views") == []
    assert called == []


def test_album_candidates_expands_strong_match(monkeypatch):
    album = {"browseId": "MPREb_g9n", "title": "Views", "artist": "Drake"}
    monkeypatch.setattr(main, "_find_albums", lambda q, limit=5: ([album], "Drake"))
    fake = [Candidate(videoId="v1", source="ytmusic", title="9", channel="Drake", album="Views")]
    monkeypatch.setattr(main, "_expand_album", lambda a: fake)
    assert main._album_candidates("Drake Views") == fake


def _no_cache(monkeypatch):
    monkeypatch.setattr(main, "_album_cache", {})


def test_expand_album_parses_playlist(monkeypatch):
    _no_cache(monkeypatch)

    class FakeYT:
        def get_album(self, browseId):
            assert browseId == "MPREb_g9n"
            return {
                "audioPlaylistId": "OLAK_fake",
                "title": "Views",
                "artists": [{"name": "Drake"}],
                "year": 2016,
                "isExplicit": True,
                "thumbnails": [],
                "tracks": [
                    {"title": "Keep The Family Close", "isExplicit": True, "isAvailable": True},
                    {"title": "9", "isExplicit": False, "isAvailable": True},
                ],
            }

    entries = [
        {"id": "abc123", "title": "Keep The Family Close", "channel": "Drake",
         "duration": 321, "thumbnails": []},
        {"id": None, "url": "https://www.youtube.com/watch?v=def456",
         "title": "9", "uploader": "Drake", "duration": 287, "thumbnails": []},
        {"title": "NoId", "channel": "Drake", "duration": 200, "thumbnails": []},
    ]
    fake_out = types.SimpleNamespace(
        returncode=0,
        stdout=json.dumps({"entries": entries}),
        stderr="",
    )
    monkeypatch.setattr("ytmusicapi.YTMusic", FakeYT)
    monkeypatch.setattr("subprocess.run", lambda *a, **k: fake_out)
    monkeypatch.setattr(main.shutil, "which", lambda _: "/usr/bin/yt-dlp")

    cands = main._expand_album({"browseId": "MPREb_g9n", "title": "Views", "artist": "Drake"})
    assert len(cands) == 2
    assert all(c.source == "ytmusic" and c.album == "Views" for c in cands)
    assert cands[0].videoId == "abc123"
    assert cands[0].durationSec == 321.0
    assert cands[1].videoId == "def456"
    assert [c.track for c in cands] == [1, 2]
    # per-track explicit from YTM wins over the album flag
    assert cands[0].explicit is True
    assert cands[1].explicit is False


def test_expand_album_preserves_positions_when_entries_are_missing(monkeypatch):
    _no_cache(monkeypatch)
    monkeypatch.setattr(main.discovery, "ytmusic_call", lambda *a, **k: {
        "audioPlaylistId": "OLAK_fake", "title": "Views", "artists": [],
    })
    entries = [None, {"id": "second", "title": "Second"},
               {"id": "fourth", "title": "Fourth", "playlist_index": 4}]
    monkeypatch.setattr(main.subprocess, "run", lambda *a, **k: types.SimpleNamespace(
        returncode=0, stdout=json.dumps({"entries": entries}), stderr=""))
    monkeypatch.setattr(main.shutil, "which", lambda _: "/usr/bin/yt-dlp")
    cands = main._expand_album({"browseId": "MPREb_test"})
    assert [c.track for c in cands] == [2, 4]


def test_expand_album_keeps_playlist_ids(monkeypatch):
    """Expansion stays fast: playlist extraction only, no per-track probing.

    Premium-first: the connected account plays gated IDs directly, so the
    album view returns YTM's own track list untouched."""
    _no_cache(monkeypatch)

    class FakeYT:
        def get_album(self, browseId):
            return {
                "audioPlaylistId": "OLAK_fake",
                "title": "Views",
                "artists": [{"name": "Drake"}],
                "year": 2016,
                "isExplicit": False,
                "thumbnails": [],
                "tracks": [
                    {"title": "9", "isExplicit": False, "isAvailable": False},
                ],
            }

    entries = [{"id": "gated1", "title": "9", "channel": "Drake", "duration": 287, "thumbnails": []}]
    fake_out = types.SimpleNamespace(
        returncode=0, stdout=json.dumps({"entries": entries}), stderr="",
    )
    monkeypatch.setattr("ytmusicapi.YTMusic", FakeYT)
    monkeypatch.setattr("subprocess.run", lambda *a, **k: fake_out)
    monkeypatch.setattr(main.shutil, "which", lambda _: "/usr/bin/yt-dlp")

    cands = main._expand_album({"browseId": "MPREb_g9n", "title": "Views", "artist": "Drake"})
    assert len(cands) == 1
    assert cands[0].videoId == "gated1"


def test_expand_album_uses_cache(monkeypatch):
    _no_cache(monkeypatch)
    calls = []

    class FakeYT:
        def get_album(self, browseId):
            calls.append(browseId)
            return {"audioPlaylistId": None, "title": "Views", "artists": []}

    monkeypatch.setattr("ytmusicapi.YTMusic", FakeYT)
    assert main._expand_album({"browseId": "MPREb_c", "title": "Views", "artist": "Drake"}) == []
    # prime the cache manually, then get_album must not run again
    cached = [Candidate(videoId="v9", source="ytmusic", title="9", channel="Drake", album="Views")]
    main._album_cache["MPREb_c"] = (main.time.monotonic() + 3600, cached)
    assert main._expand_album({"browseId": "MPREb_c"})[0].videoId == "v9"
    assert calls == ["MPREb_c"]


def test_expand_album_no_playlist(monkeypatch):
    _no_cache(monkeypatch)

    class FakeYT:
        def get_album(self, browseId):
            return {"title": "Views", "artists": [], "thumbnails": []}

    monkeypatch.setattr("ytmusicapi.YTMusic", FakeYT)
    assert main._expand_album({"browseId": "MPREb_x"}) == []


def test_search_response_includes_albums(monkeypatch):
    from fastapi.testclient import TestClient

    monkeypatch.setattr(main, "TOKEN", "")
    album = {"browseId": "MPREb_g9n", "title": "Views", "artist": "Drake",
             "year": 2016, "explicit": True, "thumbnails": []}
    monkeypatch.setattr(main, "_music_search", lambda q, count: ([], [album], "Drake"))
    monkeypatch.setattr(main, "_search_yt_dlp", lambda q, count: [])
    # hermetic: suggestions must not hit the live network here
    from zayfetch import discovery
    monkeypatch.setattr(discovery, "ytm_suggestions", lambda q, limit=8: [])
    client = TestClient(main.app)
    body = client.get("/search", params={"q": "Drake Views", "limit": 5}).json()
    assert body["albums"][0]["title"] == "Views"
    assert body["albums"][0]["browseId"] == "MPREb_g9n"


def test_album_endpoint_returns_track_order(monkeypatch):
    from fastapi.testclient import TestClient

    class FakeYT:
        def get_album(self, browseId):
            return {
                "title": "Views",
                "artists": [{"name": "Drake"}],
                "year": 2016,
                "isExplicit": False,
                "thumbnails": [],
            }

    monkeypatch.setattr(main, "TOKEN", "")
    monkeypatch.setattr("ytmusicapi.YTMusic", FakeYT)
    ordered = [
        Candidate(videoId="v1", source="ytmusic", title="Keep The Family Close",
                  channel="Drake", album="Views"),
        Candidate(videoId="v2", source="ytmusic", title="9",
                  channel="Drake", album="Views"),
    ]
    monkeypatch.setattr(main, "_expand_album", lambda a: ordered)
    client = TestClient(main.app)
    body = client.get("/album", params={"browseId": "MPREb_g9n"}).json()
    assert body["album"]["title"] == "Views"
    # playlist order preserved, not score-sorted
    assert [c["videoId"] for c in body["candidates"]] == ["v1", "v2"]


def test_album_endpoint_rejects_bad_id(monkeypatch):
    from fastapi.testclient import TestClient

    monkeypatch.setattr(main, "TOKEN", "")
    client = TestClient(main.app)
    assert client.get("/album", params={"browseId": "!!!"}).status_code in (400, 422)


def test_resolve_playable_url_uses_primary(monkeypatch):
    monkeypatch.setattr(main, "_extract_preview_url", lambda vid: f"https://x/{vid}")
    assert main._resolve_playable_url("abc12345678") == "https://x/abc12345678"


def test_resolve_playable_url_asks_for_sign_in_without_cookies(monkeypatch, tmp_path):
    from fastapi import HTTPException

    import pytest

    monkeypatch.setenv("DATA_DIR", str(tmp_path))
    monkeypatch.delenv("YTDLP_COOKIES_FILE", raising=False)
    monkeypatch.setattr(
        main, "_extract_preview_url", lambda vid: (_ for _ in ()).throw(HTTPException(status_code=502, detail="nope"))
    )
    with pytest.raises(HTTPException) as exc:
        main._resolve_playable_url("gated1234567", "Drake", "9")
    assert exc.value.status_code == 412
    assert "YouTube Music Premium" in str(exc.value.detail)


def test_resolve_playable_url_passes_through_with_cookies(monkeypatch, tmp_path):
    from fastapi import HTTPException

    import pytest

    cookies = tmp_path / "youtube.txt"
    cookies.write_text("# Netscape HTTP Cookie File\n.youtube.com\tTRUE\t/\tTRUE\t0\tSID\tx\n")
    monkeypatch.setenv("YTDLP_COOKIES_FILE", str(cookies))
    monkeypatch.setattr(
        main, "_extract_preview_url", lambda vid: (_ for _ in ()).throw(HTTPException(status_code=502, detail="nope"))
    )
    with pytest.raises(HTTPException) as exc:
        main._resolve_playable_url("gated1234567", "Drake", "9")
    assert exc.value.status_code == 502

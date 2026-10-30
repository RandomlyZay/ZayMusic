"""Deezer canonical metadata: parsing, cache/backoff, manual repair lookup.

Deezer is not part of search any more — it backs ``GET /metadata/lookup``
(manual tag repair) and nothing else, so these tests cover that path plus
the iTunes-based search wiring that replaced the search-time catalog.
"""

import pytest

from zayfetch import catalog, main
from zayfetch.catalog import (
    CatalogResult,
    CatalogTrack,
    album_detail,
    lookup,
)
from zayfetch.models import Candidate


def ct(**kw):
    base = dict(id=1, title="Hotline Bling", artist="Drake", album="Views",
                album_id=13082992, duration=267.0, rank=897048,
                explicit=False, isrc="USCM51500238", cover="https://cover/x.jpg")
    base.update(kw)
    return CatalogTrack(**base)


def mk(vid, title, channel, dur=200.0, source="youtube", album=None, explicit=None):
    return Candidate(videoId=vid, source=source, title=title, channel=channel,
                     album=album, durationSec=dur, explicit=explicit)


@pytest.fixture(autouse=True)
def clean():
    catalog.clear_cache()
    yield
    catalog.clear_cache()


# --- parsing ---------------------------------------------------------------

def test_parse_track_captures_authority_fields(monkeypatch):
    body = {"data": [{
        "id": 124603286, "title": "Hotline Bling", "duration": 267, "rank": 897048,
        "explicit_lyrics": False, "isrc": "USCM51500238",
        "artist": {"id": 246791, "name": "Drake"},
        "album": {"id": 13082992, "title": "Views", "cover_medium": "https://cover/m.jpg"},
    }], "total": 1}
    monkeypatch.setattr(catalog, "_get", lambda *a, **k: body)
    res = lookup("hotline bling")
    assert res is not None
    t = res.tracks[0]
    assert (t.title, t.artist, t.album) == ("Hotline Bling", "Drake", "Views")
    assert t.duration == 267.0 and t.rank == 897048
    assert t.explicit is False and t.isrc == "USCM51500238"
    assert t.cover == "https://cover/m.jpg" and t.album_id == 13082992


def test_parse_album_detail_ordered_tracklist(monkeypatch):
    monkeypatch.setattr(catalog, "_get", lambda *a, **k: None)
    body = {
        "id": 13082992, "title": "Views",
        "artist": {"name": "Drake"}, "release_date": "2016-05-06",
        "explicit_lyrics": True, "nb_tracks": 2, "cover_medium": "",
        "tracks": {"data": [
            {"id": 1, "title": "Keep The Family Close", "duration": 321, "rank": 1,
             "explicit_lyrics": True, "artist": {"name": "Drake"}},
            {"id": 2, "title": "9", "duration": 256, "rank": 2,
             "explicit_lyrics": False, "artist": {"name": "Drake"}},
        ]},
    }
    monkeypatch.setattr(catalog, "_get", lambda *a, **k: body)
    album = album_detail(13082992)
    assert album is not None
    assert album.year == 2016 and album.explicit is True
    assert [t.title for t in album.tracks] == ["Keep The Family Close", "9"]
    # album-scoped tracks inherit the album block
    assert album.tracks[0].album == "Views" and album.tracks[0].album_id == 13082992


# --- lookup: album intent ----------------------------------------------------

def _tracks_body(*rows):
    return {"data": [
        {"id": i, "title": title, "duration": dur, "rank": rank,
         "explicit_lyrics": False, "artist": {"name": artist},
         "album": {"id": aid, "title": album, "cover_medium": ""}}
        for i, (title, artist, album, aid, dur, rank) in enumerate(rows, start=1)
    ]}


def test_lookup_picks_album_on_intent(monkeypatch):
    search = _tracks_body(
        ("Views", "Drake", "Views", 13082992, 312, 454635),
        ("9", "Drake", "Views", 13082992, 256, 702673),
    )
    detail = {"id": 13082992, "title": "Views", "artist": {"name": "Drake"},
              "release_date": "2016-05-06", "explicit_lyrics": True,
              "cover_medium": "", "nb_tracks": 2,
              "tracks": {"data": [
                  {"id": 1, "title": "Keep The Family Close", "duration": 321,
                   "rank": 1, "explicit_lyrics": True, "artist": {"name": "Drake"}},
                  {"id": 2, "title": "9", "duration": 256,
                   "rank": 2, "explicit_lyrics": False, "artist": {"name": "Drake"}},
              ]}}

    def fake_get(path, params=None):
        return detail if path.startswith("/album/") else search

    monkeypatch.setattr(catalog, "_get", fake_get)
    res = lookup("Drake Views")
    assert res is not None and res.album is not None
    assert res.album.title == "Views" and res.album.id == 13082992
    assert [t.title for t in res.album.tracks] == ["Keep The Family Close", "9"]


def test_lookup_no_album_card_without_intent(monkeypatch):
    search = _tracks_body(
        ("Passionfruit", "Drake", "More Life", 111, 299, 873309),
        ("Passionfruit (Drake Cover)", "Prudence", "Passionfruit (Drake Cover)", 222, 171, 293192),
    )
    calls = []
    monkeypatch.setattr(catalog, "_get", lambda p, params=None: calls.append(p) or search)
    res = lookup("Drake Passionfruit")
    assert res is not None and res.album is None
    assert calls == ["/search"]  # no album detail fetch without intent


def test_lookup_never_raises(monkeypatch):
    import httpx

    def boom(*a, **k):
        raise httpx.ConnectError("no network")

    monkeypatch.setattr(catalog.httpx, "get", boom)
    assert lookup("hotline bling") is None
    assert album_detail(13082992) is None


# --- cache + backoff ---------------------------------------------------------

def test_search_results_cached(monkeypatch):
    calls = []
    body = _tracks_body(("Hotline Bling", "Drake", "Views", 13082992, 267, 9))

    def fake_get(path, params=None):
        calls.append(path)
        return body

    monkeypatch.setattr(catalog, "_get", fake_get)
    assert lookup("hotline bling") is not None
    assert lookup("hotline bling") is not None
    assert calls == ["/search"]


def test_throttle_sets_backoff_and_serves_stale(monkeypatch):
    body = _tracks_body(("Hotline Bling", "Drake", "Views", 13082992, 267, 9))
    monkeypatch.setattr(catalog, "_get", lambda *a, **k: body)
    assert lookup("hotline bling") is not None
    # Now every request throttles: no new network, stale cache still serves.
    monkeypatch.setattr(catalog, "_get", lambda *a, **k: None)
    catalog._note_throttled()
    assert lookup("hotline bling") is not None
    assert catalog._in_backoff()


def test_quota_error_envelope_triggers_backoff(monkeypatch):
    class FakeResp:
        status_code = 200

        def json(self):
            return {"error": {"type": "Exception", "message": "Quota limit exceeded", "code": 4}}

    monkeypatch.setattr(catalog.httpx, "get", lambda *a, **k: FakeResp())
    assert catalog._get("/search", {"q": "x"}) is None
    assert catalog._in_backoff()


# --- ranking integration (sets now come from the iTunes matcher) -------------

def test_verified_sets_beat_imposter_and_cover():
    from zayfetch.ranking import rank

    audio = mk("a", "Drake - Hotline Bling (Official Audio)", "Drake Media", dur=267.0)
    audio.views = 3_177_174
    video = mk("v", "Drake - Hotline Bling", "Drake", dur=296.0)
    video.views = 2_155_424_848
    imposter = mk("i", "Drake - Hotline Bling", "Drake", dur=155.0)
    imposter.views = 10_000
    cover = mk("c", "Hotline Bling", "William Singe", dur=240.0)
    cover.views = 24_000_000
    listed_cover = mk("m", "Hotline Bling", "Miro Mas", dur=190.0)
    listed_cover.views = 177_000
    cands = [cover, imposter, video, audio, listed_cover]
    full = {"a"}      # the requested recording (official audio upload)
    other = {"m"}     # same-titled recording by another artist
    loose = {"v"}     # artist's own channel, video-cut duration
    ranked = rank(cands, query="hotline bling", top_artist="Drake",
                  catalog_matches=full, catalog_other_matches=other,
                  catalog_loose_matches=loose, catalog_active=True)
    assert len(ranked) == 5  # sort-only: nothing removed
    assert [c.videoId for c in ranked[:2]] == ["a", "v"]
    assert ranked[2].videoId == "m"  # real recording, wrong song for the query
    assert {c.videoId for c in ranked[3:]} == {"c", "i"}


def test_damping_applies_only_to_unverified_when_active():
    from zayfetch.ranking import score

    cover = mk("c", "Hotline Bling", "William Singe", dur=240.0)
    cover.views = 24_000_000
    full = score(cover, "hotline bling")
    damped = score(cover, "hotline bling", catalog_active=True)
    assert damped < full  # popularity halved for unverified
    # verified recordings keep full popularity with or without the flag
    official = mk("a", "Hotline Bling", "Drake", dur=267.0)
    official.views = 3_177_174
    assert score(official, "hotline bling", catalog_match=True, catalog_active=True) == \
        score(official, "hotline bling", catalog_match=True)
    # no catalog opinion anywhere → existing behavior byte-for-byte
    assert score(cover, "hotline bling", catalog_active=False) == full


def test_rank_without_verification_unchanged():
    from zayfetch.ranking import rank, score

    c = mk("a", "Song", "Artist - Topic", source="ytmusic")
    assert score(c, "Song") == score(c, "Song", catalog_match=False)
    assert score(c, "Song", catalog_match=True) > score(c, "Song")
    assert len(rank([c, mk("b", "Song", "X")], query="Song")) == 2


# --- manual metadata repair --------------------------------------------------

def test_metadata_lookup_returns_deezer_candidates(monkeypatch):
    from fastapi.testclient import TestClient

    monkeypatch.setattr(main, "TOKEN", "")
    monkeypatch.setattr(catalog, "album_detail", lambda _id: None)
    monkeypatch.setattr(catalog, "lookup", lambda q, limit=25: CatalogResult(
        query=q,
        tracks=[ct(), ct(id=2, title="Hotline Bling (Remix)", artist="Drake")],
    ))
    body = TestClient(main.app).get(
        "/metadata/lookup", params={"artist": "Drake", "title": "Hotline Bling"}
    ).json()
    assert [c["title"] for c in body["candidates"]] == [
        "Hotline Bling", "Hotline Bling (Remix)",
    ]
    assert all(c["source"] == "deezer" for c in body["candidates"])
    assert body["candidates"][0]["isrc"] == "USCM51500238"
    assert body["candidates"][0]["album"] == "Views"


def test_metadata_lookup_dedupes_and_skips_incomplete(monkeypatch):
    from fastapi.testclient import TestClient

    monkeypatch.setattr(main, "TOKEN", "")
    monkeypatch.setattr(catalog, "album_detail", lambda _id: None)
    monkeypatch.setattr(catalog, "lookup", lambda q, limit=25: CatalogResult(
        query=q,
        tracks=[
            ct(),
            ct(id=2, title="hotline  bling", artist="DRAKE"),  # folded duplicate
            ct(id=3, title="", artist="Drake"),                # missing title
            ct(id=4, title="Other", artist=""),                # missing artist
        ],
    ))
    body = TestClient(main.app).get(
        "/metadata/lookup", params={"q": "hotline bling drake"}
    ).json()
    assert [c["title"] for c in body["candidates"]] == ["Hotline Bling"]


def test_metadata_lookup_adds_album_years(monkeypatch):
    from fastapi.testclient import TestClient

    from zayfetch.catalog import CatalogAlbum

    monkeypatch.setattr(main, "TOKEN", "")
    monkeypatch.setattr(catalog, "lookup", lambda q, limit=25: CatalogResult(
        query=q,
        tracks=[ct(), ct(id=2, title="Started", album="Other", album_id=7),
                ct(id=3, title="No Album", album="", album_id=None)],
    ))
    fetched: list[int] = []

    def fake_detail(album_id):
        fetched.append(album_id)
        return CatalogAlbum(id=album_id, title="x", year={13082992: 2016}.get(album_id))

    monkeypatch.setattr(catalog, "album_detail", fake_detail)
    body = TestClient(main.app).get("/metadata/lookup", params={"q": "drake"}).json()
    assert [(c["title"], c["year"]) for c in body["candidates"]] == [
        ("Hotline Bling", 2016), ("Started", None), ("No Album", None),
    ]
    assert sorted(fetched) == [7, 13082992]
    assert set(body["candidates"][0]) >= {
        "source", "title", "artist", "album", "year", "artwork", "durationSec",
    }


def test_album_years_caps_fetches_and_uses_cache(monkeypatch):
    from zayfetch.catalog import CatalogAlbum

    calls: list[int] = []

    def fake_get(path, params=None):
        album_id = int(path.rsplit("/", 1)[1])
        calls.append(album_id)
        return {"id": album_id, "title": "A", "release_date": f"{2000 + album_id}-01-01"}

    monkeypatch.setattr(catalog, "_get", fake_get)
    assert catalog.album_years([1, 2, 3, 4, 1], fetch_limit=3) == {1: 2001, 2: 2002, 3: 2003}
    assert sorted(calls) == [1, 2, 3]
    calls.clear()
    # Cached details are free and do not count against the fetch cap.
    assert catalog.album_years([1, 2, 3, 4], fetch_limit=1) == {
        1: 2001, 2: 2002, 3: 2003, 4: 2004,
    }
    assert calls == [4]
    assert isinstance(catalog._cache_get("album:4"), CatalogAlbum)


def test_album_years_skips_network_during_backoff(monkeypatch):
    monkeypatch.setattr(catalog, "_get", lambda *a, **k: pytest.fail("no fetch in backoff"))
    catalog._note_throttled()
    assert catalog.album_years([1, 2]) == {}


def test_metadata_lookup_empty_query_returns_nothing(monkeypatch):
    from fastapi.testclient import TestClient

    monkeypatch.setattr(main, "TOKEN", "")
    body = TestClient(main.app).get("/metadata/lookup").json()
    assert body["candidates"] == []


def test_metadata_lookup_total_on_outage(monkeypatch):
    from fastapi.testclient import TestClient

    monkeypatch.setattr(main, "TOKEN", "")

    def boom(*a, **k):
        raise RuntimeError("catalog down")

    monkeypatch.setattr(catalog, "lookup", boom)
    body = TestClient(main.app).get(
        "/metadata/lookup", params={"artist": "Drake", "title": "Hotline Bling"}
    ).json()
    assert body["candidates"] == []


def test_album_endpoint_rejects_garbage(monkeypatch):
    from fastapi.testclient import TestClient

    monkeypatch.setattr(main, "TOKEN", "")
    client = TestClient(main.app)
    assert client.get("/album", params={"browseId": "!!!"}).status_code in (400, 422)

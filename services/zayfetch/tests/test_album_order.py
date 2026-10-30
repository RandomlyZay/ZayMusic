import pytest

from zayfetch import album_order, main


@pytest.fixture
def lookup(monkeypatch):
    monkeypatch.setattr(album_order, "_cache", {})
    songs = [
        {"id": "a", "title": "Bitrate", "duration": 88},
        {"id": "b", "title": "Fusion Core", "duration": 153},
    ]
    album = {"name": "Forgotten Arcade", "artist": "Frizk", "song": songs}
    canonical = {"title": "Forgotten Arcade", "artists": [{"name": "Frizk"}], "tracks": [
        {"title": "Fusion Core", "duration": "2:34"},
        {"title": "Other song", "duration": "1:30"},
        {"title": "Bitrate", "duration": "1:29"},
    ]}
    calls = []

    def ytm(method, *args, **kwargs):
        calls.append(method)
        if method == "search":
            return [{"browseId": "release", **canonical}]
        return canonical

    monkeypatch.setattr(album_order, "_navidrome_get", lambda *a, **k: {"album": album})
    monkeypatch.setattr(album_order.discovery, "ytmusic_call", ytm)
    return album, canonical, calls


def test_recovers_original_numbers_for_partial_album_and_caches_lookup(lookup):
    album, _, calls = lookup
    assert album_order.recover("local") == {"a": 3, "b": 1}
    assert album_order.recover("local") == {"a": 3, "b": 1}
    assert calls == ["search", "get_album"]
    assert "track" not in album["song"][0]


def test_tagged_album_needs_no_network(lookup):
    album, _, calls = lookup
    for n, song in enumerate(album["song"], 1):
        song["track"] = n
    assert album_order.recover("local") == {}
    assert calls == []


@pytest.mark.parametrize("change", ["artist", "title", "duration", "conflict", "disc", "duplicate"])
def test_does_not_guess_order_for_wrong_or_ambiguous_release(lookup, change):
    album, canonical, _ = lookup
    if change == "artist":
        canonical["artists"] = [{"name": "Cover band"}]
    elif change == "title":
        canonical["title"] += " (Deluxe)"
    elif change == "duration":
        canonical["tracks"][0]["duration"] = "3:30"
    elif change == "conflict":
        album["song"][0]["track"] = 1
    elif change == "disc":
        album["song"][0]["discNumber"] = 2
    else:
        canonical["tracks"].append(canonical["tracks"][0].copy())
    assert album_order.recover("local") == {}


def test_metadata_outage_does_not_break_album(lookup, monkeypatch):
    def unavailable(*a, **k):
        raise RuntimeError("offline")
    monkeypatch.setattr(album_order.discovery, "ytmusic_call", unavailable)
    assert album_order.recover("local") == {}


def test_endpoint_returns_order(lookup, monkeypatch):
    from fastapi.testclient import TestClient
    monkeypatch.setattr(main, "TOKEN", "")
    response = TestClient(main.app).get("/library/albums/local/order")
    assert response.status_code == 200
    assert response.json() == {"tracks": {"a": 3, "b": 1}}

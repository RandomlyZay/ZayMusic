"""Home sections (top songs, Rediscover) and saved-album intents."""

from datetime import date, datetime, timedelta, timezone

import pytest

from zayfetch import library, store

NOW = datetime(2026, 10, 8, tzinfo=timezone.utc)


def iso(days_ago: float) -> str:
    return (NOW - timedelta(days=days_ago)).isoformat().replace("+00:00", "Z")


def song(i, plays=0, played=None, starred=False, created=400):
    return {"id": f"s{i}", "title": f"t{i}", "playCount": plays or None,
            "played": iso(played) if played is not None else None,
            "starred": iso(1) if starred else None, "created": iso(created)}


def test_top_songs_orders_by_play_count_then_recency():
    songs = [song(1, 2, 5), song(2, 5, 50), song(3, 2, 1), song(4)]
    assert [s["id"] for s in library.top_songs(songs)] == ["s2", "s3", "s1"]


def test_rediscover_picks_liked_or_replayed_songs_not_heard_lately():
    songs = [
        song(1, 3, 40), song(2, 2, 30), song(3, 0, None, starred=True, created=60),
        song(4, 1, 90, starred=True), song(5, 9, 2),            # heard recently
        song(6, 1, 90),                                          # played once, not liked
        song(7, 0, None, starred=True, created=3),               # liked yesterday
    ]
    picked = library.rediscover(songs, now=NOW, day=date(2026, 10, 8))
    assert sorted(s["id"] for s in picked) == ["s1", "s2", "s3", "s4"]


def test_rediscover_hidden_when_too_few_and_stable_per_day():
    assert library.rediscover([song(1, 3, 40), song(2, 3, 40)], now=NOW) == []
    many = [song(i, 3, 40) for i in range(30)]
    day = date(2026, 10, 8)
    first = library.rediscover(many, now=NOW, day=day)
    assert len(first) == library.REDISCOVER_SIZE
    assert first == library.rediscover(list(reversed(many)), now=NOW, day=day)
    assert first != library.rediscover(many, now=NOW, day=date(2026, 10, 9))


def test_rediscover_excludes_top_songs():
    many = [song(i, 3, 40) for i in range(6)]
    picked = library.rediscover(many, now=NOW, exclude={"s0", "s1"})
    assert {"s0", "s1"}.isdisjoint(s["id"] for s in picked)


@pytest.fixture
def db(tmp_path, monkeypatch):
    monkeypatch.setenv("DATA_DIR", str(tmp_path))
    monkeypatch.setattr(store, "_CONN", None)
    yield
    monkeypatch.setattr(store, "_CONN", None)


def fake_navidrome(monkeypatch, albums):
    starred = []

    def get(endpoint, **params):
        assert endpoint == "search3"
        return {"searchResult3": {"album": list(albums)}}

    def post(endpoint, data=None, **params):
        assert endpoint == "star"
        starred.append(params["albumId"])
        return {}

    monkeypatch.setattr(library, "_navidrome_get", get)
    monkeypatch.setattr(library, "_navidrome_post", post)
    return starred


def test_album_save_waits_for_index_then_stars(db, monkeypatch):
    albums = []
    starred = fake_navidrome(monkeypatch, albums)
    row = library.save_album("Currents", "Tame Impala")
    assert row["album_id"] is None and starred == []
    library.reconcile_album_saves()
    assert starred == []
    albums.append({"id": "al1", "name": "Currents", "artist": "Tame Impala"})
    library.reconcile_album_saves()
    assert starred == ["al1"]
    assert store.pending_album_saves() == []


def test_album_save_matches_title_and_artist(db, monkeypatch):
    starred = fake_navidrome(monkeypatch, [
        {"id": "a", "name": "Greatest Hits", "artist": "Queen"},
        {"id": "b", "name": "Greatest Hits", "artist": "ABBA"},
        {"id": "c", "name": "Greatest Hits (Deluxe)", "artist": "ABBA"},
    ])
    assert library.save_album("Greatest Hits", "ABBA")["album_id"] == "b"
    assert starred == ["b"]
    # Ambiguous title and no artist match: never star the wrong album.
    assert library.save_album("Greatest Hits", "Nobody")["album_id"] is None


def test_stale_album_save_is_dropped(db, monkeypatch):
    fake_navidrome(monkeypatch, [])
    library.save_album("Lost", "Someone")
    conn = store.get_conn()
    conn.execute("UPDATE album_saves SET created_at = ?", ((NOW - timedelta(days=30)).isoformat(),))
    conn.commit()
    library.reconcile_album_saves()
    assert store.pending_album_saves() == []

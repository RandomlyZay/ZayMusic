"""Discovery tests — everything total, never raises, never blocks."""

from zayfetch import discovery


def test_lastfm_without_key_returns_empty(monkeypatch):
    monkeypatch.setenv("LASTFM_API_KEY", "")
    assert discovery.lastfm_similar("Drake", "Hotline Bling") == []
    assert discovery.lastfm_correction("Drake", "Hotline Bling") is None
    assert discovery.lastfm_artist_top("Drake") == []


def test_ytm_suggestions_total_on_failure(monkeypatch):
    import ytmusicapi

    def boom(self, q):
        raise RuntimeError("offline")

    monkeypatch.setattr(ytmusicapi.YTMusic, "get_search_suggestions", boom)
    assert discovery.ytm_suggestions("drake") == []


def test_ytm_radio_rejects_bad_id():
    assert discovery.ytm_radio("not-an-id") == []
    assert discovery.ytm_radio("") == []


def test_browser_auth_includes_origin_for_sapisid_hash(monkeypatch):
    """Without an origin header ytmusicapi hashes "<sapisid> None" and every
    account-scoped call (library, playlists) is served as signed out."""
    from zayfetch import downloader

    monkeypatch.setattr(downloader, "read_cookies_dict", lambda: {
        "__Secure-3PAPISID": "sapisid-value",
        "VISITOR_INFO1_LIVE": "visitor",
    })
    auth = discovery._browser_auth()
    assert auth is not None
    assert auth["origin"] == "https://music.youtube.com"
    assert auth["x-origin"] == "https://music.youtube.com"
    assert auth["x-goog-visitor-id"] == "visitor"
    assert auth["cookie"].startswith("__Secure-3PAPISID=")


def test_browser_auth_none_without_sapisid(monkeypatch):
    from zayfetch import downloader

    monkeypatch.setattr(downloader, "read_cookies_dict", lambda: {"VISITOR_INFO1_LIVE": "v"})
    assert discovery._browser_auth() is None


def test_ytm_radio_total_on_failure(monkeypatch):
    import ytmusicapi

    def boom(self, **k):
        raise RuntimeError("offline")

    monkeypatch.setattr(ytmusicapi.YTMusic, "get_watch_playlist", boom)
    assert discovery.ytm_radio("dQw4w9WgXcQ") == []


def test_split_artist_title():
    assert discovery.split_artist_title("Drake - Hotline Bling") == ("Drake", "Hotline Bling")
    assert discovery.split_artist_title("Hotline Bling by Drake") == ("Drake", "Hotline Bling")
    a, t = discovery.split_artist_title("hotline bling")
    assert a == "" and t == "hotline bling"


def test_radio_reads_documented_artwork_and_duration(monkeypatch):
    import ytmusicapi

    monkeypatch.setattr(ytmusicapi.YTMusic, "get_watch_playlist", lambda self, **kw: {
        "tracks": [{"videoId": "abcdefghijk", "title": "Game theme", "length": "3:07",
                    "artists": [{"name": "Composer"}],
                    "thumbnail": [{"url": "https://example.com/art.jpg", "width": 120, "height": 120}]}]
    })
    tracks = discovery.ytm_radio("dQw4w9WgXcQ")
    assert tracks[0].durationSec == 187
    assert tracks[0].thumbnails[0]["url"] == "https://example.com/art.jpg"


def test_radio_resolves_saved_track_without_api_key(monkeypatch):
    from zayfetch import main
    from zayfetch.models import Candidate

    monkeypatch.setenv("LASTFM_API_KEY", "")
    monkeypatch.setattr(main, "_search_ytmusic", lambda query, count: [
        Candidate(videoId="seed1234567", title="Game theme", channel="Composer", source="ytmusic")
    ])
    requested = []

    def radio(video_id, limit):
        requested.append(video_id)
        return [discovery.RadioTrack(videoId="next1234567", title="Another theme", artist="Another composer")]

    monkeypatch.setattr(discovery, "ytm_radio", radio)
    result = main.radio(videoId="", artist="Composer", title="Game theme", limit=1)
    assert requested == ["seed1234567"]
    assert result.candidates[0].videoId == "next1234567"


def test_radio_excludes_seed_and_duplicate_recordings(monkeypatch):
    from zayfetch import main

    monkeypatch.setenv("LASTFM_API_KEY", "")
    monkeypatch.setattr(discovery, "ytm_radio", lambda *a: [
        discovery.RadioTrack(videoId="seed1234567", title="Seed", artist="Artist"),
        discovery.RadioTrack(videoId="next1234567", title="New song", artist="Artist"),
        discovery.RadioTrack(videoId="dupe1234567", title="New song", artist="Artist"),
        discovery.RadioTrack(videoId="live1234567", title="New song (Live)", artist="Artist"),
    ])
    result = main.radio(videoId="seed1234567", artist="Artist", title="Seed", limit=4)
    assert [c.videoId for c in result.candidates] == ["next1234567", "live1234567"]


def test_radio_seed_prefers_recorded_import_video(monkeypatch):
    from zayfetch import main

    monkeypatch.setenv("LASTFM_API_KEY", "")
    monkeypatch.setattr(main.store, "video_for_song",
                        lambda song_id: "truthful000" if song_id == "song-1" else None)
    requested = []

    def watch(video_id, limit):
        requested.append(video_id)
        return [discovery.RadioTrack(videoId="next1234567", title="Another theme", artist="Another composer")]

    monkeypatch.setattr(discovery, "ytm_radio", watch)
    result = main.radio(videoId="", songId="song-1", artist="Artist", title="Owned song", limit=1)
    assert requested == ["truthful000"]
    assert result.candidates[0].videoId == "next1234567"


def test_radio_seed_falls_back_to_embedded_comment_video(monkeypatch):
    from zayfetch import main

    monkeypatch.setenv("LASTFM_API_KEY", "")
    monkeypatch.setattr(main.store, "video_for_song", None)
    monkeypatch.setattr(main.importer, "video_id_for_library_song",
                        lambda song_id: "exactsetup1" if song_id == "song-2" else "")
    requested = []

    def watch(video_id, limit):
        requested.append(video_id)
        return [discovery.RadioTrack(videoId="next1234567", title="Another theme", artist="Another composer")]

    monkeypatch.setattr(discovery, "ytm_radio", watch)
    result = main.radio(videoId="", songId="song-2", artist="Artist", title="Owned song", limit=1)
    assert requested == ["exactsetup1"]
    assert result.candidates[0].videoId == "next1234567"


def test_seed_video_prefers_identity_and_matching_duration(monkeypatch):
    from zayfetch import main
    from zayfetch.models import Candidate

    search = [
        Candidate(videoId="q1", title="Original", channel="Artist", source="youtube",
                  durationSec=45),
        Candidate(videoId="q2", title="Original (Tropical Remix)", channel="Artist",
                  source="youtube", durationSec=380),
        Candidate(videoId="q3", title="Original", channel="Artist", source="youtube",
                  durationSec=180),
        Candidate(videoId="q4", title="Unrelated song", channel="Someone", source="youtube",
                  durationSec=185),
    ]
    assert main._pick_seed_video(search, "Artist", "Original", 180) == "q3"
    # Identity still wins over length without durations — q1/q3 are both the
    # plain recording; the remix must not be picked.
    assert main._pick_seed_video(search, "Artist", "Original", 0) in {"q1", "q3"}


def test_radio_exclude_library_skips_owned_recordings(monkeypatch):
    from zayfetch import main

    monkeypatch.setenv("LASTFM_API_KEY", "")
    monkeypatch.setattr(main.library, "known_recordings",
                        lambda: {("artist", "new song"), ("artist", "seed")})
    monkeypatch.setattr(discovery, "ytm_radio", lambda *a: [
        discovery.RadioTrack(videoId="owned1234567", title="New song", artist="Artist"),
        discovery.RadioTrack(videoId="fresh1234567", title="Brand new", artist="Artist"),
    ])
    result = main.radio(videoId="seed1234567", artist="Artist", title="Seed",
                        limit=4, excludeLibrary=True)
    assert [c.videoId for c in result.candidates] == ["fresh1234567"]
    # Without the flag the same candidates are all returned.
    result_all = main.radio(videoId="seed1234567", artist="Artist", title="Seed", limit=4)
    assert [c.videoId for c in result_all.candidates] == ["owned1234567", "fresh1234567"]


def test_ytmusic_call_falls_through_to_cookie_client(monkeypatch):
    from zayfetch import discovery

    class Client:
        def __init__(self, name, fails):
            self.name, self.fails, self.auth_type = name, fails, name

        def get_playlist(self, playlist_id, limit=100):
            if self.fails:
                raise RuntimeError("HTTP 400")
            return {"by": self.name, "limit": limit}

    used: list[str] = []

    def clients(authenticated=True):
        for c in (Client("oauth", True), Client("browser", False), Client("anon", False)):
            used.append(c.name)
            yield c

    monkeypatch.setattr(discovery, "_ytmusic_clients", clients)
    assert discovery.ytmusic_call("get_playlist", "PL1", limit=None) == {"by": "browser", "limit": None}
    assert used == ["oauth", "browser"]  # anonymous is never built when cookies work

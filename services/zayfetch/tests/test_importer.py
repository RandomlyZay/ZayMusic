"""One-way YouTube Music playlist import (§import)."""

import pytest

from zayfetch import importer, store


@pytest.fixture(autouse=True)
def isolated(tmp_path, monkeypatch):
    monkeypatch.setenv("DATA_DIR", str(tmp_path))
    monkeypatch.setenv("NAVIDROME_URL", "http://navidrome:4533")
    monkeypatch.setenv("NAVIDROME_USER", "admin")
    monkeypatch.setenv("NAVIDROME_PASS", "secret")
    store._CONN = None
    yield
    if store._CONN is not None:
        store._CONN.close()
        store._CONN = None


def _ytm_playlist(tracks):
    return {"title": "Road Trip", "tracks": tracks}


def _track(video_id, title, artist="", album="", duration=180):
    return {
        "videoId": video_id,
        "title": title,
        "artists": [{"name": artist}] if artist else [],
        "album": {"name": album} if album else None,
        "duration_seconds": duration,
        "thumbnails": [{"url": "https://img/1.jpg", "width": 120, "height": 120}],
    }


def _connected(monkeypatch):
    monkeypatch.setattr(importer.downloader, "cookies_file", lambda: "/data/cookies/youtube.txt")


# --- YouTube Music library -------------------------------------------------


def test_library_playlists_requires_connection(monkeypatch):
    monkeypatch.setattr(importer.downloader, "cookies_file", lambda: "")
    with pytest.raises(importer.ImportError_):
        importer.list_library_playlists()


def test_library_playlists_maps_fields(monkeypatch):
    _connected(monkeypatch)
    monkeypatch.setattr(importer.discovery, "ytmusic_call", lambda method, *a, **k: [
        {"playlistId": "PL1", "title": "Chill", "count": "1,160", "owned": True,
         "thumbnails": [{"url": "https://img/1.jpg", "width": 100, "height": 100}]},
        {"title": "no id"},
    ])
    playlists = importer.list_library_playlists()
    assert [p.playlistId for p in playlists] == ["PL1"]
    assert playlists[0].count == 1160 and playlists[0].owned is True
    assert playlists[0].thumbnails[0].url == "https://img/1.jpg"


def test_get_playlist_tracks_maps_and_parses_duration(monkeypatch):
    _connected(monkeypatch)
    monkeypatch.setattr(importer.discovery, "ytmusic_call", lambda method, *a, **k: _ytm_playlist([
        _track("vid00000001", "Song A", "Artist A", "Album A", duration=201),
        {"videoId": "vid00000002", "title": "Song B", "duration": "3:07"},
        {"title": "not playable"},
    ]))
    title, tracks = importer.get_playlist_tracks("PL1")
    assert title == "Road Trip"
    assert [t.videoId for t in tracks] == ["vid00000001", "vid00000002"]
    assert tracks[0].artist == "Artist A" and tracks[0].album == "Album A"
    assert tracks[1].durationSec == 187
    assert [t.position for t in tracks] == [0, 1]


# --- Matching --------------------------------------------------------------


def test_find_library_song_matches_video_id_in_comment(monkeypatch):
    monkeypatch.setattr(importer, "navidrome_search_songs", lambda q, count=20: [
        {"id": "s1", "title": "Whatever", "artist": "Someone",
         "comment": "https://www.youtube.com/watch?v=vid00000001"},
    ])
    track = importer.ImportTrack(videoId="vid00000001", title="Song A", artist="Artist A")
    assert importer.find_library_song(track) == "s1"


def test_find_library_song_matches_on_metadata(monkeypatch):
    monkeypatch.setattr(importer, "navidrome_search_songs", lambda q, count=20: [
        {"id": "s1", "title": "Song A", "artist": "Artist A", "duration": 200},
        {"id": "s2", "title": "Song C", "artist": "Artist C", "duration": 200},
    ])
    track = importer.ImportTrack(videoId="vid00000001", title="Song A",
                                 artist="Artist A", durationSec=201)
    assert importer.find_library_song(track) == "s1"


def test_find_library_song_rejects_mismatch_and_duration(monkeypatch):
    monkeypatch.setattr(importer, "navidrome_search_songs", lambda q, count=20: [
        {"id": "s1", "title": "Completely Different", "artist": "Artist A", "duration": 200},
        {"id": "s2", "title": "Song A", "artist": "Artist A", "duration": 300},
    ])
    track = importer.ImportTrack(videoId="vid00000001", title="Song A",
                                 artist="Artist A", durationSec=200)
    assert importer.find_library_song(track) is None


def test_find_library_song_handles_no_results(monkeypatch):
    monkeypatch.setattr(importer, "navidrome_search_songs", lambda q, count=20: [])
    track = importer.ImportTrack(videoId="vid00000001", title="Song A")
    assert importer.find_library_song(track) is None


# --- Import lifecycle ------------------------------------------------------


class _FakeNavidrome:
    def __init__(self):
        self.playlists: dict[str, list[str]] = {}
        self.deleted: list[str] = []
        self._n = 0

    def create(self, name, song_ids):
        self._n += 1
        pid = f"pl{self._n}"
        self.playlists[pid] = list(song_ids)
        return pid

    def entries(self, pid):
        if pid not in self.playlists:
            return None
        return list(self.playlists[pid])

    def delete(self, pid):
        self.deleted.append(pid)
        self.playlists.pop(pid, None)

    def replace(self, pid, song_ids):
        assert pid in self.playlists
        self.playlists[pid] = list(song_ids)


def _install_navidrome(monkeypatch):
    fake = _FakeNavidrome()

    def create(name, song_ids):
        created = fake.create(name, song_ids)
        # New playlists keep their id; keep old ones for delete assertions.
        return created

    monkeypatch.setattr(importer, "navidrome_create_playlist", create)
    monkeypatch.setattr(importer, "navidrome_playlist_song_ids", fake.entries)
    monkeypatch.setattr(importer, "navidrome_delete_playlist", fake.delete)
    monkeypatch.setattr(importer, "navidrome_replace_playlist_songs", fake.replace)
    return fake


def test_create_import_links_existing_and_queues_missing(monkeypatch):
    _connected(monkeypatch)
    fake = _install_navidrome(monkeypatch)
    monkeypatch.setattr(importer.discovery, "ytmusic_call", lambda method, *a, **k: _ytm_playlist([
        _track("vid00000001", "In Library", "Artist A"),
        _track("vid00000002", "Needs Download", "Artist B"),
    ]))
    monkeypatch.setattr(importer, "find_library_song",
                        lambda track: "song-1" if track.videoId == "vid00000001" else None)

    result = importer.create_import("PL1")
    assert result.name == "Road Trip"
    assert result.total == 2 and result.done == 1 and result.pending == 1
    assert result.playlistId is not None
    assert fake.entries(result.playlistId) == ["song-1"]

    missing = next(i for i in result.items if i.videoId == "vid00000002")
    assert missing.status == "downloading" and missing.jobId
    job = store.get_job(missing.jobId)
    assert job is not None and job.overrides is not None
    assert job.overrides.title == "Needs Download" and job.overrides.artist == "Artist B"


def test_reimport_reuses_existing_import(monkeypatch):
    _connected(monkeypatch)
    _install_navidrome(monkeypatch)
    monkeypatch.setattr(importer.discovery, "ytmusic_call", lambda method, *a, **k: _ytm_playlist([
        _track("vid00000001", "Song A", "Artist A"),
    ]))
    monkeypatch.setattr(importer, "find_library_song", lambda track: "song-1")
    first = importer.create_import("PL1")
    second = importer.create_import("PL1")
    assert second.id == first.id
    assert len(importer.list_imports()) == 1


def test_sync_links_downloaded_track_in_place(monkeypatch):
    _connected(monkeypatch)
    fake = _install_navidrome(monkeypatch)
    monkeypatch.setattr(importer.discovery, "ytmusic_call", lambda method, *a, **k: _ytm_playlist([
        _track("vid00000001", "Song A", "Artist A"),
        _track("vid00000002", "Song B", "Artist B"),
    ]))
    state = {"vid00000001": "song-1"}
    monkeypatch.setattr(importer, "find_library_song", lambda track: state.get(track.videoId))
    created = importer.create_import("PL1")
    assert fake.entries(created.playlistId) == ["song-1"]

    # The download lands: both tracks are now in the library, in YTM order.
    state["vid00000002"] = "song-2"
    for item in created.items:
        if item.jobId:
            store.update_job(item.jobId, status="done", progress=1.0, stage="done")
    synced = importer.sync_import(created.id)
    assert synced.done == 2 and synced.pending == 0
    assert synced.playlistId == created.playlistId
    assert fake.entries(synced.playlistId) == ["song-1", "song-2"]
    assert fake.deleted == []


def test_sync_marks_failed_job(monkeypatch):
    _connected(monkeypatch)
    _install_navidrome(monkeypatch)
    monkeypatch.setattr(importer.discovery, "ytmusic_call", lambda method, *a, **k: _ytm_playlist([
        _track("vid00000002", "Song B", "Artist B"),
    ]))
    monkeypatch.setattr(importer, "find_library_song", lambda track: None)
    result = importer.create_import("PL1")
    item = result.items[0]
    store.update_job(item.jobId, status="failed", error="boom")
    synced = importer.sync_import(result.id)
    assert synced.failed == 1
    assert synced.items[0].error == "boom"


def test_sync_skips_library_probe_while_download_running(monkeypatch):
    _connected(monkeypatch)
    _install_navidrome(monkeypatch)
    monkeypatch.setattr(importer.discovery, "ytmusic_call", lambda method, *a, **k: _ytm_playlist([
        _track("vid00000002", "Song B", "Artist B"),
    ]))
    probes: list[str] = []
    monkeypatch.setattr(importer, "find_library_song",
                        lambda track: probes.append(track.videoId) or None)
    created = importer.create_import("PL1")
    assert probes == ["vid00000002"]
    importer.sync_import(created.id)
    assert probes == ["vid00000002"]  # queued job → nothing to look up yet


def test_navidrome_delete_playlist_uses_id_param(monkeypatch):
    captured = {}
    monkeypatch.setattr(importer, "_navidrome_post",
                        lambda endpoint, data=None, **params: captured.update(endpoint=endpoint, data=data) or {})
    importer.navidrome_delete_playlist("pl-123")
    assert captured["endpoint"] == "deletePlaylist"
    assert captured["data"] == {"id": "pl-123"}


def test_remove_import_deletes_playlist_and_record(monkeypatch):
    _connected(monkeypatch)
    fake = _install_navidrome(monkeypatch)
    monkeypatch.setattr(importer.discovery, "ytmusic_call", lambda method, *a, **k: _ytm_playlist([
        _track("vid00000001", "Song A", "Artist A"),
    ]))
    monkeypatch.setattr(importer, "find_library_song", lambda track: "song-1")
    created = importer.create_import("PL1")
    importer.remove_import(created.id)
    assert fake.deleted[-1] == created.playlistId
    assert importer.list_imports() == []


def test_reconcile_preserves_order(monkeypatch):
    _connected(monkeypatch)
    fake = _install_navidrome(monkeypatch)
    monkeypatch.setattr(importer.discovery, "ytmusic_call", lambda method, *a, **k: _ytm_playlist([
        _track("vid00000001", "Song A", "Artist A"),
        _track("vid00000002", "Song B", "Artist B"),
        _track("vid00000003", "Song C", "Artist C"),
    ]))
    # Only the middle track exists at import time.
    state = {"vid00000002": "song-2"}
    monkeypatch.setattr(importer, "find_library_song", lambda track: state.get(track.videoId))
    created = importer.create_import("PL1")
    assert fake.entries(created.playlistId) == ["song-2"]

    # Later downloads resolve around it and slot in at their YTM positions.
    state.update({"vid00000001": "song-1", "vid00000003": "song-3"})
    for item in created.items:
        if item.jobId:
            store.update_job(item.jobId, status="done", progress=1.0, stage="done")
    importer.reconcile_imports()
    current = next(iter(importer.list_imports()))
    assert current.playlistId == created.playlistId
    assert fake.entries(current.playlistId) == ["song-1", "song-2", "song-3"]


def test_get_playlist_tracks_reads_every_track(monkeypatch):
    _connected(monkeypatch)
    calls = []
    monkeypatch.setattr(importer.discovery, "ytmusic_call",
                        lambda method, *a, **k: calls.append(k) or _ytm_playlist([]))
    importer.get_playlist_tracks("PL1")
    assert calls == [{"limit": None}]


def test_merge_missing_keeps_local_songs_and_fills_in_ytm_order():
    assert importer.merge_missing(["mine", "b"], ["a", "b", "c"]) == ["mine", "a", "b", "c"]
    assert importer.merge_missing(["x"], ["a", "b"]) == ["x", "a", "b"]
    assert importer.merge_missing(["b", "a"], ["a", "b"]) == ["b", "a"]


def _import_with(monkeypatch, tracks, library):
    _connected(monkeypatch)
    fake = _install_navidrome(monkeypatch)
    state = {"tracks": tracks}
    monkeypatch.setattr(importer.discovery, "ytmusic_call",
                        lambda method, *a, **k: _ytm_playlist(state["tracks"]))
    monkeypatch.setattr(importer, "find_library_song", lambda track: library.get(track.videoId))
    return fake, state, importer.create_import("PL1")


def test_pull_never_removes_local_songs(monkeypatch):
    library = {"vid00000001": "song-1"}
    fake, state, created = _import_with(monkeypatch, [_track("vid00000001", "A", "X")], library)
    fake.playlists[created.playlistId].append("my-own-song")
    # The song also disappears from the YTM playlist: still kept locally.
    state["tracks"] = []
    pulled = importer.pull_import(created.id)
    assert fake.entries(pulled.playlistId) == ["song-1", "my-own-song"]


def test_pull_adds_new_ytm_songs_and_restores_removed_ones(monkeypatch):
    library = {"vid00000001": "song-1", "vid00000002": "song-2"}
    fake, state, created = _import_with(monkeypatch, [_track("vid00000001", "A", "X")], library)
    fake.playlists[created.playlistId] = []  # removed locally
    state["tracks"] = [_track("vid00000001", "A", "X"), _track("vid00000002", "B", "Y")]
    pulled = importer.pull_import(created.id)
    assert pulled.playlistId == created.playlistId
    assert fake.entries(pulled.playlistId) == ["song-1", "song-2"]


def test_deleted_playlist_is_only_recreated_by_pull(monkeypatch):
    library = {"vid00000001": "song-1"}
    fake, _, created = _import_with(monkeypatch, [_track("vid00000001", "A", "X")], library)
    fake.delete(created.playlistId)
    importer.sync_import(created.id)
    assert fake.playlists == {}
    pulled = importer.pull_import(created.id)
    assert fake.entries(pulled.playlistId) == ["song-1"]


def test_pull_retries_failed_downloads(monkeypatch):
    fake, _, created = _import_with(monkeypatch, [_track("vid00000002", "B", "Y")], {})
    store.update_job(created.items[0].jobId, status="failed", error="boom")
    assert importer.sync_import(created.id).failed == 1
    pulled = importer.pull_import(created.id)
    assert pulled.failed == 0 and pulled.pending == 1


def test_reconcile_is_total_on_navidrome_failure(monkeypatch):
    _connected(monkeypatch)

    def boom(*a, **k):
        raise importer.ImportError_("navidrome down")

    monkeypatch.setattr(importer, "navidrome_search_songs", boom)
    monkeypatch.setattr(importer.discovery, "ytmusic_call", lambda method, *a, **k: _ytm_playlist([
        _track("vid00000001", "Song A", "Artist A"),
    ]))
    # create_import tolerates a Navidrome outage instead of raising.
    result = importer.create_import("PL1")
    assert result.total == 1
    importer.reconcile_imports()  # never raises


# --- HTTP wiring -----------------------------------------------------------


def test_endpoints_require_and_report_connection(monkeypatch):
    from fastapi.testclient import TestClient

    from zayfetch import main

    monkeypatch.setattr(main, "TOKEN", "")
    monkeypatch.setattr(importer.downloader, "cookies_file", lambda: "")
    client = TestClient(main.app)
    assert client.get("/youtube/library/playlists").status_code == 412
    assert client.post("/youtube/imports", json={"playlistId": "PL1"}).status_code == 412

    # A stored-but-rejected session is actionable, not a silent empty library.
    monkeypatch.setattr(importer.downloader, "cookies_file", lambda: "/data/cookies/youtube.txt")
    monkeypatch.setattr(main, "_youtube_status", lambda: {"configured": True, "verified": False})
    response = client.get("/youtube/library/playlists")
    assert response.status_code == 412
    assert "reconnect" in response.json()["detail"].lower()


def test_import_endpoint_creates(monkeypatch):
    from fastapi.testclient import TestClient

    from zayfetch import main

    monkeypatch.setattr(main, "TOKEN", "")
    _connected(monkeypatch)
    _install_navidrome(monkeypatch)
    monkeypatch.setattr(importer.discovery, "ytmusic_call", lambda method, *a, **k: _ytm_playlist([
        _track("vid00000001", "Song A", "Artist A"),
    ]))
    monkeypatch.setattr(importer, "find_library_song", lambda track: "song-1")
    monkeypatch.setattr(main, "_youtube_status", lambda: {"configured": True, "verified": True})
    monkeypatch.setattr(main, "ensure_worker", lambda: None)
    monkeypatch.setattr(main, "ensure_reconciler", lambda: None)
    client = TestClient(main.app)
    response = client.post("/youtube/imports", json={"playlistId": "PL1"})
    assert response.status_code == 200
    body = response.json()["import"]
    assert body["name"] == "Road Trip" and body["done"] == 1
    assert client.get("/youtube/imports").json()["imports"][0]["id"] == body["id"]


def test_repeated_playlist_tracks_are_seeded_once(monkeypatch):
    fake, _, created = _import_with(monkeypatch, [
        _track("vid00000001", "Song", "Artist"), _track("vid00000001", "Song", "Artist"),
    ], {})
    assert created.total == 1
    assert len(store.list_jobs()) == 1
    assert created.items[0].position == 0


def test_legacy_duplicate_pending_item_does_not_requeue_failed_song(monkeypatch):
    _, _, created = _import_with(monkeypatch, [_track("vid00000001", "Song", "Artist")], {})
    # Reproduce the old database: duplicate item was never updated by the first-row setter.
    conn = store.get_conn()
    conn.execute("INSERT INTO import_items (id,import_id,position,video_id,title,artist,status,updated_at) "
                 "VALUES ('legacy-duplicate',?,?,?,'Song','Artist','pending','now')",
                 (created.id, 1, "vid00000001"))
    conn.commit()
    store.update_job(created.items[0].jobId, status="failed", error="session expired")
    for _ in range(5):
        synced = importer.sync_import(created.id)
        assert synced.total == 1 and synced.failed == 1
    assert conn.execute("SELECT COUNT(*) FROM jobs").fetchone()[0] == 1
    assert {row[0] for row in conn.execute("SELECT status FROM import_items")} == {"failed"}


def test_overlapping_imports_share_a_download(monkeypatch):
    _, _, first = _import_with(monkeypatch, [_track("vid00000001", "Song", "Artist")], {})
    second = importer.create_import("PL2")
    assert first.items[0].jobId == second.items[0].jobId
    assert len(store.list_jobs()) == 1


def test_library_matching_does_not_confuse_live_recordings_or_missing_artist(monkeypatch):
    monkeypatch.setattr(importer, "navidrome_search_songs", lambda q, count=20: [
        {"id": "live", "title": "Song (Live)", "artist": "Artist", "duration": 180},
    ])
    assert importer.find_library_song(importer.ImportTrack(videoId="video001", title="Song", artist="Artist")) is None
    assert importer.find_library_song(importer.ImportTrack(videoId="video001", title="Song (Live)", artist="")) is None


def test_retry_from_downloads_resumes_a_failed_import(monkeypatch):
    _, _, created = _import_with(monkeypatch, [_track("vid00000001", "Song", "Artist")], {})
    job_id = created.items[0].jobId
    store.update_job(job_id, status="failed", error="session expired")
    assert importer.sync_import(created.id).failed == 1
    store.retry_job(job_id)
    retried = importer.sync_import(created.id)
    assert retried.failed == 0 and retried.pending == 1
    monkeypatch.setattr(importer, "find_library_song", lambda track: "song-1")
    store.update_job(job_id, status="done")
    assert importer.sync_import(created.id).done == 1

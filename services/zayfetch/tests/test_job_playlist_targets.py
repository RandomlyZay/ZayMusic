"""The "+" flow saves a download job with playlist targets: those songs must
end up in the chosen playlist once the download lands."""

from fastapi.testclient import TestClient
import pytest

from zayfetch import main, store, importer
from zayfetch.models import Overrides, JobResult


@pytest.fixture(autouse=True)
def isolated(tmp_path, monkeypatch):
    monkeypatch.setenv("DATA_DIR", str(tmp_path / "data"))
    monkeypatch.setenv("MUSIC_DIR", str(tmp_path))
    monkeypatch.setenv("STAGING_DIR", str(tmp_path / "staging"))
    monkeypatch.setattr(main, "MUSIC_DIR", tmp_path)
    monkeypatch.setattr(main, "TOKEN", "")
    monkeypatch.setattr(main, "ensure_worker", lambda: None)
    monkeypatch.setattr(main.downloader, "cookies_file", lambda: "/dev/null")
    store._CONN = None
    yield
    if store._CONN is not None:
        store._CONN.close()
        store._CONN = None


def _posts(monkeypatch, playlists: dict[str, list[str] | None]) -> list[tuple]:
    """Stub Navidrome playlist reads/writes; returns recorded playlist writes."""
    writes: list[tuple] = []
    monkeypatch.setattr(importer, "_navidrome_post", lambda endpoint, data=None, **k: writes.append((endpoint, data)))
    monkeypatch.setattr(importer, "navidrome_playlist_song_ids", lambda pid: playlists.get(pid))
    return writes


def test_create_job_records_playlist_targets():
    client = TestClient(main.app)
    body = {"videoId": "video001", "playlistIds": ["pl-a", "bad id!", "pl-b"]}
    response = client.post("/jobs", json=body)
    assert response.status_code == 200
    job_id = response.json()["job"]["id"]
    targets = store.list_job_targets()
    assert {t["playlistId"] for t in targets} == {"pl-a", "pl-b"}
    assert all(t["jobStatus"] == "queued" for t in targets)
    assert job_id == targets[0]["jobId"]


def test_reusable_job_gains_new_targets():
    client = TestClient(main.app)
    body = {"videoId": "video001", "overrides": {"title": "Song", "artist": "Artist"}}
    first = client.post("/jobs", json={**body, "playlistIds": ["pl-a"]})
    second = client.post("/jobs", json={**body, "playlistIds": ["pl-b"]})
    assert first.status_code == second.status_code == 200
    assert first.json()["job"]["id"] == second.json()["job"]["id"]
    assert {t["playlistId"] for t in store.list_job_targets()} == {"pl-a", "pl-b"}


def test_done_job_with_song_id_adds_to_playlist(monkeypatch):
    job = store.create_job("video001", overrides=Overrides(title="Song", artist="Artist"))
    store.add_job_targets(job.id, ["pl-a"])
    store.update_job(job.id, status="done", result=JobResult(path="x.opus", songId="song-1", title="Song", artist="Artist"))
    posts = _posts(monkeypatch, {"pl-a": []})
    importer.reconcile_job_targets()
    assert posts == [("updatePlaylist", {"playlistId": "pl-a", "songIdToAdd": ["song-1"]})]
    assert store.list_job_targets() == []
    # Redundant runs stay redundant: the target is gone.
    importer.reconcile_job_targets()
    assert posts == [("updatePlaylist", {"playlistId": "pl-a", "songIdToAdd": ["song-1"]})]


def test_song_already_in_playlist_is_dropped_without_write(monkeypatch):
    job = store.create_job("video001", overrides=Overrides(title="Song", artist="Artist"))
    store.add_job_targets(job.id, ["pl-a"])
    store.update_job(job.id, status="done", result=JobResult(path="x.opus", songId="song-1"))
    _posts(monkeypatch, {"pl-a": ["song-1"]})
    importer.reconcile_job_targets()
    assert store.list_job_targets() == []


def test_missing_playlist_drops_target(monkeypatch):
    job = store.create_job("video001", overrides=Overrides(title="Song", artist="Artist"))
    store.add_job_targets(job.id, ["pl-a"])
    store.update_job(job.id, status="done", result=JobResult(path="x.opus", songId="song-1"))
    posts = _posts(monkeypatch, {"pl-a": None})
    importer.reconcile_job_targets()
    assert posts == []
    assert store.list_job_targets() == []


def test_fresh_download_resolves_song_id_from_library(monkeypatch):
    job = store.create_job("video001", overrides=Overrides(title="Song", artist="Artist"))
    store.add_job_targets(job.id, ["pl-a"])
    store.update_job(job.id, status="done", result=JobResult(path="x.opus", title="Song", artist="Artist"))
    monkeypatch.setattr(importer, "navidrome_search_songs", lambda *a, **k: [
        {"id": "song-9", "title": "Song", "artist": "Artist"}
    ])
    posts = _posts(monkeypatch, {"pl-a": []})
    importer.reconcile_job_targets()
    assert posts == [("updatePlaylist", {"playlistId": "pl-a", "songIdToAdd": ["song-9"]})]
    assert store.list_job_targets() == []


def test_queued_and_failed_jobs_wait_without_writes(monkeypatch):
    queued = store.create_job("video001", overrides=Overrides(title="Song", artist="Artist"))
    store.add_job_targets(queued.id, ["pl-a"])
    failed = store.create_job("video002", overrides=Overrides(title="Other", artist="Artist"))
    store.add_job_targets(failed.id, ["pl-a"])
    posts = _posts(monkeypatch, {"pl-a": []})
    importer.reconcile_job_targets()
    assert posts == []
    assert {t["jobStatus"] for t in store.list_job_targets()} == {"queued"}

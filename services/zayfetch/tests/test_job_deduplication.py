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
    monkeypatch.setattr(importer, "navidrome_search_songs", lambda *a, **k: [])
    store._CONN = None
    yield
    if store._CONN is not None:
        store._CONN.close()
        store._CONN = None


def test_save_reuses_active_job_even_without_connected_youtube(monkeypatch):
    monkeypatch.setattr(main.downloader, "cookies_file", lambda: "")
    job = store.create_job("video001", overrides=Overrides(title="Song", artist="Artist"))
    response = TestClient(main.app).post("/jobs", json={"videoId": "video001"})
    assert response.status_code == 200
    assert response.json()["job"]["id"] == job.id


def test_save_links_an_existing_library_recording_without_acquisition(tmp_path, monkeypatch):
    path = tmp_path / "existing.opus"
    path.write_bytes(b"audio")
    monkeypatch.setattr(main.downloader, "cookies_file", lambda: "")
    monkeypatch.setattr(importer, "navidrome_search_songs", lambda *a, **k: [
        {"id": "song-1", "title": "Song", "artist": "Artist"},
    ])
    monkeypatch.setattr(importer, "_navidrome_get", lambda *a, **k: {
        "song": {"id": "song-1", "title": "Song", "artist": "Artist", "path": path.name},
    })
    client = TestClient(main.app)
    body = {"videoId": "video001", "overrides": {"title": "Song", "artist": "Artist"}}
    first = client.post("/jobs", json=body)
    second = client.post("/jobs", json=body)
    assert first.status_code == second.status_code == 200
    assert first.json()["job"]["status"] == "done"
    assert first.json()["job"]["stage"] == "existing"
    assert first.json()["job"]["result"]["songId"] == "song-1"
    assert first.json()["job"]["id"] == second.json()["job"]["id"]
    assert store.next_queued() is None


def test_worker_skips_legacy_duplicate_already_on_disk(tmp_path, monkeypatch):
    path = tmp_path / "existing.opus"
    original = store.create_job("video001")
    store.update_job(original.id, status="failed")
    duplicate = store.create_job("video001")
    path.write_bytes(b"audio")
    store.update_job(original.id, status="done", result=JobResult(path=path.name, title="Song", artist="Artist"))
    monkeypatch.setattr(main.downloader, "download", lambda *a, **k: pytest.fail("Duplicate audio acquisition"))
    main.run_job(duplicate)
    result = store.get_job(duplicate.id)
    assert result.status == "done" and result.stage == "existing"


def test_worker_preserves_authenticated_failure_without_anonymous_retry(monkeypatch):
    job = store.create_job("video001")
    calls = []
    error = "Video unavailable for the connected account"

    def fail_download(video_id, *, with_cookies=True, **kwargs):
        calls.append((video_id, with_cookies))
        raise main.DownloadError(error if with_cookies else "Anonymous failure")

    monkeypatch.setattr(main.downloader, "cookies_file", lambda: "/tmp/cookies.txt")
    monkeypatch.setattr(main.downloader, "download", fail_download)
    main.run_job(job)

    result = store.get_job(job.id)
    assert result.status == "failed"
    assert result.error == error
    assert calls == [("video001", True)]

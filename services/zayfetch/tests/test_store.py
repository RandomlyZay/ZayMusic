"""Job store state-transition tests (§15)."""

import os
from pathlib import Path

import pytest

from zayfetch import store
from zayfetch.models import JobResult, Overrides


@pytest.fixture(autouse=True)
def tmp_db(tmp_path: Path, monkeypatch):
    monkeypatch.setenv("DATA_DIR", str(tmp_path))
    # reset module singleton
    store._CONN = None
    store._LOCK = store.threading.Lock() if hasattr(store, "threading") else store._LOCK
    yield
    if store._CONN is not None:
        store._CONN.close()
        store._CONN = None


def test_create_and_get():
    job = store.create_job("abc123", url="https://youtu.be/abc123")
    assert job.status == "queued"
    assert job.videoId == "abc123"
    got = store.get_job(job.id)
    assert got is not None
    assert got.id == job.id


def test_create_with_overrides():
    ov = Overrides(title="T", artist="A", track=2)
    job = store.create_job("vid", overrides=ov)
    got = store.get_job(job.id)
    assert got.overrides is not None
    assert got.overrides.title == "T"
    assert got.overrides.track == 2


def test_next_queued_claims_one_at_a_time():
    a = store.create_job("a")
    b = store.create_job("b")
    first = store.next_queued()
    assert first is not None and first.id == a.id
    assert first.status == "running"
    second = store.next_queued()
    assert second is not None and second.id == b.id
    # nothing else queued
    assert store.next_queued() is None


def test_progress_clamped():
    job = store.create_job("x")
    store.update_job(job.id, progress=1.5)
    assert store.get_job(job.id).progress == 1.0
    store.update_job(job.id, progress=-1)
    assert store.get_job(job.id).progress == 0.0


def test_done_sets_finished():
    job = store.create_job("x")
    store.update_job(job.id, status="done", progress=1.0, stage="done",
                     result=JobResult(path="a/b.opus", title="T"))
    got = store.get_job(job.id)
    assert got.status == "done"
    assert got.finishedAt is not None
    assert got.result.path == "a/b.opus"


def test_retry_requeues_failed():
    job = store.create_job("x")
    store.update_job(job.id, status="failed", error="boom")
    got = store.retry_job(job.id)
    assert got.status == "queued"
    assert got.error is None
    assert got.finishedAt is None


def test_retry_ignored_when_done():
    job = store.create_job("x")
    store.update_job(job.id, status="done")
    got = store.retry_job(job.id)
    assert got.status == "done"


def test_cancel_queued():
    job = store.create_job("x")
    got = store.cancel_job(job.id)
    assert got.status == "cancelled"


def test_delete():
    job = store.create_job("x")
    assert store.delete_job(job.id) is True
    assert store.get_job(job.id) is None
    assert store.delete_job(job.id) is False


def test_list_newest_first():
    import time

    a = store.create_job("a")
    time.sleep(0.01)
    b = store.create_job("b")
    jobs = store.list_jobs()
    assert jobs[0].id == b.id
    assert jobs[1].id == a.id


def test_concurrent_saves_share_one_job():
    from concurrent.futures import ThreadPoolExecutor

    with ThreadPoolExecutor(max_workers=8) as pool:
        jobs = list(pool.map(lambda _: store.create_job("same-video"), range(30)))
    assert len({job.id for job in jobs}) == 1
    assert len(store.list_jobs()) == 1


def test_same_recording_different_video_reuses_active_job_but_versions_stay_separate():
    original = store.create_job("first", overrides=Overrides(title="Song", artist="Artist"))
    repeated = store.create_job("second", overrides=Overrides(title=" SONG  ", artist="artist"))
    live = store.create_job("live", overrides=Overrides(title="Song (Live)", artist="Artist"))
    other_artist = store.create_job("other", overrides=Overrides(title="Song", artist="Other"))
    assert repeated.id == original.id
    assert live.id != original.id
    assert other_artist.id != original.id


def test_completed_file_reused_before_scan_and_deleted_file_can_be_downloaded_again(tmp_path, monkeypatch):
    monkeypatch.setenv("MUSIC_DIR", str(tmp_path))
    path = tmp_path / "track.opus"
    path.write_bytes(b"audio")
    original = store.create_job("first", overrides=Overrides(title="Song", artist="Artist"))
    store.update_job(original.id, status="done", result=JobResult(path=path.name, title="Song", artist="Artist"))
    assert store.create_job("first").id == original.id
    assert store.create_job("alternate", overrides=Overrides(title="Song", artist="Artist")).id == original.id
    path.unlink()
    assert store.create_job("first").id != original.id


def test_retry_reuses_another_active_job():
    failed = store.create_job("video")
    store.update_job(failed.id, status="failed", error="expired")
    active = store.create_job("video")
    assert store.retry_job(failed.id).id == active.id
    assert store.get_job(failed.id).status == "failed"
    assert store.list_jobs()[0].id == active.id


def test_job_list_collapses_attempts_before_applying_limit():
    first = store.create_job("video-1")
    store.update_job(first.id, status="failed", error="failed")
    second = store.create_job("video-1")
    another = store.create_job("video-2")
    assert {job.id for job in store.list_jobs(2)} == {second.id, another.id}


def test_legacy_schema_is_upgraded_with_recording_identity(tmp_path):
    import sqlite3

    conn = sqlite3.connect(tmp_path / "jobs.db")
    conn.executescript(store.SCHEMA.replace("  identity_key TEXT,\n", ""))
    conn.execute("INSERT INTO jobs (id,status,videoId,overrides_json,requested_at) VALUES (?,?,?,?,?)",
                 ("legacy", "queued", "first", Overrides(title="Song", artist="Artist").model_dump_json(), "now"))
    conn.commit()
    conn.close()
    assert store.create_job("alternate", overrides=Overrides(title="Song", artist="Artist")).id == "legacy"

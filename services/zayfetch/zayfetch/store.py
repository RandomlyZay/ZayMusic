"""SQLite job queue. One file, no ORM (§5)."""

from __future__ import annotations

import json
import os
import sqlite3
import threading
import unicodedata
import uuid
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from .models import FetchJob, JobResult, Overrides

_LOCK = threading.Lock()
_CONN: sqlite3.Connection | None = None

SCHEMA = """
CREATE TABLE IF NOT EXISTS jobs (
  id TEXT PRIMARY KEY,
  status TEXT NOT NULL,
  videoId TEXT NOT NULL,
  identity_key TEXT,
  url TEXT,
  overrides_json TEXT,
  progress REAL NOT NULL DEFAULT 0,
  stage TEXT NOT NULL DEFAULT 'queued',
  error TEXT,
  requested_at TEXT NOT NULL,
  started_at TEXT,
  finished_at TEXT,
  result_json TEXT,
  heartbeat_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_jobs_status ON jobs(status);
CREATE INDEX IF NOT EXISTS idx_jobs_requested ON jobs(requested_at DESC);

CREATE TABLE IF NOT EXISTS imports (
  id TEXT PRIMARY KEY,
  ytm_playlist_id TEXT NOT NULL,
  name TEXT NOT NULL,
  navidrome_playlist_id TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_imports_ytm ON imports(ytm_playlist_id);

CREATE TABLE IF NOT EXISTS import_items (
  id TEXT PRIMARY KEY,
  import_id TEXT NOT NULL,
  position INTEGER NOT NULL,
  video_id TEXT NOT NULL,
  title TEXT NOT NULL,
  artist TEXT NOT NULL DEFAULT '',
  album TEXT NOT NULL DEFAULT '',
  duration_sec REAL NOT NULL DEFAULT 0,
  song_id TEXT,
  status TEXT NOT NULL DEFAULT 'pending',
  job_id TEXT,
  error TEXT,
  updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_import_items_import ON import_items(import_id, position);

CREATE TABLE IF NOT EXISTS album_saves (
  id TEXT PRIMARY KEY,
  title TEXT NOT NULL,
  artist TEXT NOT NULL DEFAULT '',
  album_id TEXT,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS job_playlist_targets (
  job_id TEXT NOT NULL,
  playlist_id TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY (job_id, playlist_id)
);
"""


def _now() -> str:
    return datetime.now(timezone.utc).isoformat()


def _data_dir() -> Path:
    return Path(os.environ.get("DATA_DIR", "/data"))


def _db_path() -> Path:
    return _data_dir() / "jobs.db"


def get_conn() -> sqlite3.Connection:
    global _CONN
    if _CONN is None:
        _data_dir().mkdir(parents=True, exist_ok=True)
        _CONN = sqlite3.connect(_db_path(), check_same_thread=False)
        _CONN.row_factory = sqlite3.Row
        _CONN.executescript(SCHEMA)
        columns = {row["name"] for row in _CONN.execute("PRAGMA table_info(jobs)")}
        if "identity_key" not in columns:
            _CONN.execute("ALTER TABLE jobs ADD COLUMN identity_key TEXT")
            for row in _CONN.execute("SELECT id, overrides_json, result_json FROM jobs").fetchall():
                metadata = json.loads(row["overrides_json"] or row["result_json"] or "{}")
                _CONN.execute("UPDATE jobs SET identity_key=? WHERE id=?",
                              (song_identity(metadata.get("title"), metadata.get("artist")), row["id"]))
        _CONN.execute("CREATE INDEX IF NOT EXISTS idx_jobs_video ON jobs(videoId, status)")
        _CONN.execute("CREATE INDEX IF NOT EXISTS idx_jobs_identity ON jobs(identity_key, status)")
        _CONN.commit()
    return _CONN


def _row_to_job(row: sqlite3.Row) -> FetchJob:
    overrides = None
    if row["overrides_json"]:
        try:
            overrides = Overrides.model_validate(json.loads(row["overrides_json"]))
        except Exception:
            overrides = None
    result = None
    if row["result_json"]:
        try:
            result = JobResult.model_validate(json.loads(row["result_json"]))
        except Exception:
            result = None
    return FetchJob(
        id=row["id"],
        status=row["status"],
        videoId=row["videoId"],
        url=row["url"],
        requestedAt=row["requested_at"],
        startedAt=row["started_at"],
        finishedAt=row["finished_at"],
        progress=row["progress"],
        stage=row["stage"],
        error=row["error"],
        overrides=overrides,
        result=result,
    )


def song_identity(title: str | None, artist: str | None) -> str | None:
    """Conservative recording identity: keep version/remix/live qualifiers."""
    def normalize(value: str | None) -> str:
        return " ".join(unicodedata.normalize("NFKC", value or "").casefold().split())
    title, artist = normalize(title), normalize(artist)
    return json.dumps([title, artist], ensure_ascii=False) if title and artist else None


def _find_reusable_job(conn: sqlite3.Connection, video_id: str, overrides: Overrides | None,
                       *, exclude_id: str = "", completed_only: bool = False) -> FetchJob | None:
    identity = song_identity(overrides.title, overrides.artist) if overrides else None
    statuses = "('done')" if completed_only else "('queued', 'running', 'done')"
    rows = conn.execute(
        f"SELECT * FROM jobs WHERE (videoId=? OR identity_key=?) AND id!=? AND status IN {statuses} "
        "ORDER BY CASE status WHEN 'running' THEN 0 WHEN 'queued' THEN 1 ELSE 2 END, requested_at DESC",
        (video_id, identity, exclude_id),
    ).fetchall()
    root = Path(os.environ.get("MUSIC_DIR", "/music")).resolve()
    for row in rows:
        job = _row_to_job(row)
        if job.status in ("queued", "running"):
            return job
        if job.result and job.result.path:
            path = (root / job.result.path).resolve()
            if path.is_relative_to(root) and path.is_file():
                return job
    return None


def find_reusable_job(video_id: str, overrides: Overrides | None = None, *,
                      exclude_id: str = "", completed_only: bool = False) -> FetchJob | None:
    with _LOCK:
        return _find_reusable_job(get_conn(), video_id, overrides,
                                  exclude_id=exclude_id, completed_only=completed_only)


def create_job(videoId: str, url: str | None = None,
               overrides: Overrides | None = None, *, existing_result: JobResult | None = None) -> FetchJob:
    job_id = uuid.uuid4().hex[:12]
    with _LOCK:
        conn = get_conn()
        # Check and insert under the same lock: concurrent saves/imports reuse one job.
        reusable = _find_reusable_job(conn, videoId, overrides)
        if reusable:
            return reusable
        identity = song_identity(overrides.title, overrides.artist) if overrides else None
        if not identity and existing_result:
            identity = song_identity(existing_result.title, existing_result.artist)
        now = _now()
        conn.execute(
            "INSERT INTO jobs (id, status, videoId, identity_key, url, overrides_json, progress, stage, "
            "requested_at, finished_at, result_json) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            (
                job_id,
                "done" if existing_result else "queued",
                videoId,
                identity,
                url,
                overrides.model_dump_json(exclude_none=True) if overrides else None,
                1 if existing_result else 0,
                "existing" if existing_result else "queued",
                now,
                now if existing_result else None,
                existing_result.model_dump_json() if existing_result else None,
            ),
        )
        conn.commit()
    return get_job(job_id)  # type: ignore[return-value]


def get_job(job_id: str) -> FetchJob | None:
    with _LOCK:
        conn = get_conn()
        row = conn.execute("SELECT * FROM jobs WHERE id = ?", (job_id,)).fetchone()
    return _row_to_job(row) if row else None


def add_job_targets(job_id: str, playlist_ids: list[str]) -> None:
    """Record that this job's song should end up in these playlists."""
    if not playlist_ids:
        return
    with _LOCK:
        conn = get_conn()
        now = _now()
        conn.executemany(
            "INSERT OR IGNORE INTO job_playlist_targets (job_id, playlist_id, created_at) "
            "VALUES (?, ?, ?)",
            [(job_id, playlist_id, now) for playlist_id in playlist_ids],
        )
        conn.commit()


def list_job_targets() -> list[dict]:
    """Pending playlist targets joined with their job's status."""
    with _LOCK:
        conn = get_conn()
        rows = conn.execute(
            "SELECT t.job_id AS jobId, t.playlist_id AS playlistId, t.created_at AS createdAt, "
            "j.status AS jobStatus FROM job_playlist_targets t "
            "LEFT JOIN jobs j ON j.id = t.job_id ORDER BY t.created_at"
        ).fetchall()
    return [dict(row) for row in rows]


def delete_job_target(job_id: str, playlist_id: str) -> None:
    with _LOCK:
        conn = get_conn()
        conn.execute(
            "DELETE FROM job_playlist_targets WHERE job_id = ? AND playlist_id = ?",
            (job_id, playlist_id),
        )
        conn.commit()



def list_jobs(limit: int = 50) -> list[FetchJob]:
    with _LOCK:
        conn = get_conn()
        rows = conn.execute(
            # Keep all attempts for import references, but show each source once.
            "SELECT * FROM (SELECT *, ROW_NUMBER() OVER (PARTITION BY videoId ORDER BY "
            "CASE status WHEN 'running' THEN 0 WHEN 'queued' THEN 1 ELSE 2 END, "
            "COALESCE(finished_at, started_at, requested_at) DESC) AS attempt FROM jobs) "
            "WHERE attempt=1 ORDER BY CASE status WHEN 'running' THEN 0 WHEN 'queued' THEN 1 "
            "WHEN 'failed' THEN 2 ELSE 3 END, requested_at DESC LIMIT ?", (limit,)
        ).fetchall()
    return [_row_to_job(r) for r in rows]


def find_active_job(video_id: str) -> FetchJob | None:
    """Most recent queued/running job for a video, so imports never double-queue."""
    with _LOCK:
        conn = get_conn()
        row = conn.execute(
            "SELECT * FROM jobs WHERE videoId = ? AND status IN ('queued', 'running') "
            "ORDER BY requested_at DESC LIMIT 1",
            (video_id,),
        ).fetchone()
    return _row_to_job(row) if row else None


def next_queued() -> FetchJob | None:
    """Claim the oldest queued job (single worker, §9.6)."""
    with _LOCK:
        conn = get_conn()
        row = conn.execute(
            "SELECT * FROM jobs WHERE status = 'queued' ORDER BY requested_at ASC LIMIT 1"
        ).fetchone()
        if not row:
            return None
        conn.execute(
            "UPDATE jobs SET status='running', started_at=?, heartbeat_at=? WHERE id=?",
            (_now(), _now(), row["id"]),
        )
        conn.commit()
    return get_job(row["id"])


def update_job(
    job_id: str,
    *,
    status: str | None = None,
    progress: float | None = None,
    stage: str | None = None,
    error: str | None = None,
    result: JobResult | None = None,
    heartbeat: bool = False,
) -> FetchJob | None:
    sets: list[str] = []
    vals: list[Any] = []
    if status is not None:
        sets.append("status = ?")
        vals.append(status)
        if status in ("done", "failed", "cancelled"):
            sets.append("finished_at = ?")
            vals.append(_now())
    if progress is not None:
        sets.append("progress = ?")
        vals.append(max(0.0, min(1.0, progress)))
    if stage is not None:
        sets.append("stage = ?")
        vals.append(stage)
    if error is not None:
        sets.append("error = ?")
        vals.append(error)
    if result is not None:
        sets.append("result_json = ?")
        vals.append(result.model_dump_json())
        sets.append("identity_key = COALESCE(identity_key, ?)")
        vals.append(song_identity(result.title, result.artist))
    if heartbeat:
        sets.append("heartbeat_at = ?")
        vals.append(_now())
    if not sets:
        return get_job(job_id)
    vals.append(job_id)
    with _LOCK:
        conn = get_conn()
        conn.execute(f"UPDATE jobs SET {', '.join(sets)} WHERE id = ?", vals)
        conn.commit()
    return get_job(job_id)


def retry_job(job_id: str) -> FetchJob | None:
    with _LOCK:
        conn = get_conn()
        row = conn.execute("SELECT * FROM jobs WHERE id=?", (job_id,)).fetchone()
        job = _row_to_job(row) if row else None
        if not job or job.status not in ("failed", "cancelled"):
            return job
        reusable = _find_reusable_job(conn, job.videoId, job.overrides, exclude_id=job.id)
        if reusable:
            return reusable
        conn.execute(
            "UPDATE jobs SET status='queued', progress=0, stage='queued', error=NULL, "
            "started_at=NULL, finished_at=NULL, heartbeat_at=NULL WHERE id=?",
            (job_id,),
        )
        conn.commit()
    return get_job(job_id)


def cancel_job(job_id: str) -> FetchJob | None:
    job = get_job(job_id)
    if not job:
        return None
    if job.status in ("queued", "running"):
        update_job(job_id, status="cancelled", error="cancelled by user")
    return get_job(job_id)


def delete_job(job_id: str) -> bool:
    with _LOCK:
        conn = get_conn()
        cur = conn.execute("DELETE FROM jobs WHERE id = ?", (job_id,))
        conn.commit()
        return cur.rowcount > 0


def requeue_stale_heartbeats(max_age_sec: int = 60) -> int:
    """Mark running jobs with no heartbeat for max_age_sec as failed (§9.6)."""
    with _LOCK:
        conn = get_conn()
        cur = conn.execute(
            "UPDATE jobs SET status='failed', error='heartbeat timeout', finished_at=? "
            "WHERE status='running' AND (heartbeat_at IS NULL OR "
            "julianday('now') - julianday(heartbeat_at) > ?)",
            (_now(), max_age_sec / 86400.0),
        )
        conn.commit()
        return cur.rowcount


# ---------------------------------------------------------------------------
# YouTube Music playlist imports
# ---------------------------------------------------------------------------


def _row_to_import(row: sqlite3.Row) -> dict[str, Any]:
    return {
        "id": row["id"],
        "ytmPlaylistId": row["ytm_playlist_id"],
        "name": row["name"],
        "playlistId": row["navidrome_playlist_id"],
        "createdAt": row["created_at"],
        "updatedAt": row["updated_at"],
    }


def _row_to_import_item(row: sqlite3.Row) -> dict[str, Any]:
    return {
        "videoId": row["video_id"],
        "title": row["title"],
        "artist": row["artist"],
        "album": row["album"],
        "durationSec": row["duration_sec"],
        "position": row["position"],
        "status": row["status"],
        "songId": row["song_id"],
        "jobId": row["job_id"],
        "error": row["error"],
    }


def create_import(
    *, ytm_playlist_id: str, name: str, navidrome_playlist_id: str | None = None
) -> dict[str, Any]:
    import_id = uuid.uuid4().hex[:12]
    now = _now()
    with _LOCK:
        conn = get_conn()
        conn.execute(
            "INSERT INTO imports (id, ytm_playlist_id, name, navidrome_playlist_id, created_at, updated_at) "
            "VALUES (?, ?, ?, ?, ?, ?)",
            (import_id, ytm_playlist_id, name, navidrome_playlist_id, now, now),
        )
        conn.commit()
    return get_import(import_id)  # type: ignore[return-value]


def get_import(import_id: str) -> dict[str, Any] | None:
    with _LOCK:
        conn = get_conn()
        row = conn.execute("SELECT * FROM imports WHERE id = ?", (import_id,)).fetchone()
    return _row_to_import(row) if row else None


def get_import_by_ytm(ytm_playlist_id: str) -> dict[str, Any] | None:
    with _LOCK:
        conn = get_conn()
        row = conn.execute(
            "SELECT * FROM imports WHERE ytm_playlist_id = ?", (ytm_playlist_id,)
        ).fetchone()
    return _row_to_import(row) if row else None


def list_imports() -> list[dict[str, Any]]:
    with _LOCK:
        conn = get_conn()
        rows = conn.execute("SELECT * FROM imports ORDER BY created_at DESC").fetchall()
    return [_row_to_import(r) for r in rows]


def update_import(import_id: str, **fields: Any) -> dict[str, Any] | None:
    columns = {
        "name": "name",
        "navidrome_playlist_id": "navidrome_playlist_id",
        "playlistId": "navidrome_playlist_id",
    }
    with _LOCK:
        conn = get_conn()
        sets: list[str] = ["updated_at = ?"]
        vals: list[Any] = [_now()]
        for key, value in fields.items():
            column = columns.get(key)
            if column is None:
                continue
            sets.append(f"{column} = ?")
            vals.append(value)
        vals.append(import_id)
        conn.execute(f"UPDATE imports SET {', '.join(sets)} WHERE id = ?", vals)
        conn.commit()
    return get_import(import_id)


def delete_import(import_id: str) -> bool:
    with _LOCK:
        conn = get_conn()
        conn.execute("DELETE FROM import_items WHERE import_id = ?", (import_id,))
        cur = conn.execute("DELETE FROM imports WHERE id = ?", (import_id,))
        conn.commit()
        return cur.rowcount > 0


def add_import_item(
    import_id: str,
    *,
    video_id: str,
    title: str,
    artist: str = "",
    album: str = "",
    duration_sec: float = 0.0,
    position: int = 0,
    status: str = "pending",
    song_id: str | None = None,
    job_id: str | None = None,
) -> None:
    with _LOCK:
        conn = get_conn()
        if conn.execute("SELECT 1 FROM import_items WHERE import_id=? AND video_id=?",
                        (import_id, video_id)).fetchone():
            return
        conn.execute(
            "INSERT INTO import_items (id, import_id, position, video_id, title, artist, album, "
            "duration_sec, song_id, status, job_id, updated_at) "
            "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            (
                uuid.uuid4().hex[:12],
                import_id,
                position,
                video_id,
                title,
                artist,
                album,
                duration_sec,
                song_id,
                status,
                job_id,
                _now(),
            ),
        )
        conn.commit()


def list_import_items(import_id: str) -> list[dict[str, Any]]:
    with _LOCK:
        conn = get_conn()
        rows = conn.execute(
            "SELECT * FROM (SELECT *, ROW_NUMBER() OVER (PARTITION BY video_id ORDER BY "
            "CASE status WHEN 'done' THEN 0 WHEN 'downloading' THEN 1 WHEN 'failed' THEN 2 ELSE 3 END, "
            "position ASC) AS duplicate FROM import_items WHERE import_id=?) "
            "WHERE duplicate=1 ORDER BY position ASC", (import_id,)
        ).fetchall()
    return [_row_to_import_item(r) for r in rows]


def video_for_song(song_id: str) -> str | None:
    """The exact YouTube video a library song came from, when it was imported."""
    if not song_id:
        return None
    with _LOCK:
        conn = get_conn()
        row = conn.execute(
            "SELECT video_id FROM import_items WHERE song_id = ? "
            "ORDER BY updated_at DESC LIMIT 1", (song_id,)
        ).fetchone()
    return row["video_id"] if row else None


def update_import_item(item_id: str, **fields: Any) -> None:
    columns = {
        "status": "status",
        "song_id": "song_id",
        "songId": "song_id",
        "job_id": "job_id",
        "jobId": "job_id",
        "error": "error",
    }
    sets: list[str] = ["updated_at = ?"]
    vals: list[Any] = [_now()]
    for key, value in fields.items():
        column = columns.get(key)
        if column is None:
            continue
        sets.append(f"{column} = ?")
        vals.append(value)
    with _LOCK:
        conn = get_conn()
        conn.execute(f"UPDATE import_items SET {', '.join(sets)} WHERE id = ?", [*vals, item_id])
        conn.commit()


def set_import_item_by_video(import_id: str, video_id: str, **fields: Any) -> None:
    columns = {
        "status": "status",
        "song_id": "song_id",
        "songId": "song_id",
        "job_id": "job_id",
        "jobId": "job_id",
        "error": "error",
        "position": "position",
    }
    with _LOCK:
        conn = get_conn()
        row = conn.execute(
            "SELECT id FROM import_items WHERE import_id = ? AND video_id = ?", (import_id, video_id)
        ).fetchone()
        if row is None:
            return
        item_id = row["id"]
        sets: list[str] = ["updated_at = ?"]
        vals: list[Any] = [_now()]
        for key, value in fields.items():
            column = columns.get(key)
            if column is None:
                continue
            sets.append(f"{column} = ?")
            vals.append(value)
        # Repair legacy duplicate records together; leaving one pending causes an
        # endless re-download loop on every reconciliation tick.
        conn.execute(f"UPDATE import_items SET {', '.join(sets)} WHERE import_id=? AND video_id=?",
                     [*vals, import_id, video_id])
        conn.commit()


# --- album saves: "save this YTM album" intents, starred once indexed ---


def create_album_save(title: str, artist: str) -> dict[str, Any]:
    with _LOCK:
        conn = get_conn()
        row = conn.execute(
            "SELECT * FROM album_saves WHERE title = ? AND artist = ? AND album_id IS NULL",
            (title, artist),
        ).fetchone()
        if row:
            return dict(row)
        save_id = uuid.uuid4().hex[:12]
        conn.execute(
            "INSERT INTO album_saves (id, title, artist, created_at) VALUES (?, ?, ?, ?)",
            (save_id, title, artist, _now()),
        )
        conn.commit()
        return dict(conn.execute("SELECT * FROM album_saves WHERE id = ?", (save_id,)).fetchone())


def pending_album_saves() -> list[dict[str, Any]]:
    with _LOCK:
        conn = get_conn()
        rows = conn.execute(
            "SELECT * FROM album_saves WHERE album_id IS NULL ORDER BY created_at"
        ).fetchall()
    return [dict(r) for r in rows]


def finish_album_save(save_id: str, album_id: str) -> None:
    with _LOCK:
        conn = get_conn()
        conn.execute("UPDATE album_saves SET album_id = ? WHERE id = ?", (album_id, save_id))
        conn.commit()


def delete_album_save(save_id: str) -> None:
    with _LOCK:
        conn = get_conn()
        conn.execute("DELETE FROM album_saves WHERE id = ?", (save_id,))
        conn.commit()

//! Listening history and OpenSubsonic scrobbles, shared by both audio engines.
use crate::{err, model::Song, NativeCore, Result};
use rusqlite::params;
use serde_json::{json, Value};
use std::sync::{atomic::Ordering, Arc};

#[derive(Default)]
pub(crate) struct Listening {
    epoch: u64,
    started: bool,
    submitted: bool,
}

#[uniffi::export]
impl NativeCore {
    /// Report the actual native player's clock. Paused/prepared items never count.
    /// SQLite and network work run away from the UI and audio streaming threads.
    pub fn playback_progress(
        self: Arc<Self>,
        epoch: u64,
        song_id: String,
        position: f64,
        duration: f64,
        playing: bool,
    ) -> Result<()> {
        if !playing || !position.is_finite() || position < 0. {
            return Ok(());
        }
        let queue = self.queue.lock().map_err(|_| err("Queue unavailable"))?;
        if epoch != self.playback_epoch.load(Ordering::SeqCst) {
            return Ok(());
        }
        let Some(song) = queue.current().filter(|s| s.id == song_id).cloned() else {
            return Ok(());
        };
        // Previews are not server-library songs and must not become library history.
        if song.preview_url.as_ref().is_some_and(|url| !url.is_empty())
            || song.id.starts_with("yt:")
        {
            return Ok(());
        }
        let mut state = self
            .listening
            .lock()
            .map_err(|_| err("History unavailable"))?;
        if state.epoch != epoch {
            *state = Listening {
                epoch,
                ..Default::default()
            };
        }
        let started = !state.started;
        let duration = if duration.is_finite() && duration > 0. {
            duration
        } else {
            song.duration
        };
        // Match the web engine: half the track, or four minutes, whichever comes first.
        let submit =
            !state.submitted && ((duration > 0. && position >= duration * 0.5) || position >= 240.);
        state.started = true;
        state.submitted |= submit;
        drop(state);
        drop(queue);
        if started || submit {
            let played_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_micros() as i64;
            std::thread::spawn(move || {
                if started {
                    self.record_recent(&song, played_at);
                    let _ = self.api(
                        "rest".into(),
                        "scrobble".into(),
                        "GET".into(),
                        json!({"id":song.id,"submission":false}).to_string(),
                    );
                }
                if submit {
                    let _ = self.api(
                        "rest".into(),
                        "scrobble".into(),
                        "GET".into(),
                        json!({"id":song.id,"submission":true}).to_string(),
                    );
                }
            });
        }
        Ok(())
    }

    pub fn recent_songs(&self) -> Result<String> {
        let db = self.db.lock().map_err(|_| err("Catalog unavailable"))?;
        let mut stmt = db
            .prepare("SELECT song FROM recent_songs ORDER BY played_at DESC, rowid DESC LIMIT 50")
            .map_err(|_| err("Cannot read recent songs"))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|_| err("Cannot read recent songs"))?;
        let values: Vec<Value> = rows
            .filter_map(|row| row.ok().and_then(|s| serde_json::from_str(&s).ok()))
            .collect();
        Ok(json!(values).to_string())
    }
}

impl NativeCore {
    fn record_recent(&self, song: &Song, played_at: i64) {
        let Ok(mut db) = self.db.lock() else { return };
        let Ok(tx) = db.transaction() else { return };
        if tx
            .execute(
                "INSERT INTO recent_songs(id,played_at,song) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET played_at=excluded.played_at,song=excluded.song WHERE excluded.played_at>=recent_songs.played_at",
                params![song.id, played_at, json!(song).to_string()],
            )
            .is_err()
        {
            return;
        }
        if tx.execute("DELETE FROM recent_songs WHERE id NOT IN (SELECT id FROM recent_songs ORDER BY played_at DESC, rowid DESC LIMIT 50)", []).is_err() { return; }
        let _ = tx.commit();
    }
}

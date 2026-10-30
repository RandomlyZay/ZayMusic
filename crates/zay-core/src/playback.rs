//! Persisted playback position. UI/audio engines report clocks; restore is always paused.
use crate::{err, NativeCore, Result};
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
#[uniffi::export]
impl NativeCore {
    pub fn save_playback_position(&self, epoch: u64, song_id: String, position: f64) -> Result<()> {
        if !position.is_finite() || position < 0. {
            return Ok(());
        }
        let queue = self.queue.lock().map_err(|_| err("Queue unavailable"))?;
        if epoch != self.playback_epoch.load(Ordering::SeqCst)
            || queue.current().is_none_or(|song| song.id != song_id)
        {
            return Ok(());
        }
        let value = json!({"id":song_id,"position":position});
        self.setting("playbackPosition".into(), Some(value.to_string()))?;
        *self
            .resume_position
            .lock()
            .map_err(|_| err("Playback unavailable"))? = value;
        Ok(())
    }
    pub fn playback_position(&self) -> Result<String> {
        Ok(self
            .resume_position
            .lock()
            .map_err(|_| err("Playback unavailable"))?
            .to_string())
    }
}
pub(crate) fn sanitize_error(detail: &str) -> String {
    let lower = detail.to_ascii_lowercase();
    if detail.chars().any(|c| c == '\t' || c == '\n' || c == '\r')
        || [
            "password=",
            "authorization:",
            "cookie:",
            "token=",
            "secret=",
        ]
        .iter()
        .any(|s| lower.contains(s))
    {
        return "The server rejected the request. Reconnect or retry.".into();
    }
    detail
        .split_whitespace()
        .map(|word| {
            let clean = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '/' && c != ':');
            if word.contains("http://") || word.contains("https://") {
                "[server URL]"
            } else if word.contains('@')
                || (clean.len() >= 24
                    && clean
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
            {
                "[redacted]"
            } else {
                word
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(300)
        .collect()
}

#[uniffi::export]
impl NativeCore {
    pub fn play_rows(
        &self,
        songs_json: String,
        index: u32,
        context_json: String,
    ) -> Result<String> {
        let songs: Value =
            serde_json::from_str(&songs_json).map_err(|_| err("Invalid song list"))?;
        let context: Value =
            serde_json::from_str(&context_json).map_err(|_| err("Invalid context"))?;
        self.record_context(context.to_string())?;
        let mut command = json!({"op":"set","songs":songs,"index":index});
        if ["albums", "newAlbums"].contains(&context["kind"].as_str().unwrap_or("")) {
            command["shuffleMode"] = json!("off");
        }
        self.queue_command(command.to_string())
    }
    pub fn play_collection(&self, songs_json: String, context_json: String) -> Result<String> {
        let context: Value =
            serde_json::from_str(&context_json).map_err(|_| err("Invalid context"))?;
        self.record_context(context.to_string())?;
        if ["playlists", "liked"].contains(&context["kind"].as_str().unwrap_or("")) {
            self.start_shuffle(songs_json, "off".into())?;
            self.queue_command("{}".into())
        } else {
            self.play_rows(songs_json, 0, context_json)
        }
    }
    pub fn play_context(&self, context_json: String) -> Result<String> {
        let context: Value =
            serde_json::from_str(&context_json).map_err(|_| err("Invalid context"))?;
        let page = if context["kind"] == "liked" {
            self.browse("Liked Songs".into(), String::new())?
        } else {
            self.open_collection(
                context["kind"].as_str().unwrap_or("").into(),
                context["id"].as_str().unwrap_or("").into(),
            )?
        };
        let page: Value = serde_json::from_str(&page).map_err(|_| err("Invalid collection"))?;
        self.play_collection(page["rows"].to_string(), context_json)
    }
}

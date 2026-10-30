//! Shared playlist mutations, including identity-preserving reorder and preview acquisition.
use crate::{catalog, err, NativeCore, Result};
use serde_json::{json, Value};
#[uniffi::export]
impl NativeCore {
    pub fn playlist_command(&self, command: String) -> Result<String> {
        let v: Value =
            serde_json::from_str(&command).map_err(|_| err("Invalid playlist action"))?;
        let _mutation = self
            .playlist_mutations
            .lock()
            .map_err(|_| err("Playlist unavailable"))?;
        let op = v["op"].as_str().unwrap_or("");
        let id = v["id"].as_str().unwrap_or("");
        let name = v["name"].as_str().unwrap_or("").trim();
        let post = |endpoint: &str, p: Value| -> Result<Value> {
            serde_json::from_str(&self.api(
                "rest".into(),
                endpoint.into(),
                "POST".into(),
                p.to_string(),
            )?)
            .map_err(|_| err("Invalid playlist response"))
        };
        let result = match op {
            "create" => {
                if name.is_empty() {
                    return Err(err("Choose a playlist name"));
                }
                let mut ids = v["songIds"].as_array().cloned().unwrap_or_default();
                let songs = v["songs"].as_array().cloned().unwrap_or_default();
                let mut previews = vec![];
                for song in songs {
                    if song["id"].as_str().is_some_and(|id| id.starts_with("yt:")) {
                        previews.push(song);
                    } else if song["id"].is_string() {
                        ids.push(song["id"].clone());
                    }
                }
                let mut created = post("createPlaylist", json!({"name":name,"songId":ids}))?;
                if !previews.is_empty() {
                    if let Some(key) = created["playlist"]["id"].as_str() {
                        match self.add_playlist_songs(key, &previews) {
                            Ok(result) => {
                                created["queued"] = result["queued"].clone();
                            }
                            Err(_) => {
                                created["warning"] = json!("Playlist created; preview download could not be queued. Add the song again to retry.");
                            }
                        }
                    }
                }
                created
            }
            "rename" => {
                if id.is_empty() || name.is_empty() {
                    return Err(err("Choose a playlist name"));
                }
                post("updatePlaylist", json!({"playlistId":id,"name":name}))?
            }
            "delete" => {
                if id.is_empty() {
                    return Err(err("Choose a playlist"));
                }
                let result = post("deletePlaylist", json!({"id":id}))?;
                if let Ok(text) = self.api(
                    "fetch".into(),
                    "youtube/imports".into(),
                    "GET".into(),
                    "{}".into(),
                ) {
                    if let Ok(imports) = serde_json::from_str::<Value>(&text) {
                        for item in catalog::list(&imports["imports"]) {
                            if item["playlistId"] == id {
                                if let Some(key) = item["id"].as_str() {
                                    let key = url::form_urlencoded::byte_serialize(key.as_bytes())
                                        .collect::<String>();
                                    let _ = self.api(
                                        "fetch".into(),
                                        format!("youtube/imports/{key}"),
                                        "DELETE".into(),
                                        "{}".into(),
                                    );
                                }
                            }
                        }
                    }
                }
                result
            }
            "add" => {
                if id.is_empty() {
                    return Err(err("Choose a playlist"));
                }
                let songs = v["songs"]
                    .as_array()
                    .ok_or_else(|| err("Choose songs to add"))?;
                self.add_playlist_songs(id, songs)?
            }
            "remove" | "move" => {
                if id.is_empty() {
                    return Err(err("Choose a playlist"));
                }
                let current: Value = serde_json::from_str(&self.api(
                    "rest".into(),
                    "getPlaylist".into(),
                    "GET".into(),
                    json!({"id":id}).to_string(),
                )?)
                .map_err(|_| err("Invalid playlist"))?;
                let mut songs = catalog::list(&current["playlist"]["entry"]);
                let from = v[if op == "remove" { "index" } else { "from" }]
                    .as_u64()
                    .ok_or_else(|| err("Invalid playlist position"))?
                    as usize;
                if from >= songs.len() {
                    return Err(err("Playlist changed. Refresh it before editing."));
                }
                if let Some(expected) = v["songId"].as_str() {
                    if songs[from]["id"] != expected {
                        return Err(err("Playlist changed. Refresh it before editing."));
                    }
                }
                if op == "remove" {
                    post(
                        "updatePlaylist",
                        json!({"playlistId":id,"songIndexToRemove":[from]}),
                    )?
                } else {
                    let to = v["to"]
                        .as_u64()
                        .ok_or_else(|| err("Invalid playlist position"))?
                        as usize;
                    if to >= songs.len() {
                        return Err(err("Invalid playlist position"));
                    }
                    if from != to {
                        let song = songs.remove(from);
                        songs.insert(to, song);
                        let ids: Vec<_> = songs.iter().map(|s| s["id"].clone()).collect();
                        post("createPlaylist", json!({"playlistId":id,"songId":ids}))?
                    } else {
                        json!({})
                    }
                }
            }
            _ => return Err(err("Unknown playlist action")),
        };
        let db = self.db.lock().map_err(|_| err("Catalog unavailable"))?;
        db.execute(
            "DELETE FROM kv WHERE key=?1 OR key=?2",
            [
                format!("browse:{}", json!(["Playlists", ""])),
                format!("collection:{}", json!(["playlists", id])),
            ],
        )
        .map_err(|_| err("Cannot refresh playlist cache"))?;
        if op == "delete" {
            db.execute(
                "DELETE FROM kv WHERE key=?1",
                [format!("context:playlists:{id}")],
            )
            .map_err(|_| err("Cannot update recent collections"))?;
        }
        Ok(result.to_string())
    }
}

impl NativeCore {
    fn add_playlist_songs(&self, id: &str, songs: &[Value]) -> Result<Value> {
        let mut ids = vec![];
        let mut queued = 0;
        for song in songs {
            let key = song["id"]
                .as_str()
                .ok_or_else(|| err("Invalid playlist song"))?;
            if let Some(video) = key.strip_prefix("yt:") {
                self.queue_acquisition(song.clone(),json!({"videoId":video,"playlistIds":[id],"overrides":{"title":song["title"],"artist":song["artist"],"album":song["album"],"track":song["track"],"year":song["year"]}}))?;
                queued += 1;
            } else {
                ids.push(key);
            }
        }
        if !ids.is_empty() {
            self.api(
                "rest".into(),
                "updatePlaylist".into(),
                "POST".into(),
                json!({"playlistId":id,"songIdToAdd":ids}).to_string(),
            )?;
        }
        Ok(json!({"added":ids.len(),"queued":queued}))
    }
}

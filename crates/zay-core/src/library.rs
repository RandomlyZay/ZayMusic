//! Shared library saves, deletion and recent collection shortcuts.
use crate::{catalog, err, NativeCore, Result};
use serde_json::{json, Value};
#[uniffi::export]
impl NativeCore {
    pub fn library_command(&self, command: String) -> Result<String> {
        let v: Value = serde_json::from_str(&command).map_err(|_| err("Invalid library action"))?;
        let item = &v["item"];
        let kind = v["kind"].as_str().unwrap_or("songs");
        let op = v["op"].as_str().unwrap_or("");
        let mut cache_edits = json!({});
        let result = match op {
            "save" | "unsave" => {
                let key = match kind {
                    "songs" => "id",
                    "albums" => "albumId",
                    "artists" => "artistId",
                    _ => return Err(err("Unsupported library item")),
                };
                let id = item["id"]
                    .as_str()
                    .filter(|s| !s.is_empty() && !s.starts_with("yt:"))
                    .ok_or_else(|| err("Choose a library item"))?;
                let response = self.api(
                    "rest".into(),
                    if op == "save" { "star" } else { "unstar" }.into(),
                    "GET".into(),
                    json!({key:id}).to_string(),
                )?;
                let starred = if op == "save" {
                    json!(time::OffsetDateTime::now_utc()
                        .format(&time::format_description::well_known::Rfc3339)
                        .unwrap_or_default())
                } else {
                    Value::Null
                };
                cache_edits = json!({"starred":starred});
                if kind == "songs" {
                    self.queue_command(
                        json!({"op":"updateMetadata","id":id,"edits":{"starred":starred}})
                            .to_string(),
                    )?;
                }
                response
            }
            "acquireAlbum" => {
                let id = item["browseId"]
                    .as_str()
                    .or(item["id"].as_str())
                    .ok_or_else(|| err("Choose an album"))?;
                let collection: Value =
                    serde_json::from_str(&self.open_collection("newAlbums".into(), id.into())?)
                        .map_err(|_| err("Invalid album"))?;
                let mut queued = 0;
                let mut failed = 0;
                for song in catalog::list(&collection["rows"]) {
                    let mut request = json!({"videoId":song["id"].as_str().and_then(|id|id.strip_prefix("yt:")).unwrap_or(""),"overrides":{"title":song["title"],"artist":song["artist"],"album":song["album"],"track":song["track"],"year":song["year"]}});
                    if let Some(id) = v["playlistId"].as_str().filter(|s| !s.is_empty()) {
                        request["playlistIds"] = json!([id]);
                    }
                    if self.queue_acquisition(song, request).is_ok() {
                        queued += 1
                    } else {
                        failed += 1
                    }
                }
                if failed == 0 {
                    self.api("fetch".into(),"album-saves".into(),"POST".into(),json!({"title":item["title"].as_str().or(item["name"].as_str()).unwrap_or(""),"artist":item["artist"].as_str().unwrap_or("")}).to_string())?;
                }
                json!({"queued":queued,"failed":failed}).to_string()
            }
            "editTags" => {
                let id = item["id"]
                    .as_str()
                    .filter(|s| !s.is_empty() && !s.starts_with("yt:"))
                    .ok_or_else(|| err("Choose a library song"))?;
                let mut edits = json!({});
                if let Some(values) = v["edits"].as_object() {
                    for (key, value) in values {
                        if ["title", "artist", "album", "track", "year", "genre"]
                            .contains(&key.as_str())
                        {
                            edits[key] = value.clone();
                        }
                    }
                }
                let endpoint = format!(
                    "music/{}/tags",
                    url::form_urlencoded::byte_serialize(id.as_bytes()).collect::<String>()
                );
                let mut rest = edits.clone();
                rest["id"] = json!(id);
                let response = self
                    .api("fetch".into(), endpoint, "POST".into(), edits.to_string())
                    .or_else(|_| {
                        self.api(
                            "rest".into(),
                            "updateSong".into(),
                            "POST".into(),
                            rest.to_string(),
                        )
                    })
                    .or_else(|_| {
                        self.api(
                            "rest".into(),
                            "updateSong".into(),
                            "GET".into(),
                            rest.to_string(),
                        )
                    })?;
                self.queue_command(
                    json!({"op":"updateMetadata","id":id,"edits":edits}).to_string(),
                )?;
                cache_edits = edits;
                response
            }
            "deleteSong" => {
                let id = item["id"]
                    .as_str()
                    .filter(|s| !s.is_empty() && !s.starts_with("yt:"))
                    .ok_or_else(|| err("Choose a library song"))?;
                let path = url::form_urlencoded::byte_serialize(id.as_bytes()).collect::<String>();
                let result = self.api(
                    "fetch".into(),
                    format!("music/{path}"),
                    "DELETE".into(),
                    "{}".into(),
                )?;
                self.db
                    .lock()
                    .map_err(|_| err("Catalog unavailable"))?
                    .execute("DELETE FROM recent_songs WHERE id=?1", [id])
                    .map_err(|_| err("Cannot update history"))?;
                let snapshot: Value = serde_json::from_str(&self.queue_command("{}".into())?)
                    .map_err(|_| err("Invalid queue"))?;
                for (field, operation) in
                    [("queue", "remove"), ("secondaryQueue", "removeSecondary")]
                {
                    for (index, song) in catalog::list(&snapshot["state"][field])
                        .iter()
                        .enumerate()
                        .rev()
                    {
                        if song["id"] == id {
                            self.queue_command(json!({"op":operation,"index":index}).to_string())?;
                        }
                    }
                }
                result
            }
            _ => return Err(err("Unknown library action")),
        };
        self.update_library_cache(item, kind, &cache_edits, op == "deleteSong", op == "unsave")?;
        Ok(result)
    }
    pub fn record_context(&self, context_json: String) -> Result<()> {
        let mut v: Value =
            serde_json::from_str(&context_json).map_err(|_| err("Invalid play context"))?;
        let kind = v["kind"].as_str().or(v["type"].as_str()).unwrap_or("");
        let kind = match kind {
            "albums" | "album" => "albums",
            "playlists" | "playlist" => "playlists",
            "liked" => "liked",
            _ => return Ok(()),
        }
        .to_owned();
        let id = v["id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| err("Invalid play context"))?
            .to_owned();
        v["playedAt"] =
            json!((time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64);
        v["kind"] = json!(kind);
        v["id"] = json!(id);
        self.setting(format!("context:{kind}:{id}"), Some(v.to_string()))?;
        // REPLACE gives a fresh rowid and therefore stable most-recent ordering.
        self.db.lock().map_err(|_|err("Catalog unavailable"))?.execute("DELETE FROM kv WHERE key LIKE 'context:%' AND key NOT IN (SELECT key FROM kv WHERE key LIKE 'context:%' ORDER BY rowid DESC LIMIT 100)",[]).map_err(|_|err("Cannot trim history"))?;
        Ok(())
    }
    pub fn recent_contexts(&self) -> Result<String> {
        let db = self.db.lock().map_err(|_| err("Catalog unavailable"))?;
        let mut stmt = db
            .prepare(
                "SELECT value FROM kv WHERE key LIKE 'context:%' ORDER BY rowid DESC LIMIT 100",
            )
            .map_err(|_| err("Cannot read contexts"))?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|_| err("Cannot read contexts"))?;
        let contexts: Vec<Value> = rows
            .filter_map(|r| r.ok().and_then(|s| serde_json::from_str(&s).ok()))
            .collect();
        Ok(json!(contexts).to_string())
    }
}

fn stamp(value: &Value) -> i64 {
    value.as_i64().unwrap_or_else(|| {
        value
            .as_str()
            .and_then(|s| {
                time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
            })
            .map(|d| (d.unix_timestamp_nanos() / 1_000_000) as i64)
            .unwrap_or(0)
    })
}
#[uniffi::export]
impl NativeCore {
    pub fn metadata_lookup(&self, query: String) -> Result<String> {
        self.api(
            "fetch".into(),
            "metadata/lookup".into(),
            "GET".into(),
            json!({"q":query}).to_string(),
        )
    }
}
impl NativeCore {
    pub(crate) fn library_rows(&self, query: &str) -> Result<Value> {
        let options: Value = serde_json::from_str(query).unwrap_or_else(|_| json!({"text":query}));
        let filter = options["filter"].as_str().unwrap_or("all");
        let sort = options["sort"].as_str().unwrap_or("recent");
        let text = options["text"].as_str().unwrap_or("").to_lowercase();
        let starred: Value = serde_json::from_str(&self.api(
            "rest".into(),
            "getStarred2".into(),
            "GET".into(),
            "{}".into(),
        )?)
        .map_err(|_| err("Invalid library"))?;
        let playlists: Value = serde_json::from_str(&self.api(
            "rest".into(),
            "getPlaylists".into(),
            "GET".into(),
            "{}".into(),
        )?)
        .map_err(|_| err("Invalid playlists"))?;
        let contexts = catalog::list(
            &serde_json::from_str::<Value>(&self.recent_contexts()?)
                .map_err(|_| err("Invalid contexts"))?,
        );
        let recent = catalog::list(
            &serde_json::from_str::<Value>(&self.recent_songs()?)
                .map_err(|_| err("Invalid history"))?,
        );
        let mut rows = vec![
            json!({"id":"liked","name":"Liked Songs","browseKind":"liked","songCount":catalog::list(&starred["starred2"]["song"]).len(),"addedAt":i64::MAX,"playedAt":0}),
        ];
        for (kind, items) in [
            (
                "playlists",
                catalog::list(&playlists["playlists"]["playlist"]),
            ),
            ("albums", catalog::list(&starred["starred2"]["album"])),
            ("artists", catalog::list(&starred["starred2"]["artist"])),
        ] {
            if filter != "all" && filter != kind {
                continue;
            }
            for mut item in items {
                item["browseKind"] = json!(kind);
                item["addedAt"] = json!(stamp(if kind == "playlists" {
                    &item["created"]
                } else {
                    &item["starred"]
                }));
                let context = contexts
                    .iter()
                    .find(|c| c["kind"] == kind && c["id"] == item["id"]);
                let played = context.map(|c| stamp(&c["playedAt"])).unwrap_or(0);
                let artist_play = if kind == "artists" {
                    recent
                        .iter()
                        .position(|s| s["artistId"] == item["id"])
                        .map(|i| time::OffsetDateTime::now_utc().unix_timestamp() * 1000 - i as i64)
                        .unwrap_or(0)
                } else {
                    0
                };
                item["playedAt"] = json!(played.max(artist_play));
                rows.push(item);
            }
        }
        rows.retain(|row| {
            text.is_empty()
                || format!(
                    "{} {}",
                    row["name"].as_str().unwrap_or(""),
                    row["artist"].as_str().unwrap_or("")
                )
                .to_lowercase()
                .contains(&text)
        });
        rows.sort_by(|a, b| {
            if a["browseKind"] == "liked" {
                return std::cmp::Ordering::Less;
            }
            if b["browseKind"] == "liked" {
                return std::cmp::Ordering::Greater;
            }
            let rank = |v: &Value| {
                if sort == "added" {
                    stamp(&v["addedAt"])
                } else {
                    stamp(&v["addedAt"]).max(stamp(&v["playedAt"]))
                }
            };
            (if sort == "name" {
                std::cmp::Ordering::Equal
            } else {
                rank(b).cmp(&rank(a))
            })
            .then_with(|| {
                a["name"]
                    .as_str()
                    .unwrap_or("")
                    .to_lowercase()
                    .cmp(&b["name"].as_str().unwrap_or("").to_lowercase())
            })
        });
        Ok(json!({"rows":rows,"extra":options}))
    }
    fn update_library_cache(
        &self,
        item: &Value,
        kind: &str,
        edits: &Value,
        deleting: bool,
        unsave: bool,
    ) -> Result<()> {
        let id = item["id"].as_str().unwrap_or("");
        let db = self.db.lock().map_err(|_| err("Catalog unavailable"))?;
        let cached = {
            let mut stmt = db
                .prepare(
                    "SELECT key,value FROM kv WHERE key LIKE 'browse:%' OR key LIKE 'collection:%'",
                )
                .map_err(|_| err("Cannot read cache"))?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .map_err(|_| err("Cannot read cache"))?;
            rows.filter_map(|r| r.ok()).collect::<Vec<_>>()
        };
        for (key, text) in cached {
            let Ok(mut value) = serde_json::from_str::<Value>(&text) else {
                db.execute("DELETE FROM kv WHERE key=?1", [key])
                    .map_err(|_| err("Cannot update cache"))?;
                continue;
            };
            let owned = matches!(
                (value["title"].as_str().unwrap_or(""), kind),
                ("Liked Songs", "songs") | ("Albums", "albums") | ("Artists", "artists")
            );
            if let Some(rows) = value["rows"].as_array_mut() {
                if deleting || (owned && unsave) {
                    rows.retain(|row| row["id"] != id);
                }
                for row in rows.iter_mut().filter(|row| row["id"] == id) {
                    if let Some(patch) = edits.as_object() {
                        for (key, value) in patch {
                            row[key] = value.clone();
                        }
                    }
                }
                if owned && !unsave && !deleting && !rows.iter().any(|row| row["id"] == id) {
                    let mut row = item.clone();
                    if let Some(patch) = edits.as_object() {
                        for (key, value) in patch {
                            row[key] = value.clone();
                        }
                    }
                    rows.insert(0, row);
                }
            }
            if value["context"]["id"] == id {
                if let Some(patch) = edits.as_object() {
                    for (key, item) in patch {
                        value["context"][key] = item.clone();
                    }
                }
            }
            if deleting {
                value["extra"]["sections"] = json!([]);
            }
            db.execute(
                "UPDATE kv SET value=?1 WHERE key=?2",
                rusqlite::params![value.to_string(), key],
            )
            .map_err(|_| err("Cannot update cache"))?;
        }
        for (table, column) in [("recent_songs", "song"), ("downloads", "meta")] {
            let sql = format!("SELECT {column} FROM {table} WHERE id=?1");
            if let Ok(text) = db.query_row(&sql, [id], |r| r.get::<_, String>(0)) {
                if let Ok(mut value) = serde_json::from_str::<Value>(&text) {
                    if let Some(patch) = edits.as_object() {
                        for (key, item) in patch {
                            value[key] = item.clone();
                        }
                    }
                    db.execute(
                        &format!("UPDATE {table} SET {column}=?1 WHERE id=?2"),
                        rusqlite::params![value.to_string(), id],
                    )
                    .map_err(|_| err("Cannot update local metadata"))?;
                }
            }
        }
        Ok(())
    }
}

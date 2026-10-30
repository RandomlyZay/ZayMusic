//! Platform-independent navigation, discovery and wire normalization.
use crate::{err, model::Song, NativeCore, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Input {
    Empty,
    LibraryQuery { query: String },
    VideoUrl { url: String },
    PlaylistUrl { url: String },
    AlbumUrl { url: String },
}
pub fn classify(raw: &str) -> Input {
    let text = raw.trim();
    if text.is_empty() {
        return Input::Empty;
    }
    let query = || Input::LibraryQuery { query: text.into() };
    let Ok(url) = url::Url::parse(&if text.starts_with("http") {
        text.into()
    } else {
        format!("https://{text}")
    }) else {
        return query();
    };
    let host = url.host_str().unwrap_or("");
    let bare = host
        .strip_prefix("www.")
        .or_else(|| host.strip_prefix("m."))
        .or_else(|| host.strip_prefix("music."))
        .unwrap_or(host);
    if !["youtube.com", "youtu.be"].contains(&bare) {
        return query();
    }
    let has = |key: &str| url.query_pairs().any(|(k, v)| k == key && !v.is_empty());
    let result = url.to_string();
    if bare == "youtu.be" {
        return Input::VideoUrl { url: result };
    }
    if (url.path() == "/playlist" && has("list"))
        || (url.path() == "/watch" && has("v") && has("list"))
    {
        return Input::PlaylistUrl { url: result };
    }
    if host.starts_with("music.") && url.path().starts_with("/browse/") {
        return Input::AlbumUrl { url: result };
    }
    if url.path().starts_with("/shorts/") || url.path().starts_with("/live/") || has("v") {
        return Input::VideoUrl { url: result };
    }
    if has("list") {
        return Input::PlaylistUrl { url: result };
    }
    query()
}
pub fn list(value: &Value) -> Vec<Value> {
    match value {
        Value::Array(a) => a.clone(),
        Value::Object(_) => vec![value.clone()],
        _ => vec![],
    }
}
pub fn preview(candidate: &Value) -> Value {
    let video = candidate["videoId"].as_str().unwrap_or("");
    let mut params = url::form_urlencoded::Serializer::new(String::new());
    params.append_pair("artist", candidate["channel"].as_str().unwrap_or(""));
    params.append_pair("title", candidate["title"].as_str().unwrap_or(""));
    params.append_pair(
        "duration",
        &candidate["durationSec"].as_f64().unwrap_or(0.).to_string(),
    );
    json!({"id":format!("yt:{video}"),"title":candidate["title"].as_str().unwrap_or(""),"artist":candidate["channel"].as_str().unwrap_or(""),"album":candidate["album"].as_str().unwrap_or(""),"track":candidate["track"],"duration":candidate["durationSec"].as_f64().unwrap_or(0.),"previewUrl":format!("/api/fetch/preview/{video}?{}",params.finish()),"artworkUrl":candidate["thumbnails"].as_array().and_then(|t|t.last()).and_then(|t|t["url"].as_str()),"candidate":candidate})
}

impl NativeCore {
    // Server shelves can be cached; listening history and shortcuts are always local/current.
    fn refresh_home_local(&self, page: &mut Value, online: bool) -> Result<()> {
        let old_rows = list(&page["rows"]);
        let old_sections = list(&page["extra"]["sections"]);
        let mut rows = list(
            &serde_json::from_str::<Value>(&self.recent_songs()?)
                .map_err(|_| err("Invalid history"))?,
        );
        let mut sections = vec![];
        if !rows.is_empty() {
            sections.push(json!({"title":"Recently played","start":0,"count":rows.len()}));
        }
        for section in old_sections {
            if section["title"] == "Recently played" {
                continue;
            }
            let start = section["start"].as_u64().unwrap_or(0) as usize;
            let count = section["count"].as_u64().unwrap_or(0) as usize;
            let shelf: Vec<_> = old_rows.iter().skip(start).take(count).cloned().collect();
            if !shelf.is_empty() {
                sections
                    .push(json!({"title":section["title"],"start":rows.len(),"count":shelf.len()}));
                rows.extend(shelf);
            }
        }
        let mut contexts = list(
            &serde_json::from_str::<Value>(&self.recent_contexts()?)
                .map_err(|_| err("Invalid contexts"))?,
        );
        contexts.truncate(8);
        if contexts.len() < 8 && !contexts.iter().any(|c| c["kind"] == "liked") {
            contexts.push(json!({"id":"liked","kind":"liked","name":"Liked Songs"}));
        }
        let playlists = if online && contexts.len() < 8 {
            self.browse("Playlists".into(), String::new())
                .unwrap_or_default()
        } else {
            self.setting(format!("browse:{}", json!(["Playlists", ""])), None)?
        };
        if let Ok(playlists) = serde_json::from_str::<Value>(&playlists) {
            for mut playlist in list(&playlists["rows"]) {
                if contexts.len() >= 8 {
                    break;
                }
                if !contexts
                    .iter()
                    .any(|c| c["kind"] == "playlists" && c["id"] == playlist["id"])
                {
                    playlist["kind"] = json!("playlists");
                    contexts.push(playlist);
                }
            }
        }
        page["rows"] = json!(rows);
        page["extra"]["sections"] = json!(sections);
        page["extra"]["contexts"] = json!(contexts);
        Ok(())
    }
}

#[uniffi::export]
impl NativeCore {
    /// A common view model for both native UIs. Successful pages are cached for offline browsing.
    pub fn browse(&self, page: String, query: String) -> Result<String> {
        let key = format!("browse:{}", json!([page, query]));
        // Device codes and cookie/session status belong in memory, never the page cache.
        if page == "Settings" {
            self.db
                .lock()
                .map_err(|_| err("Catalog unavailable"))?
                .execute("DELETE FROM kv WHERE key=?1", [&key])
                .map_err(|_| err("Cannot clear settings cache"))?;
        }
        let result = (|| -> Result<Value> {
            let call = |service: &str, endpoint: &str, p: Value| -> Result<Value> {
                serde_json::from_str(&self.api(
                    service.into(),
                    endpoint.into(),
                    "GET".into(),
                    p.to_string(),
                )?)
                .map_err(|_| err("Invalid catalog response"))
            };
            let mut kind = "songs";
            let mut extra = json!({});
            let rows = match page.as_str() {
                "Home" => {
                    let v = call("fetch", "home", json!({}))?;
                    let mut rows = vec![];
                    let mut sections = vec![];
                    let recent = list(
                        &serde_json::from_str::<Value>(&self.recent_songs()?)
                            .map_err(|_| err("Invalid listening history"))?,
                    );
                    if !recent.is_empty() {
                        sections.push(
                            json!({"title":"Recently played","start":0,"count":recent.len()}),
                        );
                        rows.extend(recent);
                    }
                    for (title, field) in
                        [("Your top songs", "topSongs"), ("Rediscover", "rediscover")]
                    {
                        let songs = list(&v[field]);
                        if !songs.is_empty() {
                            sections.push(
                                json!({"title":title,"start":rows.len(),"count":songs.len()}),
                            );
                            rows.extend(songs);
                        }
                    }
                    extra = json!({"sections":sections,"contexts":serde_json::from_str::<Value>(&self.recent_contexts()?).map_err(|_|err("Invalid contexts"))?});
                    rows
                }
                "Recent Collections" => {
                    kind = "contexts";
                    list(
                        &serde_json::from_str::<Value>(&self.recent_contexts()?)
                            .map_err(|_| err("Invalid contexts"))?,
                    )
                }
                "Recent Songs" => list(
                    &serde_json::from_str::<Value>(&self.recent_songs()?)
                        .map_err(|_| err("Invalid listening history"))?,
                ),
                "Library" => {
                    kind = "library";
                    let library = self.library_rows(&query)?;
                    extra = library["extra"].clone();
                    list(&library["rows"])
                }
                "Liked Songs" | "Albums" | "Artists" => {
                    let v = call("rest", "getStarred2", json!({}))?;
                    let field = match page.as_str() {
                        "Albums" => {
                            kind = "albums";
                            "album"
                        }
                        "Artists" => {
                            kind = "artists";
                            "artist"
                        }
                        _ => "song",
                    };
                    let mut rows = list(&v["starred2"][field]);
                    rows.sort_by(|a, b| {
                        let undated = if page == "Liked Songs" {
                            "\u{10ffff}"
                        } else {
                            ""
                        };
                        b["starred"]
                            .as_str()
                            .unwrap_or(undated)
                            .cmp(a["starred"].as_str().unwrap_or(undated))
                    });
                    rows
                }
                "All Albums" | "Recently Added" | "Recently Played" => {
                    kind = "albums";
                    let t = match page.as_str() {
                        "Recently Played" => "recent",
                        "All Albums" => "alphabeticalByName",
                        _ => "newest",
                    };
                    list(
                        &call("rest", "getAlbumList2", json!({"type":t,"size":500}))?["albumList2"]
                            ["album"],
                    )
                }
                "All Artists" => {
                    kind = "artists";
                    list(&call("rest", "getArtists", json!({}))?["artists"]["index"])
                        .iter()
                        .flat_map(|index| list(&index["artist"]))
                        .collect()
                }
                "Genres" => {
                    kind = "genres";
                    list(&call("rest", "getGenres", json!({}))?["genres"]["genre"])
                }
                "Playlists" => {
                    kind = "playlists";
                    let mut rows =
                        list(&call("rest", "getPlaylists", json!({}))?["playlists"]["playlist"]);
                    let contexts = list(
                        &serde_json::from_str::<Value>(&self.recent_contexts()?)
                            .map_err(|_| err("Invalid contexts"))?,
                    );
                    rows.sort_by_key(|row| {
                        contexts
                            .iter()
                            .position(|c| c["kind"] == "playlists" && c["id"] == row["id"])
                            .unwrap_or(usize::MAX)
                    });
                    rows
                }
                "Downloads" => {
                    extra = serde_json::from_str(&self.storage_stats()?)
                        .map_err(|_| err("Invalid storage totals"))?;
                    list(
                        &serde_json::from_str::<Value>(&self.downloads()?)
                            .map_err(|_| err("Invalid offline catalog"))?,
                    )
                    .into_iter()
                    .map(|r| {
                        let mut s = r["song"].clone();
                        s["download"] = r;
                        s
                    })
                    .collect()
                }
                "Queue" => {
                    let v: Value = serde_json::from_str(&self.queue_command("{}".into())?)
                        .map_err(|_| err("Invalid queue"))?;
                    let mut rows = vec![];
                    let priority = list(&v["state"]["secondaryQueue"]);
                    for (index, mut row) in priority.into_iter().enumerate() {
                        row["queueLane"] = json!("priority");
                        row["queueIndex"] = json!(index);
                        rows.push(row);
                    }
                    let main = list(&v["state"]["queue"]);
                    let indices: Vec<usize> = if v["state"]["shuffle"] == true {
                        list(&v["state"]["shuffleOrder"])
                            .into_iter()
                            .skip((v["state"]["shufflePos"].as_i64().unwrap_or(-1) + 1).max(0)
                                as usize)
                            .filter_map(|i| i.as_u64().map(|i| i as usize))
                            .collect()
                    } else {
                        ((v["state"]["index"].as_i64().unwrap_or(-1) + 1).max(0) as usize
                            ..main.len())
                            .collect()
                    };
                    for index in indices {
                        if let Some(mut row) = main.get(index).cloned() {
                            row["queueLane"] = json!("main");
                            row["queueIndex"] = json!(index);
                            rows.push(row);
                        }
                    }

                    rows
                }
                "Jobs" => {
                    kind = "jobs";
                    let mut rows = list(
                        &serde_json::from_str::<Value>(&self.acquisition_failures()?)
                            .map_err(|_| err("Invalid failures"))?,
                    );
                    if let Ok(v) = call("fetch", "jobs", json!({"limit":200})) {
                        self.record_server_jobs(&v["jobs"]);
                        rows.extend(list(&v["jobs"]));
                    } else if rows.is_empty() {
                        return Err(err("Server unreachable"));
                    }
                    for row in &mut rows {
                        row["name"] = row["overrides"]["title"]
                            .as_str()
                            .or(row["result"]["title"].as_str())
                            .or(row["name"].as_str())
                            .unwrap_or("Download")
                            .into();
                    }
                    rows
                }
                "Imports" => {
                    kind = "imports";
                    list(&call("fetch", "youtube/imports", json!({}))?["imports"])
                }
                "YouTube Playlists" => {
                    kind = "youtubePlaylists";
                    list(&call("fetch", "youtube/library/playlists", json!({}))?["playlists"])
                }
                "Settings" => {
                    kind = "settings";
                    extra = serde_json::from_str(&self.management_status()?)
                        .map_err(|_| err("Invalid settings"))?;
                    vec![]
                }
                "Search" | "New music" | "Songs" | "Omnibar" => match classify(&query) {
                    Input::Empty if page != "Songs" => {
                        let rows = list(
                            &serde_json::from_str::<Value>(&self.recent_songs()?)
                                .map_err(|_| err("Invalid history"))?,
                        );
                        if !rows.is_empty() {
                            extra = json!({"sections":[{"title":"Recently played","start":0,"count":rows.len()}]});
                        }
                        rows
                    }
                    Input::VideoUrl { url }
                    | Input::PlaylistUrl { url }
                    | Input::AlbumUrl { url } => {
                        let v = call("fetch", "resolve", json!({"url":url}))?;
                        extra = v.clone();
                        list(&v["candidates"]).iter().map(preview).collect()
                    }
                    _ => {
                        let results = self.search_catalog(&page, &query)?;
                        extra = results["extra"].clone();
                        list(&results["rows"])
                    }
                },
                _ => return Err(err("Unknown catalog page")),
            };
            Ok(json!({"title":page,"kind":kind,"rows":rows,"extra":extra,"offline":false}))
        })();
        match result {
            Ok(value) => {
                let mut value = value;
                if page == "Home" {
                    self.refresh_home_local(&mut value, true)?;
                }
                let text = value.to_string();
                if page != "Settings" {
                    self.setting(key, Some(text.clone()))?;
                }
                Ok(text)
            }
            Err(error) => {
                let cached = self.setting(key, None)?;
                if let Ok(mut value) = serde_json::from_str::<Value>(&cached) {
                    if value.is_object() {
                        if page == "Home" {
                            self.refresh_home_local(&mut value, false)?;
                        }
                        value["offline"] = json!(true);
                        return Ok(value.to_string());
                    }
                }
                Err(error)
            }
        }
    }
    pub fn open_collection(&self, kind: String, id: String) -> Result<String> {
        let key = format!("collection:{}", json!([kind, id]));
        let result = (|| -> Result<String> {
            let (service, endpoint, params, container, field, next) = match kind.as_str() {
                "albums" => (
                    "rest",
                    "getAlbum",
                    json!({"id":id}),
                    "album",
                    "song",
                    "songs",
                ),
                "artists" => (
                    "rest",
                    "getArtist",
                    json!({"id":id}),
                    "artist",
                    "album",
                    "albums",
                ),
                "playlists" => (
                    "rest",
                    "getPlaylist",
                    json!({"id":id}),
                    "playlist",
                    "entry",
                    "songs",
                ),
                "genres" => (
                    "rest",
                    "getSongsByGenre",
                    json!({"genre":id,"count":500}),
                    "songList",
                    "song",
                    "songs",
                ),
                "newAlbums" => (
                    "fetch",
                    "album",
                    json!({"browseId":id}),
                    "",
                    "candidates",
                    "songs",
                ),
                _ => return Err(err("Unknown collection type")),
            };
            let v: Value = serde_json::from_str(&self.api(
                service.into(),
                endpoint.into(),
                "GET".into(),
                params.to_string(),
            )?)
            .map_err(|_| err("Invalid collection"))?;
            let mut rows = if container.is_empty() {
                list(&v[field]).iter().map(preview).collect()
            } else {
                list(&v[container][field])
            };
            if kind == "newAlbums" {
                for (index, row) in rows.iter_mut().enumerate() {
                    row["album"] = v["album"]["title"].clone();
                    if row["track"].is_null() {
                        row["track"] = json!(index + 1);
                    }
                }
            }
            if kind == "playlists" {
                for (index, row) in rows.iter_mut().enumerate() {
                    row["playlistId"] = json!(id);
                    row["playlistIndex"] = json!(index);
                }
            }
            let title = v[container]["name"]
                .as_str()
                .or(v["album"]["title"].as_str())
                .unwrap_or("Tracks");
            let source = if container.is_empty() {
                &v["album"]
            } else {
                &v[container]
            };
            let artwork = source["coverArt"].clone();
            let artwork_url = rows
                .first()
                .map(|r| r["artworkUrl"].clone())
                .unwrap_or(Value::Null);
            // Header facts come only from the server response; absent fields stay null.
            let context = json!({"kind":kind,"id":id,"name":title,"coverArt":artwork,
                "artworkUrl":if artwork.is_null() { artwork_url } else { Value::Null },
                "artist":source["artist"],"year":source["year"],"owner":source["owner"],
                "starred":source["starred"],"songCount":rows.len(),"duration":rows.iter().filter_map(|r| r["duration"].as_f64()).sum::<f64>()});
            Ok(
                json!({"title":title,"kind":next,"rows":rows,"offline":false,"context":context})
                    .to_string(),
            )
        })();
        match result {
            Ok(text) => {
                self.setting(key, Some(text.clone()))?;
                Ok(text)
            }
            Err(error) => {
                let value = self.setting(key, None)?;
                if value != "null" {
                    let mut v: Value = serde_json::from_str(&value)
                        .map_err(|_| err("Invalid cached collection"))?;
                    v["offline"] = json!(true);
                    Ok(v.to_string())
                } else {
                    Err(error)
                }
            }
        }
    }
    pub fn acquire_song(&self, song_json: String) -> Result<String> {
        let song: Song = serde_json::from_str(&song_json).map_err(|_| err("Invalid song"))?;
        let video = song
            .id
            .strip_prefix("yt:")
            .ok_or_else(|| err("Song is already in the library"))?;
        self.queue_acquisition(serde_json::from_str(&song_json).map_err(|_|err("Invalid acquisition song"))?,json!({"videoId":video,"overrides":{"title":song.title,"artist":song.artist,"album":song.album,"track":song.track,"year":song.year}}))
    }

    pub fn warm_upcoming(&self) -> Result<()> {
        let v: Value = serde_json::from_str(&self.queue_command("{}".into())?)
            .map_err(|_| err("Invalid queue"))?;
        let ids: Vec<_> = list(&v["upcoming"])
            .iter()
            .filter_map(|s| {
                s["id"]
                    .as_str()
                    .and_then(|s| s.strip_prefix("yt:"))
                    .map(str::to_owned)
            })
            .take(2)
            .collect();
        if !ids.is_empty() {
            self.api(
                "fetch".into(),
                "preview/warm".into(),
                "POST".into(),
                json!({"videoIds":ids}).to_string(),
            )?;
        }
        Ok(())
    }
}

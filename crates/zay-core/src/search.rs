//! Mixed omnibar results and common playable-row filtering.
use crate::{
    catalog::{list, preview},
    err,
    model::Song,
    NativeCore, Result,
};
use serde_json::{json, Value};
use unicode_normalization::UnicodeNormalization;
pub fn rank_library(rows: &mut [Value], query: &str) {
    let normalize = |s: &str| {
        s.nfd()
            .filter(|c| !('\u{0300}'..='\u{036f}').contains(c))
            .collect::<String>()
            .to_lowercase()
            .trim()
            .to_owned()
    };
    let q = normalize(query);
    let words: Vec<_> = q.split_whitespace().collect();
    let relevance = |row: &Value| {
        let name = row["title"].as_str().or(row["name"].as_str()).unwrap_or("");
        let primary = normalize(name);
        let combined = normalize(&format!(
            "{} {}",
            row["artist"].as_str().unwrap_or(""),
            name
        ));
        (if combined == q {
            125
        } else if primary == q {
            100
        } else if primary.starts_with(&q) {
            60
        } else if primary.contains(&q) {
            40
        } else {
            0
        }) + if combined.contains(&q) { 25 } else { 0 }
            + words
                .iter()
                .filter(|word| combined.contains(**word))
                .count() as i32
                * 5
    };
    rows.sort_by_key(|row| std::cmp::Reverse(relevance(row)));
}
pub(crate) fn playable(value: Value) -> Result<Vec<Song>> {
    value
        .as_array()
        .ok_or_else(|| err("Invalid song list"))?
        .iter()
        .filter(|s| s["browseKind"].as_str().is_none_or(|kind| kind == "songs"))
        .map(|s| serde_json::from_value(s.clone()).map_err(|_| err("Invalid song")))
        .collect()
}
impl NativeCore {
    /// Page each result type independently; a large library must not disappear after 500 songs.
    fn library_search(&self, query: &str) -> Result<Value> {
        const BATCH: usize = 40;
        let mut result = json!({"song":[],"album":[],"artist":[]});
        let mut offsets = [0usize; 3];
        let mut done = [false; 3];
        let mut seen: [std::collections::HashSet<String>; 3] = Default::default();
        while done.iter().any(|finished| !finished) {
            let text = self.api(
                "rest".into(),
                "search3".into(),
                "GET".into(),
                json!({
                    "query":query,
                    "songCount":if done[0]{0}else{BATCH},"songOffset":offsets[0],
                    "albumCount":if done[1]{0}else{BATCH},"albumOffset":offsets[1],
                    "artistCount":if done[2]{0}else{BATCH},"artistOffset":offsets[2]
                })
                .to_string(),
            )?;
            let value: Value =
                serde_json::from_str(&text).map_err(|_| err("Invalid search response"))?;
            for (i, field) in ["song", "album", "artist"].iter().enumerate() {
                if done[i] {
                    continue;
                }
                let batch = list(&value["searchResult3"][field]);
                let count = batch.len();
                let mut added = 0;
                for row in batch {
                    let key = row["id"]
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| row.to_string());
                    if seen[i].insert(key) {
                        result[field].as_array_mut().unwrap().push(row);
                        added += 1;
                    }
                }
                offsets[i] += count;
                // Also tolerate a server that ignores offsets, without looping forever.
                done[i] = count < BATCH || added == 0;
            }
        }
        Ok(result)
    }
    pub(crate) fn search_catalog(&self, page: &str, query: &str) -> Result<Value> {
        let mut rows = vec![];
        let mut sections = vec![];
        let mut extra = json!({});
        let mut successes = 0;
        let mut append = |title: &str, mut values: Vec<Value>, kind: &str| {
            if ["songs", "albums", "artists"].contains(&kind) && title != "New music" {
                rank_library(&mut values, query);
            }
            if values.is_empty() {
                return;
            }
            let id = match title {
                "In your library" => "librarySongs",
                "Albums" => "libraryAlbums",
                "Artists" => "libraryArtists",
                "New music" => "newSongs",
                _ => "newAlbums",
            };
            sections.push(json!({"id":id,"title":title,"start":rows.len(),"count":values.len()}));
            for mut value in values {
                if kind != "songs" {
                    value["browseKind"] = json!(kind);
                    value["title"] = value["name"]
                        .as_str()
                        .or(value["title"].as_str())
                        .unwrap_or("")
                        .into();
                }
                rows.push(value);
            }
        };
        if page != "New music" {
            if let Ok(v) = self.library_search(query) {
                successes += 1;
                append("In your library", list(&v["song"]), "songs");
                if page != "Songs" {
                    append("Albums", list(&v["album"]), "albums");
                    append("Artists", list(&v["artist"]), "artists");
                }
            }
        }
        if page != "Search" && page != "Songs" && !query.trim().is_empty() {
            if let Ok(text) = self.api(
                "fetch".into(),
                "search".into(),
                "GET".into(),
                json!({"q":query,"limit":50,"source":"all"}).to_string(),
            ) {
                let v: Value =
                    serde_json::from_str(&text).map_err(|_| err("Invalid search response"))?;
                successes += 1;
                extra = v.clone();
                append(
                    "New music",
                    list(&v["candidates"]).iter().map(preview).collect(),
                    "songs",
                );
                let albums = list(&v["albums"])
                    .into_iter()
                    .map(|mut a| {
                        a["id"] = a["browseId"].clone();
                        a["artworkUrl"] = a["thumbnails"]
                            .as_array()
                            .and_then(|t| t.last())
                            .map(|t| t["url"].clone())
                            .unwrap_or(Value::Null);
                        a
                    })
                    .collect();
                append("New albums", albums, "newAlbums");
            }
        }
        if successes == 0 {
            return Err(err("Search unavailable"));
        }
        extra["sections"] = json!(sections);
        Ok(json!({"rows":rows,"extra":extra}))
    }
}

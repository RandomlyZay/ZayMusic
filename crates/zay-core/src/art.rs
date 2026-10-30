//! Cover art resolution and a bounded on-disk cache shared by both native clients.
//!
//! Artwork goes through the existing gateway (`/rest/getCoverArt`), which injects
//! credentials server-side, or a YouTube thumbnail already present on a preview.
//! Cached files keep collections and offline pages looking the same without a network.
use crate::{err, NativeCore, Result};
use serde_json::Value;
use std::io::Read;

/// Upper bound on cached artwork files. Small square covers are tens of kilobytes.
const CACHE_LIMIT: usize = 1500;
const MAX_BYTES: u64 = 5 * 1024 * 1024;

/// Stable FNV-1a; `DefaultHasher` may change between Rust releases and orphan the cache.
fn fnv(text: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

impl NativeCore {
    /// The artwork URL for a song, album, artist or playlist row, if it has one.
    pub(crate) fn artwork_source(&self, item: &Value, size: u32) -> Option<url::Url> {
        if let Some(remote) = item["artworkUrl"].as_str().filter(|s| !s.is_empty()) {
            let url = self.base.join(remote).ok()?;
            return ["http", "https"].contains(&url.scheme()).then_some(url);
        }
        let id = item["coverArt"]
            .as_str()
            .or(item["albumId"].as_str())
            .filter(|s| !s.is_empty())?;
        let mut url = self.endpoint("rest", "getCoverArt").ok()?;
        url.query_pairs_mut()
            .append_pair("id", id)
            .append_pair("size", &size.to_string());
        Some(url)
    }
    fn fetch_artwork(&self, url: &url::Url) -> Result<Vec<u8>> {
        let response = self
            .http
            .get(url.clone())
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .map_err(|_| err("Artwork unavailable"))?;
        let image = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("image/"));
        if !response.status().is_success() || !image {
            return Err(err("Artwork unavailable"));
        }
        let mut bytes = vec![];
        response
            .take(MAX_BYTES)
            .read_to_end(&mut bytes)
            .map_err(|_| err("Artwork unavailable"))?;
        if bytes.is_empty() {
            return Err(err("Artwork unavailable"));
        }
        Ok(bytes)
    }
    fn prune_artwork(&self, directory: &std::path::Path) {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        let mut files: Vec<_> = entries
            .flatten()
            .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
            .collect();
        if files.len() <= CACHE_LIMIT {
            return;
        }
        files.sort();
        for (_, path) in files.iter().take(files.len() - CACHE_LIMIT * 3 / 4) {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[uniffi::export]
impl NativeCore {
    /// Return only an already-cached local artwork URL. Safe for platform metadata callbacks.
    pub fn artwork_cache_url(&self, item_json: String, size: u32) -> Result<String> {
        let item: Value = serde_json::from_str(&item_json).map_err(|_| err("Invalid item"))?;
        let source = self
            .artwork_source(&item, size.clamp(32, 1200))
            .ok_or_else(|| err("No artwork"))?;
        let mut path = self.root.join("art").join(fnv(source.as_str()));
        if !path.is_file() {
            // Native row/player tiles may have cached a different resolution already.
            path = [
                112, 96, 80, 104, 64, 300, 384, 192, 512, 256, 56, 48, 40, 32, 1200,
            ]
            .into_iter()
            .filter_map(|size| self.artwork_source(&item, size))
            .map(|url| self.root.join("art").join(fnv(url.as_str())))
            .find(|p| p.is_file())
            .ok_or_else(|| err("Artwork is not cached"))?;
        }
        url::Url::from_file_path(path)
            .map(|u| u.to_string())
            .map_err(|_| err("Artwork unavailable"))
    }
    /// Encoded image bytes for a catalog row. Errors mean "show the placeholder";
    /// misses are remembered for this session so long lists do not refetch them.
    pub fn artwork(&self, item_json: String, size: u32) -> Result<Vec<u8>> {
        let item: Value = serde_json::from_str(&item_json).map_err(|_| err("Invalid item"))?;
        let url = self
            .artwork_source(&item, size.clamp(32, 1200))
            .ok_or_else(|| err("No artwork"))?;
        let key = fnv(url.as_str());
        let directory = self.root.join("art");
        let path = directory.join(&key);
        if let Ok(bytes) = std::fs::read(&path) {
            return Ok(bytes);
        }
        if self
            .art_misses
            .lock()
            .map_err(|_| err("Artwork unavailable"))?
            .contains(&key)
        {
            return Err(err("Artwork unavailable"));
        }
        let bytes = match self.fetch_artwork(&url) {
            Ok(bytes) => bytes,
            Err(error) => {
                if let Ok(mut misses) = self.art_misses.lock() {
                    misses.insert(key);
                }
                return Err(error);
            }
        };
        if std::fs::create_dir_all(&directory).is_ok() {
            let partial = directory.join(format!("{key}.partial"));
            if std::fs::write(&partial, &bytes).is_ok() && std::fs::rename(&partial, &path).is_ok()
            {
                self.prune_artwork(&directory);
            } else {
                let _ = std::fs::remove_file(&partial);
            }
        }
        Ok(bytes)
    }
}

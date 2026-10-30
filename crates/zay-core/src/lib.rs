mod art;
pub mod catalog;
pub mod downloads;
mod history;
mod library;
mod management;
pub mod model;
mod playback;
pub mod playlists;
pub mod queue;
pub mod radio;
pub mod search;
use model::Song;
use queue::Queue;
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Condvar, Mutex,
    },
};
uniffi::setup_scaffolding!();
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum CoreError {
    #[error("{message}")]
    Failure { message: String },
}
type Result<T> = std::result::Result<T, CoreError>;
fn err(message: &str) -> CoreError {
    CoreError::Failure {
        message: message.into(),
    }
}
#[derive(uniffi::Object)]
pub struct NativeCore {
    queue: Mutex<Queue>,
    prepared: Mutex<Vec<Queue>>,
    generation: AtomicU64,
    playback_epoch: AtomicU64,
    listening: Mutex<history::Listening>,
    radio_epoch: AtomicU64,
    autoplay: AtomicBool,
    user_volume: AtomicU64,
    oauth_flow: Mutex<Option<management::OAuthFlow>>,
    oauth_error: Mutex<String>,
    resume_position: Mutex<Value>,
    radio: Mutex<radio::RadioState>,
    radio_done: Condvar,
    db: Mutex<Connection>,
    download_commit: Mutex<()>,
    playlist_mutations: Mutex<()>,
    art_misses: Mutex<std::collections::HashSet<String>>,
    server_job_failures: AtomicU64,
    base: url::Url,
    http: reqwest::blocking::Client,
    root: PathBuf,
}
#[uniffi::export]
impl NativeCore {
    #[uniffi::constructor]
    pub fn new(server: String, directory: String) -> Result<Arc<Self>> {
        let base = url::Url::parse(&server).map_err(|_| err("Invalid server URL"))?;
        if !["http", "https"].contains(&base.scheme())
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
        {
            return Err(err("Use an HTTP(S) server URL without credentials"));
        }
        let root = PathBuf::from(directory);
        std::fs::create_dir_all(root.join("audio"))
            .map_err(|_| err("Cannot create data directory"))?;
        let db = Connection::open(root.join("catalog.sqlite"))
            .map_err(|_| err("Cannot open catalog"))?;
        db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS kv(key TEXT PRIMARY KEY, value TEXT NOT NULL); CREATE TABLE IF NOT EXISTS downloads(id TEXT PRIMARY KEY, state TEXT NOT NULL, path TEXT NOT NULL, meta TEXT NOT NULL, bytes INTEGER NOT NULL DEFAULT 0, error TEXT);").map_err(|_| err("Cannot migrate catalog"))?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS recent_songs(id TEXT PRIMARY KEY, played_at INTEGER NOT NULL, song TEXT NOT NULL);").map_err(|_| err("Cannot migrate listening history"))?;
        downloads::migrate(&db)?;
        downloads::clean_partial_files(&root)?;
        let queue = db
            .query_row("SELECT value FROM kv WHERE key='queue'", [], |r| {
                r.get::<_, String>(0)
            })
            .ok()
            .and_then(|v| serde_json::from_str(&v).ok())
            .unwrap_or_default();
        let resume_position = db
            .query_row(
                "SELECT value FROM kv WHERE key='playbackPosition'",
                [],
                |r| r.get::<_, String>(0),
            )
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .unwrap_or(Value::Null);
        let autoplay = db
            .query_row("SELECT value FROM kv WHERE key='autoplay'", [], |r| {
                r.get::<_, String>(0)
            })
            .ok()
            .and_then(|s| serde_json::from_str::<bool>(&s).ok())
            .unwrap_or(true);
        let volume = db
            .query_row("SELECT value FROM kv WHERE key='volume'", [], |r| {
                r.get::<_, String>(0)
            })
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|v| v.is_finite())
            .unwrap_or(1.)
            .clamp(0., 1.);
        let http = reqwest::blocking::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|_| err("Cannot initialize network"))?;
        Ok(Arc::new(Self {
            queue: Mutex::new(queue),
            prepared: Mutex::new(Vec::new()),
            generation: AtomicU64::new(0),
            playback_epoch: AtomicU64::new(0),
            listening: Mutex::new(history::Listening::default()),
            radio_epoch: AtomicU64::new(0),
            autoplay: AtomicBool::new(autoplay),
            user_volume: AtomicU64::new(volume.to_bits()),
            oauth_flow: Mutex::new(None),
            oauth_error: Mutex::new(String::new()),
            resume_position: Mutex::new(resume_position),
            radio: Mutex::new(radio::RadioState::default()),
            radio_done: Condvar::new(),
            db: Mutex::new(db),
            download_commit: Mutex::new(()),
            playlist_mutations: Mutex::new(()),
            art_misses: Mutex::new(Default::default()),
            server_job_failures: AtomicU64::new(0),
            base,
            http,
            root,
        }))
    }
    /// JSON is the stable FFI envelope; no credentials or raw network errors cross it.
    pub fn queue_command(&self, command: String) -> Result<String> {
        let v: Value = serde_json::from_str(&command).map_err(|_| err("Invalid queue command"))?;
        let mut q = self.queue.lock().map_err(|_| err("Queue unavailable"))?;
        let previous_id = q.current().map(|s| s.id.clone());
        let songs = || search::playable(v["songs"].clone());
        let index = |key: &str| v[key].as_u64().unwrap_or(0) as usize;
        let mut playing = true;
        let restart = v["op"] == "previous" && v["position"].as_f64().unwrap_or(0.) > 3.;
        let op = v["op"].as_str().unwrap_or("snapshot");
        let mut prepared = self
            .prepared
            .lock()
            .map_err(|_| err("Playback plan unavailable"))?;
        if ["set", "weave"].contains(&op) {
            self.generation.fetch_add(1, Ordering::SeqCst);
        }
        if op != "snapshot" && op != "next" {
            prepared.clear();
        }
        match op {
            "set" => {
                if v["shuffleMode"] == "off" {
                    q.shuffle = false;
                }
                let selected = index("index");
                let selected = v["songs"]
                    .as_array()
                    .map(|rows| {
                        rows.iter()
                            .take(selected)
                            .filter(|s| s["browseKind"].as_str().is_none_or(|kind| kind == "songs"))
                            .count()
                    })
                    .unwrap_or(selected);
                q.set(songs()?, selected);
            }
            "next" => {
                if prepared.is_empty() {
                    playing = q.advance();
                } else {
                    *q = prepared.remove(0);
                }
            }
            "previous" => {
                if !restart {
                    q.previous();
                } else {
                    *self
                        .resume_position
                        .lock()
                        .map_err(|_| err("Playback unavailable"))? = Value::Null;
                    self.setting("playbackPosition".into(), Some("null".into()))?;
                }
            }
            "shuffle" => q.toggle_shuffle(),
            "repeat" => {
                q.repeat = match q.repeat {
                    queue::Repeat::Off => queue::Repeat::All,
                    queue::Repeat::All => queue::Repeat::One,
                    queue::Repeat::One => queue::Repeat::Off,
                }
            }
            "playNext" => q.priority(songs()?, false),
            "playLast" => q.priority(songs()?, true),
            "append" => q.append(songs()?),
            "jumpSecondary" => q.jump_secondary(index("index")),
            "jump" => q.jump(index("index")),
            "remove" => q.remove(index("index")),
            "move" => q.move_song(index("from"), index("to")),
            "updateMetadata" => {
                let id = v["id"].as_str().unwrap_or("");
                let update = |song: &mut Song| {
                    if song.id == id {
                        let mut value = json!(*song);
                        if let Some(patch) = v["edits"].as_object() {
                            for (key, item) in patch {
                                if [
                                    "title", "artist", "album", "track", "year", "genre", "starred",
                                ]
                                .contains(&key.as_str())
                                {
                                    value[key] = item.clone();
                                }
                            }
                        }
                        if let Ok(replacement) = serde_json::from_value(value) {
                            *song = replacement;
                        }
                    }
                };
                q.queue.iter_mut().for_each(&update);
                q.secondary_queue.iter_mut().for_each(&update);
                q.history.iter_mut().for_each(&update);
                if let Some(song) = q.active_song.as_mut() {
                    update(song);
                }
            }
            "moveUpcoming" => {
                let from = index("from");
                let direction = v["direction"].as_i64().unwrap_or(0).clamp(-1, 1) as isize;
                if q.shuffle {
                    if let Some(pos) = q.shuffle_order.iter().position(|i| *i == from) {
                        let target = pos as isize + direction;
                        if pos as isize > q.shuffle_pos
                            && target > q.shuffle_pos
                            && target >= 0
                            && (target as usize) < q.shuffle_order.len()
                        {
                            q.shuffle_order.swap(pos, target as usize);
                        }
                    }
                } else {
                    let target = from as isize + direction;
                    if from as isize > q.index && target > q.index && target >= 0 {
                        q.move_song(from, target as usize);
                    }
                }
            }
            "removeSecondary" => {
                let i = index("index");
                if i < q.secondary_queue.len() {
                    q.secondary_queue.remove(i);
                }
            }
            "moveSecondary" => {
                let f = index("from");
                let t = index("to");
                if f < q.secondary_queue.len() && t < q.secondary_queue.len() {
                    let s = q.secondary_queue.remove(f);
                    q.secondary_queue.insert(t, s);
                }
            }
            "weave" => {
                let fresh: Vec<Song> = serde_json::from_value(v["fresh"].clone())
                    .map_err(|_| err("Invalid recommendations"))?;
                let dense = v["dense"].as_bool().unwrap_or(false);
                q.set_woven(songs()?, fresh, dense);
            }
            "radio" => {
                let fresh = model::fresh_radio(&q.queue, songs()?);
                q.append(fresh);
            }
            "snapshot" => {}
            _ => return Err(err("Unknown queue command")),
        }
        if op != "snapshot" {
            if (["set", "next", "previous", "jump", "jumpSecondary", "weave"].contains(&op)
                && !restart)
                || previous_id != q.current().map(|s| s.id.clone())
            {
                self.playback_epoch.fetch_add(1, Ordering::SeqCst);
                *self
                    .resume_position
                    .lock()
                    .map_err(|_| err("Playback unavailable"))? = Value::Null;
                self.setting("playbackPosition".into(), Some("null".into()))?;
            }
            let encoded = serde_json::to_string(&*q).map_err(|_| err("Cannot serialize queue"))?;
            self.db
                .lock()
                .map_err(|_| err("Catalog unavailable"))?
                .execute("INSERT OR REPLACE INTO kv VALUES('queue',?1)", [encoded])
                .map_err(|_| err("Cannot save queue"))?;
        }
        let mut lookahead = prepared.last().cloned().unwrap_or_else(|| q.clone());
        while prepared.len() < 3 {
            if !lookahead.advance() {
                break;
            }
            prepared.push(lookahead.clone());
        }
        let upcoming: Vec<Song> = prepared
            .iter()
            .filter_map(|q| q.current().cloned())
            .collect();
        let radio = self.radio.lock().map_err(|_| err("Radio unavailable"))?;
        let resume = self
            .resume_position
            .lock()
            .map_err(|_| err("Playback unavailable"))?
            .clone();
        let resume_position = if q.current().is_some_and(|song| resume["id"] == song.id) {
            resume["position"].as_f64().unwrap_or(0.)
        } else {
            0.
        };
        Ok(json!({"restart":restart,"resumePosition":resume_position,"playbackEpoch":self.playback_epoch.load(Ordering::SeqCst),"state":*q,"current":q.current(),"upcoming":upcoming,"volume":self.volume(),"gain":q.current().map(Song::gain).unwrap_or(1.0)*self.volume(),"continue":playing,"generation":self.generation.load(Ordering::SeqCst),"autoplay":self.autoplay.load(Ordering::SeqCst),"radioLoading":radio.running,"radioError":radio.error}).to_string())
    }
    pub fn api(
        &self,
        service: String,
        endpoint: String,
        method: String,
        parameters: String,
    ) -> Result<String> {
        let mut url = self.endpoint(&service, &endpoint)?;
        let p: Value = serde_json::from_str(&parameters).map_err(|_| err("Invalid parameters"))?;
        let mut req = match method.as_str() {
            "POST" => self.http.post(url.clone()),
            "DELETE" => self.http.delete(url.clone()),
            "GET" => self.http.get(url.clone()),
            _ => return Err(err("Invalid method")),
        };
        if service == "rest" || method == "GET" {
            if let Some(obj) = p.as_object() {
                for (k, v) in obj {
                    let values = if let Some(a) = v.as_array() {
                        a.clone()
                    } else {
                        vec![v.clone()]
                    };
                    for value in values {
                        url.query_pairs_mut().append_pair(
                            k,
                            &value
                                .as_str()
                                .map(str::to_owned)
                                .unwrap_or_else(|| value.to_string()),
                        );
                    }
                }
            }
            req = match method.as_str() {
                "POST" if service == "rest" => {
                    let form = url.query().unwrap_or("").to_owned();
                    url.set_query(None);
                    let origin = url.origin().ascii_serialization();
                    self.http
                        .post(url)
                        .header(reqwest::header::ORIGIN, origin)
                        .header(
                            reqwest::header::CONTENT_TYPE,
                            "application/x-www-form-urlencoded",
                        )
                        .body(form)
                }
                "POST" => self.http.post(url),
                "DELETE" => self.http.delete(url),
                _ => self.http.get(url),
            };
        } else {
            req = req.json(&p);
        }
        if method != "GET" {
            req = req.header(
                reqwest::header::ORIGIN,
                self.base.origin().ascii_serialization(),
            );
        }
        let res = req.send().map_err(|_| err("Server unreachable"))?;
        if !res.status().is_success() {
            let status = res.status().as_u16();
            let body = res.json::<Value>().unwrap_or(Value::Null);
            let detail = body["detail"]
                .as_str()
                .or(body["message"].as_str())
                .map(playback::sanitize_error)
                .unwrap_or_default();
            return Err(err(&if detail.is_empty() {
                format!("Server returned HTTP {status}")
            } else {
                format!("{detail} (HTTP {status})")
            }));
        }
        let mut body: Value = res.json().map_err(|_| err("Invalid server response"))?;
        if service == "rest" {
            body = body["subsonic-response"].clone();
            if body["status"] != "ok" {
                return Err(err("Library request failed"));
            }
        }
        if service == "rest" && endpoint == "getAlbum" {
            if let Some(songs) = body["album"]["song"].as_array_mut() {
                if songs.iter().any(|s| s["track"].as_u64().unwrap_or(0) == 0) {
                    let id = p["id"].as_str().unwrap_or("");
                    if let Ok(order) = self.api(
                        "fetch".into(),
                        format!(
                            "library/albums/{}/order",
                            url::form_urlencoded::byte_serialize(id.as_bytes()).collect::<String>()
                        ),
                        "GET".into(),
                        "{}".into(),
                    ) {
                        if let Ok(order) = serde_json::from_str::<Value>(&order) {
                            for song in songs.iter_mut() {
                                if song["track"].as_u64().unwrap_or(0) == 0 {
                                    let id = song["id"].as_str().unwrap_or("").to_string();
                                    if let Some(track) = order["tracks"][&id].as_u64() {
                                        song["track"] = json!(track);
                                    }
                                }
                            }
                        }
                    }
                }
                songs.sort_by_key(|s| {
                    (
                        s["discNumber"].as_u64().filter(|n| *n > 0).unwrap_or(1),
                        s["track"].as_u64().filter(|n| *n > 0).unwrap_or(u64::MAX),
                    )
                });
            }
        }
        Ok(body.to_string())
    }
    pub fn media_url(&self, song_json: String, download: bool) -> Result<String> {
        let s: Song = serde_json::from_str(&song_json).map_err(|_| err("Invalid song"))?;
        if !download {
            let db = self.db.lock().map_err(|_| err("Catalog unavailable"))?;
            if let Ok(path) = db.query_row(
                "SELECT path FROM downloads WHERE id=?1 AND state='ready'",
                [&s.id],
                |r| r.get::<_, String>(0),
            ) {
                let path = self.root.join(path);
                if path.is_file() {
                    return url::Url::from_file_path(path)
                        .map(String::from)
                        .map_err(|_| err("Invalid offline path"));
                }
            }
        }
        if let Some(preview) = s.preview_url {
            return self
                .base
                .join(&preview)
                .map(String::from)
                .map_err(|_| err("Invalid preview URL"));
        }
        let mut url = self.endpoint("rest", if download { "download" } else { "stream" })?;
        url.query_pairs_mut().append_pair("id", &s.id);
        Ok(url.to_string())
    }
    pub fn setting(&self, key: String, value: Option<String>) -> Result<String> {
        let db = self.db.lock().map_err(|_| err("Catalog unavailable"))?;
        if let Some(value) = value {
            db.execute(
                "INSERT OR REPLACE INTO kv VALUES(?1,?2)",
                params![key, value],
            )
            .map_err(|_| err("Cannot save setting"))?;
        }
        Ok(db
            .query_row("SELECT value FROM kv WHERE key=?1", [key], |r| r.get(0))
            .unwrap_or("null".into()))
    }
}
impl NativeCore {
    fn endpoint(&self, service: &str, endpoint: &str) -> Result<url::Url> {
        if !["rest", "fetch"].contains(&service)
            || endpoint.split('/').any(|s| s == "..")
            || endpoint.contains(['?', '#', ':'])
            || endpoint.starts_with('/')
        {
            return Err(err("Invalid API endpoint"));
        }
        self.base
            .join(&format!(
                "/{}/{}",
                if service == "rest" {
                    "rest"
                } else {
                    "api/fetch"
                },
                endpoint
            ))
            .map_err(|_| err("Invalid API URL"))
    }
}
fn safe_id(id: &str) -> String {
    id.as_bytes().iter().map(|b| format!("{b:02x}")).collect()
}

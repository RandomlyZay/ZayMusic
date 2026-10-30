//! Shared native-file catalog and download lifecycle. OS transfers report progress
//! with a ticket so cancelled or superseded work cannot overwrite newer records.
use crate::{err, model::Song, safe_id, NativeCore, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    io::Write,
    path::{Component, Path},
    sync::Mutex,
    time::{Duration, Instant},
};
pub(crate) fn migrate(db: &Connection) -> Result<()> {
    let mut statement = db
        .prepare("PRAGMA table_info(downloads)")
        .map_err(|_| err("Cannot read catalog schema"))?;
    let columns: Vec<String> = statement
        .query_map([], |r| r.get(1))
        .map_err(|_| err("Cannot read catalog schema"))?
        .collect::<std::result::Result<_, _>>()
        .map_err(|_| err("Cannot read catalog schema"))?;
    for (name, definition) in [
        ("total", "INTEGER NOT NULL DEFAULT 0"),
        ("progress", "REAL NOT NULL DEFAULT 0"),
        ("generation", "INTEGER NOT NULL DEFAULT 0"),
        ("task_key", "TEXT NOT NULL DEFAULT ''"),
    ] {
        if !columns.iter().any(|c| c == name) {
            db.execute_batch(&format!(
                "ALTER TABLE downloads ADD COLUMN {name} {definition}"
            ))
            .map_err(|_| err("Cannot migrate download catalog"))?;
        }
    }
    db.execute(
        "UPDATE downloads SET progress=1,total=bytes WHERE state='ready' AND progress=0",
        [],
    )
    .map_err(|_| err("Cannot migrate ready downloads"))?;
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS download_tickets(id INTEGER PRIMARY KEY AUTOINCREMENT)",
    )
    .map_err(|_| err("Cannot initialize transfer tickets"))?;
    db.execute("UPDATE downloads SET state='failed',error='Download interrupted; retry.',task_key='' WHERE state='downloading' AND task_key LIKE 'desktop:%'",[]).map_err(|_|err("Cannot recover interrupted transfers"))?;
    Ok(())
}
pub(crate) fn clean_partial_files(root: &Path) -> Result<()> {
    for entry in
        std::fs::read_dir(root.join("audio")).map_err(|_| err("Cannot inspect offline files"))?
    {
        let entry = entry.map_err(|_| err("Cannot inspect offline files"))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Some(stem) = name
            .strip_suffix(".part")
            .or_else(|| name.strip_suffix(".incoming"))
        {
            if let Some((id, ticket)) = stem.split_once('-') {
                if !id.is_empty()
                    && id.bytes().all(|b| b.is_ascii_hexdigit())
                    && ticket.parse::<u64>().is_ok()
                {
                    std::fs::remove_file(entry.path())
                        .map_err(|_| err("Cannot recover partial download"))?;
                }
            }
        }
    }
    Ok(())
}
fn extension(song: &Song) -> &str {
    let suffix = song.suffix.as_deref().unwrap_or("");
    match suffix.trim_start_matches('.') {
        "mp3" => "mp3",
        "ogg" | "oga" => "ogg",
        "opus" => "opus",
        "m4a" | "mp4" => "m4a",
        "aac" => "aac",
        "flac" => "flac",
        "wav" => "wav",
        _ => match song.content_type.as_deref().unwrap_or("") {
            "audio/ogg" => "ogg",
            "audio/mpeg" => "mp3",
            "audio/mp4" | "audio/aac" => "m4a",
            "audio/flac" => "flac",
            "audio/wav" => "wav",
            _ => "media",
        },
    }
}
fn owned_path(path: &str) -> bool {
    let parts: Vec<_> = Path::new(path).components().collect();
    matches!(parts.as_slice(),[Component::Normal(folder),Component::Normal(_)] if *folder==std::ffi::OsStr::new("audio"))
}
#[uniffi::export]
impl NativeCore {
    pub fn queue_download(&self, song_json: String) -> Result<String> {
        let item: Value =
            serde_json::from_str(&song_json).map_err(|_| err("Invalid download item"))?;
        if item["browseKind"]
            .as_str()
            .is_some_and(|kind| kind != "songs")
        {
            return Err(err("Open the collection to download its songs"));
        }
        let song: Song =
            serde_json::from_str(&song_json).map_err(|_| err("Invalid download metadata"))?;
        if song.id.starts_with("yt:") || song.preview_url.as_ref().is_some_and(|s| !s.is_empty()) {
            return Err(err(
                "Add this preview to your library before downloading it",
            ));
        }
        if song.id.is_empty() {
            return Err(err("Missing song identifier"));
        }
        let mut metadata: Value =
            serde_json::from_str(&song_json).map_err(|_| err("Invalid download metadata"))?;
        if let Some(object) = metadata.as_object_mut() {
            for key in [
                "download",
                "queueLane",
                "queueIndex",
                "playlistId",
                "playlistIndex",
            ] {
                object.remove(key);
            }
        }
        let song_json = metadata.to_string();
        let url = self.media_url(song_json.clone(), true)?;
        let db = self.db.lock().map_err(|_| err("Catalog unavailable"))?;
        let existing: Option<(String, String, u64)> = db
            .query_row(
                "SELECT state,path,generation FROM downloads WHERE id=?1",
                [&song.id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(|_| err("Cannot read download"))?;
        if let Some((state, path, ticket)) = &existing {
            if state == "ready" && owned_path(path) && self.root.join(path).is_file() {
                return Ok(json!({"id":song.id,"ticket":ticket,"ready":true,"url":url}).to_string());
            }
            if state == "queued" || state == "downloading" {
                return Ok(
                    json!({"id":song.id,"ticket":ticket,"ready":false,"url":url,"state":state})
                        .to_string(),
                );
            }
        }
        db.execute("INSERT INTO download_tickets DEFAULT VALUES", [])
            .map_err(|_| err("Cannot allocate transfer ticket"))?;
        let ticket = db.last_insert_rowid() as u64;
        db.execute("INSERT INTO downloads(id,state,path,meta,bytes,error,total,progress,generation,task_key) VALUES(?1,'queued','',?2,0,NULL,0,0,?3,'') ON CONFLICT(id) DO UPDATE SET state='queued',meta=excluded.meta,bytes=0,error=NULL,total=0,progress=0,generation=excluded.generation,task_key=''",params![song.id,song_json,ticket]).map_err(|_|err("Cannot queue download"))?;
        Ok(
            json!({"id":song.id,"ticket":ticket,"ready":false,"url":url,"state":"queued"})
                .to_string(),
        )
    }
    pub fn begin_download(&self, id: String, ticket: u64, task_key: String) -> Result<bool> {
        let changed=self.db.lock().map_err(|_|err("Catalog unavailable"))?.execute("UPDATE downloads SET state='downloading',task_key=?3 WHERE id=?1 AND generation=?2 AND state='queued'",params![id,ticket,task_key]).map_err(|_|err("Cannot begin download"))?;
        Ok(changed == 1)
    }
    pub fn download_active(&self, id: String, ticket: u64) -> Result<bool> {
        self.db.lock().map_err(|_|err("Catalog unavailable"))?.query_row("SELECT EXISTS(SELECT 1 FROM downloads WHERE id=?1 AND generation=?2 AND state IN ('queued','downloading'))",params![id,ticket],|r|r.get(0)).map_err(|_|err("Cannot read download"))
    }
    pub fn download_progress(
        &self,
        id: String,
        ticket: u64,
        received: u64,
        total: u64,
    ) -> Result<()> {
        let progress = if total > 0 {
            (received as f64 / total as f64).clamp(0., 0.99)
        } else {
            0.
        };
        self.db.lock().map_err(|_|err("Catalog unavailable"))?.execute("UPDATE downloads SET bytes=?3,total=?4,progress=?5 WHERE id=?1 AND generation=?2 AND state='downloading'",params![id,ticket,received,total,progress]).map_err(|_|err("Cannot save download progress"))?;
        Ok(())
    }
    pub fn fail_download(&self, id: String, ticket: u64, reason: String) -> Result<()> {
        let message = match reason.as_str() {
            "http" => "Server rejected the download.",
            "storage" => "Could not store the downloaded file.",
            "interrupted" => "Download interrupted; retry.",
            _ => "Download connection failed; retry.",
        };
        self.db.lock().map_err(|_|err("Catalog unavailable"))?.execute("UPDATE downloads SET state='failed',error=?3,task_key='' WHERE id=?1 AND generation=?2 AND state IN ('queued','downloading')",params![id,ticket,message]).map_err(|_|err("Cannot record download failure"))?;
        Ok(())
    }
    pub fn cancel_download(&self, id: String) -> Result<()> {
        self.db.lock().map_err(|_|err("Catalog unavailable"))?.execute("UPDATE downloads SET state='cancelled',generation=generation+1,error=NULL,task_key='' WHERE id=?1 AND state IN ('queued','downloading')",[id]).map_err(|_|err("Cannot cancel download"))?;
        Ok(())
    }
    pub fn complete_download(&self, id: String, ticket: u64, temporary_file: String) -> Result<()> {
        let _commit = self
            .download_commit
            .lock()
            .map_err(|_| err("Offline storage unavailable"))?;
        let metadata: String = {
            let db = self.db.lock().map_err(|_| err("Catalog unavailable"))?;
            db.query_row("SELECT meta FROM downloads WHERE id=?1 AND generation=?2 AND state IN ('queued','downloading')",params![id,ticket],|r|r.get(0)).map_err(|_|err("Download was cancelled or superseded"))?
        };
        let song: Song =
            serde_json::from_str(&metadata).map_err(|_| err("Invalid download metadata"))?;
        let name = format!("audio/{}-{}.{}", safe_id(&id), ticket, extension(&song));
        let incoming = self
            .root
            .join(format!("audio/{}-{ticket}.incoming", safe_id(&id)));
        let result = (|| -> Result<()> {
            let mut source = std::fs::File::open(&temporary_file)
                .map_err(|_| err("Temporary download missing"))?;
            let mut output =
                std::fs::File::create(&incoming).map_err(|_| err("Cannot create offline file"))?;
            let bytes = std::io::copy(&mut source, &mut output)
                .map_err(|_| err("Cannot copy offline file"))?;
            if bytes == 0 {
                return Err(err("Downloaded file is empty"));
            }
            output
                .sync_all()
                .map_err(|_| err("Cannot finish offline file"))?;
            drop(output);
            let db = self.db.lock().map_err(|_| err("Catalog unavailable"))?;
            let active:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM downloads WHERE id=?1 AND generation=?2 AND state IN ('queued','downloading'))",params![id,ticket],|r|r.get(0)).map_err(|_|err("Cannot read download"))?;
            if !active {
                return Err(err("Download was cancelled or superseded"));
            }
            let target = self.root.join(&name);
            std::fs::rename(&incoming, &target).map_err(|_| err("Cannot commit offline file"))?;
            if db.execute("UPDATE downloads SET state='ready',path=?3,bytes=?4,total=?4,progress=1,error=NULL,task_key='' WHERE id=?1 AND generation=?2",params![id,ticket,name,bytes]).is_err(){let _=std::fs::remove_file(target);return Err(err("Cannot commit offline catalog"));}
            Ok(())
        })();
        let _ = std::fs::remove_file(incoming);
        result
    }
    /// Compatibility helper for a local file import. Transfer delegates use ticketed completion.
    pub fn register_download(&self, song_json: String, temporary_file: String) -> Result<()> {
        let plan: Value = serde_json::from_str(&self.queue_download(song_json)?)
            .map_err(|_| err("Invalid download plan"))?;
        if plan["ready"] == true {
            return Ok(());
        }
        self.complete_download(
            plan["id"].as_str().unwrap_or("").into(),
            plan["ticket"].as_u64().unwrap_or(0),
            temporary_file,
        )
    }
    pub fn delete_download(&self, id: String) -> Result<()> {
        let db = self.db.lock().map_err(|_| err("Catalog unavailable"))?;
        let path: Option<String> = db
            .query_row("SELECT path FROM downloads WHERE id=?1", [&id], |r| {
                r.get(0)
            })
            .optional()
            .map_err(|_| err("Cannot read download"))?;
        if let Some(path) = path {
            if owned_path(&path) {
                let file = self.root.join(path);
                if file.exists() {
                    std::fs::remove_file(file).map_err(|_| err("Cannot remove offline file"))?;
                }
            }
        }
        db.execute("DELETE FROM downloads WHERE id=?1", [id])
            .map_err(|_| err("Cannot remove download record"))?;
        Ok(())
    }
    pub fn delete_all_downloads(&self) -> Result<()> {
        let rows: Value = serde_json::from_str(&self.downloads()?)
            .map_err(|_| err("Invalid download catalog"))?;
        for row in rows.as_array().unwrap_or(&vec![]) {
            if let Some(id) = row["song"]["id"].as_str() {
                self.delete_download(id.into())?;
            }
        }
        Ok(())
    }
    pub fn recover_downloads(&self, active_json: String) -> Result<()> {
        let active: Vec<Value> =
            serde_json::from_str(&active_json).map_err(|_| err("Invalid active download list"))?;
        let db = self.db.lock().map_err(|_| err("Catalog unavailable"))?;
        let mut statement = db
            .prepare("SELECT id,generation FROM downloads WHERE state IN ('queued','downloading')")
            .map_err(|_| err("Cannot recover downloads"))?;
        let rows: Vec<(String, u64)> = statement
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|_| err("Cannot recover downloads"))?
            .collect::<std::result::Result<_, _>>()
            .map_err(|_| err("Cannot recover downloads"))?;
        for (id, ticket) in rows {
            if !active
                .iter()
                .any(|v| v["id"] == id && v["ticket"] == ticket)
            {
                db.execute("UPDATE downloads SET state='failed',error='Download interrupted; retry.',task_key='' WHERE id=?1 AND generation=?2",params![id,ticket]).map_err(|_|err("Cannot recover download"))?;
            }
        }
        Ok(())
    }
    pub fn storage_stats(&self) -> Result<String> {
        let rows: Value = serde_json::from_str(&self.downloads()?)
            .map_err(|_| err("Invalid download catalog"))?;
        let mut stats = json!({"readyBytes":0_u64,"readyCount":0_u64,"queued":0_u64,"downloading":0_u64,"failed":0_u64,"cancelled":0_u64});
        for row in rows.as_array().unwrap_or(&vec![]) {
            let state = row["state"].as_str().unwrap_or("");
            if state == "ready" {
                stats["readyBytes"] = json!(
                    stats["readyBytes"].as_u64().unwrap() + row["bytes"].as_u64().unwrap_or(0)
                );
                stats["readyCount"] = json!(stats["readyCount"].as_u64().unwrap() + 1);
            } else if ["queued", "downloading", "failed", "cancelled"].contains(&state) {
                stats[state] = json!(stats[state].as_u64().unwrap() + 1);
            }
        }
        Ok(stats.to_string())
    }
    pub fn downloads(&self) -> Result<String> {
        let db = self.db.lock().map_err(|_| err("Catalog unavailable"))?;
        let mut statement=db.prepare("SELECT meta,state,bytes,error,total,progress,generation FROM downloads ORDER BY rowid DESC").map_err(|_|err("Cannot read downloads"))?;
        let rows = statement
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, u64>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, u64>(4)?,
                    r.get::<_, f64>(5)?,
                    r.get::<_, u64>(6)?,
                ))
            })
            .map_err(|_| err("Cannot read downloads"))?;
        let mut out = vec![];
        for row in rows {
            let (meta, state, bytes, error, total, progress, ticket) =
                row.map_err(|_| err("Invalid download record"))?;
            out.push(json!({"song":serde_json::from_str::<Value>(&meta).unwrap_or(Value::Null),"state":state,"bytes":bytes,"error":error,"total":total,"progress":progress,"ticket":ticket}));
        }
        Ok(json!(out).to_string())
    }
    pub fn download(&self, song_json: String) -> Result<()> {
        let plan: Value = serde_json::from_str(&self.queue_download(song_json)?)
            .map_err(|_| err("Invalid download plan"))?;
        if plan["ready"] == true {
            return Ok(());
        }
        self.transfer_download(plan)
    }
    pub fn download_many(&self, songs_json: String) -> Result<String> {
        let songs: Vec<Value> =
            serde_json::from_str(&songs_json).map_err(|_| err("Invalid download list"))?;
        let mut plans = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for song in songs
            .into_iter()
            .filter(|s| s["browseKind"].as_str().is_none_or(|kind| kind == "songs"))
            .filter(|s| {
                !s["id"].as_str().is_some_and(|id| id.starts_with("yt:"))
                    && s["previewUrl"].as_str().is_none_or(str::is_empty)
            })
        {
            let plan: Value = serde_json::from_str(&self.queue_download(song.to_string())?)
                .map_err(|_| err("Invalid download plan"))?;
            if seen.insert(plan["id"].as_str().unwrap_or("").to_owned()) {
                plans.push(plan);
            }
        }
        let pending = Mutex::new(VecDeque::from(plans));
        let counts = Mutex::new((0_u64, 0_u64, 0_u64, 0_u64));
        std::thread::scope(|scope| {
            for _ in 0..2 {
                let pending = &pending;
                let counts = &counts;
                scope.spawn(move || loop {
                    let Some(song) = pending.lock().unwrap().pop_front() else {
                        break;
                    };
                    let id = song["id"].as_str().unwrap_or("").to_owned();
                    let result = if song["ready"] == true {
                        Ok(())
                    } else {
                        self.transfer_download(song)
                    };
                    let state = self
                        .db
                        .lock()
                        .unwrap()
                        .query_row("SELECT state FROM downloads WHERE id=?1", [&id], |r| {
                            r.get::<_, String>(0)
                        })
                        .unwrap_or("cancelled".into());
                    let mut counts = counts.lock().unwrap();
                    if state == "ready" {
                        counts.0 += 1;
                    } else if state == "cancelled" {
                        counts.2 += 1;
                    } else if result.is_err() || state == "failed" {
                        counts.1 += 1;
                    } else {
                        counts.3 += 1;
                    }
                });
            }
        });
        let counts = counts.into_inner().unwrap();
        Ok(
            json!({"completed":counts.0,"failed":counts.1,"cancelled":counts.2,"pending":counts.3})
                .to_string(),
        )
    }
}

impl NativeCore {
    fn transfer_download(&self, plan: Value) -> Result<()> {
        let id = plan["id"].as_str().unwrap_or("").to_string();
        let ticket = plan["ticket"].as_u64().unwrap_or(0);
        if !self.begin_download(
            id.clone(),
            ticket,
            format!("desktop:{}", std::process::id()),
        )? {
            return Ok(());
        }
        let part = self
            .root
            .join(format!("audio/{}-{ticket}.part", safe_id(&id)));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| {
                let _ = self.fail_download(id.clone(), ticket, "transport".into());
                err("Cannot initialize transfer")
            })?;
        let result = runtime.block_on(async {
            let client = reqwest::Client::builder().connect_timeout(Duration::from_secs(10)).read_timeout(Duration::from_secs(60)).timeout(Duration::from_secs(3600)).build().map_err(|_|err("Cannot initialize download"))?;
            let request = client.get(plan["url"].as_str().unwrap_or("")).send();
            tokio::pin!(request);
            let mut check = tokio::time::interval(Duration::from_millis(100));
            let mut response = loop {
                tokio::select! {
                    response = &mut request => break response.map_err(|_|err("Download connection failed"))?,
                    _ = check.tick() => { if !self.download_active(id.clone(),ticket)? { return Err(err("Download cancelled")); } }
                }
            };
            if !response.status().is_success() || response.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v|v.to_str().ok()).is_some_and(|v|v.contains("json")||v.contains("text/html")) {return Err(err("Server rejected the download"));}
            let total=response.content_length().unwrap_or(0); let mut received=0; let mut last=Instant::now();
            let mut file=std::fs::File::create(&part).map_err(|_|err("Cannot create download file"))?;
            loop {
                let chunk = loop {
                    tokio::select! {
                        chunk = response.chunk() => break chunk.map_err(|_|err("Download interrupted"))?,
                        _ = check.tick() => { if !self.download_active(id.clone(),ticket)? {return Err(err("Download cancelled"));} }
                    }
                };
                let Some(chunk)=chunk else { break };
                file.write_all(&chunk).map_err(|_|err("Could not write download"))?; received+=chunk.len() as u64;
                if last.elapsed()>Duration::from_millis(250) { self.download_progress(id.clone(),ticket,received,total)?; last=Instant::now(); }
            }
            file.sync_all().map_err(|_|err("Cannot finish download"))?; drop(file);
            if total>0 && received!=total {return Err(err("Download interrupted"));}
            self.complete_download(id.clone(),ticket,part.to_string_lossy().into())
        });
        let _ = std::fs::remove_file(part);
        if result.is_err() {
            let _ = self.fail_download(id, ticket, "transport".into());
        }
        result
    }
}

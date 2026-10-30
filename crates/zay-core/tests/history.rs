use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use zay_core::NativeCore;
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    url: String,
    requests: Arc<Mutex<Vec<Value>>>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(vec![]));
        let stop = Arc::new(AtomicBool::new(false));
        let (captured, done) = (requests.clone(), stop.clone());
        let worker = std::thread::spawn(move || {
            while !done.load(Ordering::SeqCst) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                reader.read_line(&mut first).unwrap();
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                }
                let path = first.split_whitespace().nth(1).unwrap();
                let parsed = url::Url::parse(&format!("http://fixture.invalid{path}")).unwrap();
                let body = if parsed.path() == "/rest/scrobble" {
                    let params: std::collections::HashMap<_, _> = parsed
                        .query_pairs()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect();
                    captured.lock().unwrap().push(json!(params));
                    json!({"subsonic-response":{"status":"ok"}})
                } else if parsed.path() == "/rest/getPlaylists" {
                    json!({"subsonic-response":{"status":"ok","playlists":{"playlist":[]}}})
                } else {
                    assert_eq!(parsed.path(), "/api/fetch/home");
                    json!({"topSongs":[{"id":"server-song","title":"Server song"}],"rediscover":[]})
                }
                .to_string();
                write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
            }
        });
        let root = std::env::temp_dir().join(format!(
            "zay-history-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        Self {
            url,
            requests,
            stop,
            worker: Some(worker),
            root,
        }
    }
    fn core(&self) -> Arc<NativeCore> {
        NativeCore::new(self.url.clone(), self.root.to_string_lossy().into()).unwrap()
    }
    fn wait(&self, n: usize) -> Vec<Value> {
        let deadline = Instant::now() + Duration::from_secs(4);
        while self.requests.lock().unwrap().len() < n && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        let rows = self.requests.lock().unwrap().clone();
        assert_eq!(rows.len(), n);
        rows
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.worker.take().unwrap().join().unwrap();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
fn set(core: &NativeCore, song: Value) -> u64 {
    let v: Value = serde_json::from_str(
        &core
            .queue_command(json!({"op":"set","songs":[song]}).to_string())
            .unwrap(),
    )
    .unwrap();
    v["playbackEpoch"].as_u64().unwrap()
}
fn recent(core: &NativeCore) -> Vec<Value> {
    serde_json::from_str(&core.recent_songs().unwrap()).unwrap()
}
fn report(
    core: &Arc<NativeCore>,
    epoch: u64,
    id: &str,
    position: f64,
    duration: f64,
    playing: bool,
) {
    core.clone()
        .playback_progress(epoch, id.into(), position, duration, playing)
        .unwrap();
}

#[test]
fn half_track_scrobble_is_once_per_play_and_repeat_gets_a_new_epoch() {
    let fixture = Fixture::new();
    let core = fixture.core();
    let epoch = set(&core, json!({"id":"fake & song","duration":60}));
    report(&core, epoch, "fake & song", 0., 60., true);
    report(&core, epoch, "fake & song", 29., 60., true);
    assert_eq!(fixture.wait(1)[0]["submission"], "false");
    report(&core, epoch, "fake & song", 30., 60., true);
    report(&core, epoch, "fake & song", 59., 60., true);
    let rows = fixture.wait(2);
    assert_eq!(rows[1]["submission"], "true");
    assert_eq!(rows[1]["id"], "fake & song");
    core.queue_command("{\"op\":\"repeat\"}".into()).unwrap();
    core.queue_command("{\"op\":\"repeat\"}".into()).unwrap();
    let next: Value =
        serde_json::from_str(&core.queue_command("{\"op\":\"next\"}".into()).unwrap()).unwrap();
    let next_epoch = next["playbackEpoch"].as_u64().unwrap();
    assert_ne!(next_epoch, epoch);
    report(&core, next_epoch, "fake & song", 30., 60., true);
    fixture.wait(4);
    assert_eq!(recent(&core).len(), 1);
}
#[test]
fn four_minutes_counts_for_long_and_unknown_tracks() {
    let fixture = Fixture::new();
    let core = fixture.core();
    for (i, duration) in [900., 0.].into_iter().enumerate() {
        let id = format!("song-{i}");
        let epoch = set(&core, json!({"id":id,"duration":duration}));
        report(&core, epoch, &id, 239., duration, true);
        fixture.wait(i * 2 + 1);
        report(&core, epoch, &id, 240., duration, true);
        fixture.wait(i * 2 + 2);
    }
}
#[test]
fn paused_invalid_stale_and_preview_reports_do_not_count() {
    let fixture = Fixture::new();
    let core = fixture.core();
    let epoch = set(&core, json!({"id":"song","duration":60}));
    for position in [f64::NAN, f64::INFINITY, -1.] {
        report(&core, epoch, "song", position, 60., true);
    }
    report(&core, epoch, "song", 55., 60., false);
    report(&core, epoch + 1, "song", 55., 60., true);
    report(&core, epoch, "another-song", 55., 60., true);
    for song in [
        json!({"id":"yt:fake","duration":60}),
        json!({"id":"preview","previewUrl":"/api/fetch/preview/fake","duration":60}),
    ] {
        let epoch = set(&core, song.clone());
        report(&core, epoch, song["id"].as_str().unwrap(), 60., 60., true);
    }
    std::thread::sleep(Duration::from_millis(40));
    assert!(fixture.requests.lock().unwrap().is_empty());
    assert!(recent(&core).is_empty());
}
#[test]
fn recents_are_bounded_ordered_deduplicated_and_survive_restart() {
    let fixture = Fixture::new();
    let core = fixture.core();
    for i in 0..51 {
        let id = format!("song-{i}");
        let epoch = set(&core, json!({"id":id,"title":id,"duration":60}));
        report(&core, epoch, &id, 0., 60., true);
    }
    fixture.wait(51);
    let rows = recent(&core);
    assert_eq!(rows.len(), 50);
    assert_eq!(rows[0]["id"], "song-50");
    assert_eq!(rows[49]["id"], "song-1");
    let epoch = set(
        &core,
        json!({"id":"song-30","title":"Updated title","duration":60}),
    );
    report(&core, epoch, "song-30", 0., 60., true);
    fixture.wait(52);
    let rows = recent(&core);
    assert_eq!(rows.len(), 50);
    assert_eq!(rows[0]["title"], "Updated title");
    drop(core);
    assert_eq!(recent(&fixture.core()), rows);
}
#[test]
fn history_is_available_offline_and_home_uses_the_shared_recent_section() {
    let fixture = Fixture::new();
    let core = fixture.core();
    let epoch = set(
        &core,
        json!({"id":"local","title":"Local song","duration":60}),
    );
    report(&core, epoch, "local", 0., 60., true);
    fixture.wait(1);
    let home: Value =
        serde_json::from_str(&core.browse("Home".into(), "".into()).unwrap()).unwrap();
    assert_eq!(home["rows"][0]["id"], "local");
    assert_eq!(home["extra"]["sections"][0]["title"], "Recently played");
    assert_eq!(home["extra"]["sections"][1]["start"], 1);
    let offline = NativeCore::new(
        "http://127.0.0.1:9".into(),
        fixture.root.to_string_lossy().into(),
    )
    .unwrap();
    let page: Value =
        serde_json::from_str(&offline.browse("Recent Songs".into(), "".into()).unwrap()).unwrap();
    assert_eq!(page["rows"][0]["id"], "local");
    let epoch = set(&offline, json!({"id":"offline","duration":60}));
    report(&offline, epoch, "offline", 40., 60., true);
    let deadline = Instant::now() + Duration::from_secs(3);
    while recent(&offline).len() < 2 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(recent(&offline)[0]["id"], "offline");
}

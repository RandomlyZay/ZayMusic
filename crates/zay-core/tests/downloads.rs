use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    sync::Arc,
};
use zay_core::NativeCore;
fn fixture() -> (Arc<NativeCore>, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("zay-download-{}", rand::random::<u64>()));
    (
        NativeCore::new(
            "https://example.invalid".into(),
            dir.to_string_lossy().into(),
        )
        .unwrap(),
        dir,
    )
}
fn plan(core: &NativeCore, id: &str) -> Value {
    serde_json::from_str(
        &core
            .queue_download(json!({"id":id,"title":id,"suffix":"wav"}).to_string())
            .unwrap(),
    )
    .unwrap()
}
fn record(core: &NativeCore, id: &str) -> Value {
    let rows: Value = serde_json::from_str(&core.downloads().unwrap()).unwrap();
    rows.as_array()
        .unwrap()
        .iter()
        .find(|v| v["song"]["id"] == id)
        .unwrap()
        .clone()
}
#[test]
fn completion_is_offline_and_progress_is_not_ready_early() {
    let (core, dir) = fixture();
    let p = plan(&core, "fake");
    let ticket = p["ticket"].as_u64().unwrap();
    assert!(core
        .begin_download("fake".into(), ticket, "ios:1".into())
        .unwrap());
    core.download_progress("fake".into(), ticket, 4, 4).unwrap();
    assert_eq!(record(&core, "fake")["progress"], 0.99);
    assert_eq!(record(&core, "fake")["state"], "downloading");
    let temp = dir.join("input");
    std::fs::write(&temp, b"fake audio bytes").unwrap();
    core.complete_download("fake".into(), ticket, temp.to_string_lossy().into())
        .unwrap();
    let v = record(&core, "fake");
    assert_eq!(v["state"], "ready");
    assert_eq!(v["progress"], 1.0);
    let url = core
        .media_url(json!({"id":"fake"}).to_string(), false)
        .unwrap();
    assert!(url.starts_with("file:"));
    assert_eq!(
        std::fs::read(url::Url::parse(&url).unwrap().to_file_path().unwrap()).unwrap(),
        b"fake audio bytes"
    );
    assert!(url.ends_with(".wav"));
    drop(core);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn late_completion_cannot_resurrect_cancelled_or_deleted_downloads() {
    let (core, dir) = fixture();
    let first = plan(&core, "fake");
    let ticket = first["ticket"].as_u64().unwrap();
    core.begin_download("fake".into(), ticket, "ios:1".into())
        .unwrap();
    let temp = dir.join("input");
    std::fs::write(&temp, b"fake").unwrap();
    core.cancel_download("fake".into()).unwrap();
    assert!(core
        .complete_download("fake".into(), ticket, temp.to_string_lossy().into())
        .is_err());
    assert_eq!(record(&core, "fake")["state"], "cancelled");
    core.delete_download("fake".into()).unwrap();
    let retry = plan(&core, "fake");
    assert_ne!(retry["ticket"], first["ticket"]);
    assert!(core
        .complete_download("fake".into(), ticket, temp.to_string_lossy().into())
        .is_err());
    assert_eq!(record(&core, "fake")["state"], "queued");
    drop(core);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn duplicate_requests_share_a_ticket_and_only_one_can_start() {
    let (core, dir) = fixture();
    let first = plan(&core, "fake");
    let second = plan(&core, "fake");
    assert_eq!(first["ticket"], second["ticket"]);
    let t = first["ticket"].as_u64().unwrap();
    assert!(core
        .begin_download("fake".into(), t, "ios:1".into())
        .unwrap());
    assert!(!core
        .begin_download("fake".into(), t, "ios:2".into())
        .unwrap());
    drop(core);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn failed_download_can_retry_without_old_progress_overwriting_it() {
    let (core, dir) = fixture();
    let first = plan(&core, "fake");
    let old = first["ticket"].as_u64().unwrap();
    core.begin_download("fake".into(), old, "ios:1".into())
        .unwrap();
    core.fail_download("fake".into(), old, "transport".into())
        .unwrap();
    let mut metadata = json!({"id":"fake","title":"Fake","download":{"state":"failed"},"queueLane":"main","queueIndex":1});
    let next: Value =
        serde_json::from_str(&core.queue_download(metadata.take().to_string()).unwrap()).unwrap();
    assert!(record(&core, "fake")["song"]["download"].is_null());
    assert!(record(&core, "fake")["song"]["queueLane"].is_null());
    let new = next["ticket"].as_u64().unwrap();
    assert_ne!(new, old);
    core.begin_download("fake".into(), new, "ios:2".into())
        .unwrap();
    core.download_progress("fake".into(), old, 99, 100).unwrap();
    assert_eq!(record(&core, "fake")["bytes"], 0);
    core.download_progress("fake".into(), new, 5, 10).unwrap();
    assert_eq!(record(&core, "fake")["progress"], 0.5);
    drop(core);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn recovery_preserves_os_owned_transfers_and_fails_orphans() {
    let (core, dir) = fixture();
    let a = plan(&core, "active");
    let b = plan(&core, "orphan");
    core.begin_download(
        "active".into(),
        a["ticket"].as_u64().unwrap(),
        "ios:1".into(),
    )
    .unwrap();
    core.begin_download(
        "orphan".into(),
        b["ticket"].as_u64().unwrap(),
        "ios:2".into(),
    )
    .unwrap();
    core.recover_downloads(json!([{"id":"active","ticket":a["ticket"]}]).to_string())
        .unwrap();
    assert_eq!(record(&core, "active")["state"], "downloading");
    assert_eq!(record(&core, "orphan")["state"], "failed");
    drop(core);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn empty_completion_never_enters_ready_catalog() {
    let (core, dir) = fixture();
    let p = plan(&core, "fake");
    let temp = dir.join("empty");
    std::fs::write(&temp, b"").unwrap();
    assert!(core
        .complete_download(
            "fake".into(),
            p["ticket"].as_u64().unwrap(),
            temp.to_string_lossy().into()
        )
        .is_err());
    assert_ne!(record(&core, "fake")["state"], "ready");
    assert_eq!(std::fs::read_dir(dir.join("audio")).unwrap().count(), 0);
    drop(core);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn deleting_offline_files_keeps_settings_and_playback() {
    let (core, dir) = fixture();
    let temp = dir.join("input");
    std::fs::write(&temp, b"fake").unwrap();
    core.register_download(
        json!({"id":"fake","title":"Fake"}).to_string(),
        temp.to_string_lossy().into(),
    )
    .unwrap();
    core.setting("volume".into(), Some("0.4".into())).unwrap();
    core.queue_command(json!({"op":"set","songs":[{"id":"fake","title":"Fake"}]}).to_string())
        .unwrap();
    core.delete_all_downloads().unwrap();
    assert_eq!(core.downloads().unwrap(), "[]");
    assert_eq!(core.setting("volume".into(), None).unwrap(), "0.4");
    assert!(core.queue_command("{}".into()).unwrap().contains("Fake"));
    drop(core);
    std::fs::remove_dir_all(dir).unwrap();
}
fn http(
    body: Vec<u8>,
    content_type: &'static str,
    declared: usize,
) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let work = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut first = String::new();
        reader.read_line(&mut first).unwrap();
        assert!(first.contains("/rest/download?id=fake"));
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
        }
        write!(stream,"HTTP/1.1 200 OK\r\nContent-Length: {declared}\r\nContent-Type: {content_type}\r\nConnection: close\r\n\r\n").unwrap();
        let _ = stream.write_all(&body);
    });
    (base, work)
}
#[test]
fn desktop_transfer_streams_into_a_native_file() {
    let body = vec![7_u8; 150000];
    let (base, work) = http(body.clone(), "audio/wav", body.len());
    let dir = std::env::temp_dir().join(format!("zay-http-{}", rand::random::<u64>()));
    let core = NativeCore::new(base, dir.to_string_lossy().into()).unwrap();
    core.download(json!({"id":"fake","title":"Fake","suffix":"wav"}).to_string())
        .unwrap();
    work.join().unwrap();
    let url = core
        .media_url(json!({"id":"fake"}).to_string(), false)
        .unwrap();
    assert_eq!(
        std::fs::read(url::Url::parse(&url).unwrap().to_file_path().unwrap()).unwrap(),
        body
    );
    assert_eq!(record(&core, "fake")["bytes"], 150000);
    drop(core);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn truncated_transfer_is_failed_and_never_playable_offline() {
    let (base, work) = http(b"partial".to_vec(), "audio/wav", 1000);
    let dir = std::env::temp_dir().join(format!("zay-short-{}", rand::random::<u64>()));
    let core = NativeCore::new(base, dir.to_string_lossy().into()).unwrap();
    assert!(core
        .download(json!({"id":"fake","title":"Fake"}).to_string())
        .is_err());
    work.join().unwrap();
    assert_eq!(record(&core, "fake")["state"], "failed");
    assert_eq!(std::fs::read_dir(dir.join("audio")).unwrap().count(), 0);
    drop(core);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn server_error_json_is_not_saved_as_audio() {
    let (base, work) = http(b"{}".to_vec(), "application/json", 2);
    let dir = std::env::temp_dir().join(format!("zay-json-{}", rand::random::<u64>()));
    let core = NativeCore::new(base, dir.to_string_lossy().into()).unwrap();
    assert!(core
        .download(json!({"id":"fake","title":"Fake"}).to_string())
        .is_err());
    work.join().unwrap();
    assert_eq!(record(&core, "fake")["state"], "failed");
    drop(core);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn legacy_catalog_migrates_without_losing_ready_files() {
    let dir = std::env::temp_dir().join(format!("zay-legacy-{}", rand::random::<u64>()));
    std::fs::create_dir_all(dir.join("audio")).unwrap();
    std::fs::write(dir.join("audio/legacy.media"), b"data").unwrap();
    {
        let db = rusqlite::Connection::open(dir.join("catalog.sqlite")).unwrap();
        db.execute_batch("CREATE TABLE downloads(id TEXT PRIMARY KEY,state TEXT NOT NULL,path TEXT NOT NULL,meta TEXT NOT NULL,bytes INTEGER NOT NULL DEFAULT 0,error TEXT)").unwrap();
        db.execute(
            "INSERT INTO downloads VALUES('fake','ready','audio/legacy.media',?1,4,NULL)",
            [json!({"id":"fake","title":"Legacy"}).to_string()],
        )
        .unwrap();
    }
    let core = NativeCore::new(
        "https://example.invalid".into(),
        dir.to_string_lossy().into(),
    )
    .unwrap();
    assert_eq!(record(&core, "fake")["progress"], 1.0);
    let stats: Value = serde_json::from_str(&core.storage_stats().unwrap()).unwrap();
    assert_eq!(stats["readyBytes"], 4);
    assert!(core
        .media_url(json!({"id":"fake"}).to_string(), false)
        .unwrap()
        .starts_with("file:"));
    drop(core);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn restart_recovers_interrupted_desktop_transfers_and_partial_files() {
    let (core, dir) = fixture();
    let a = plan(&core, "desktop");
    let b = plan(&core, "ios");
    core.begin_download(
        "desktop".into(),
        a["ticket"].as_u64().unwrap(),
        "desktop:fake".into(),
    )
    .unwrap();
    core.begin_download("ios".into(), b["ticket"].as_u64().unwrap(), "ios:1".into())
        .unwrap();
    std::fs::write(dir.join("audio/6661-1.part"), b"partial").unwrap();
    drop(core);
    let core = NativeCore::new(
        "https://example.invalid".into(),
        dir.to_string_lossy().into(),
    )
    .unwrap();
    assert_eq!(record(&core, "desktop")["state"], "failed");
    assert_eq!(record(&core, "ios")["state"], "downloading");
    assert!(!dir.join("audio/6661-1.part").exists());
    drop(core);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn bulk_download_persists_the_batch_and_respects_cancellation_before_start() {
    use std::sync::{mpsc, Mutex};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (start_tx, started) = mpsc::channel();
    let (release, release_rx) = mpsc::channel();
    let release_rx = Arc::new(Mutex::new(release_rx));
    let server = std::thread::spawn(move || {
        let mut handles = vec![];
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            let started = start_tx.clone();
            let gate = release_rx.clone();
            handles.push(std::thread::spawn(move||{let mut reader=BufReader::new(stream.try_clone().unwrap());loop{let mut line=String::new();reader.read_line(&mut line).unwrap();if line=="\r\n"{break;}}started.send(()).unwrap();gate.lock().unwrap().recv_timeout(std::time::Duration::from_secs(5)).unwrap();write!(stream,"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nContent-Type: audio/wav\r\nConnection: close\r\n\r\nDATA").unwrap();}));
        }
        for handle in handles {
            handle.join().unwrap();
        }
    });
    let dir = std::env::temp_dir().join(format!("zay-bulk-{}", rand::random::<u64>()));
    let core = NativeCore::new(base, dir.to_string_lossy().into()).unwrap();
    let c = core.clone();
    let worker = std::thread::spawn(move || {
        c.download_many(
            json!([{"id":"a","title":"A"},{"id":"b","title":"B"},{"id":"c","title":"C"}])
                .to_string(),
        )
        .unwrap()
    });
    for _ in 0..2 {
        started
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
    }
    let stats: Value = serde_json::from_str(&core.storage_stats().unwrap()).unwrap();
    assert_eq!(stats["downloading"], 2);
    assert_eq!(stats["queued"], 1);
    core.cancel_download("c".into()).unwrap();
    release.send(()).unwrap();
    release.send(()).unwrap();
    let result: Value = serde_json::from_str(&worker.join().unwrap()).unwrap();
    server.join().unwrap();
    assert_eq!(result["completed"], 2);
    assert_eq!(result["cancelled"], 1);
    assert_eq!(record(&core, "c")["state"], "cancelled");
    drop(core);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cancellation_interrupts_a_stalled_desktop_request() {
    use std::sync::mpsc;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (start_tx, started) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
        }
        start_tx.send(()).unwrap();
        gate.recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
    });
    let dir = std::env::temp_dir().join(format!("zay-cancel-{}", rand::random::<u64>()));
    let core = NativeCore::new(base, dir.to_string_lossy().into()).unwrap();
    let c = core.clone();
    let (done_tx, done) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        done_tx
            .send(
                c.download(json!({"id":"fake","title":"Fake"}).to_string())
                    .is_err(),
            )
            .unwrap()
    });
    started
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    core.cancel_download("fake".into()).unwrap();
    assert!(done
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("Cancellation must not await the stalled HTTP response"));
    release.send(()).unwrap();
    worker.join().unwrap();
    server.join().unwrap();
    assert_eq!(record(&core, "fake")["state"], "cancelled");
    drop(core);
    std::fs::remove_dir_all(dir).unwrap();
}

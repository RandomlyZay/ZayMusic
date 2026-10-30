use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
use zay_core::NativeCore;
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Root(std::path::PathBuf);
impl Root {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
            "zay-management-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )))
    }
    fn core(&self, url: &str) -> std::sync::Arc<NativeCore> {
        NativeCore::new(url.into(), self.0.to_string_lossy().into()).unwrap()
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn server(responses: Vec<(&'static str, Value)>) -> (String, std::thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let worker = std::thread::spawn(move || {
        let mut captured = vec![];
        for (expected, value) in responses {
            let deadline = Instant::now() + Duration::from_secs(5);
            let (mut s, _) = loop {
                match listener.accept() {
                    Ok(v) => break v,
                    Err(_) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(2))
                    }
                    Err(e) => panic!("Expected fixture request {expected}: {e}"),
                }
            };
            s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut head = String::new();
            r.read_line(&mut head).unwrap();
            assert!(head.contains(expected), "Unexpected fixture route {head}");
            let mut len = 0;
            loop {
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                if let Some(v) = line.to_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap()
                }
                head.push_str(&line);
            }
            let mut body = vec![0; len];
            r.read_exact(&mut body).unwrap();
            head.push_str(&String::from_utf8(body).unwrap());
            captured.push(head);
            let body = value.to_string();
            write!(s,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
        }
        captured
    });
    (url, worker)
}
fn ok() -> Value {
    json!({"subsonic-response":{"status":"ok"}})
}
#[test]
fn persisted_volume_scales_replaygain_and_rejects_nonfinite_values() {
    let root = Root::new();
    let core = root.core("http://127.0.0.1:9");
    core.queue_command(
        json!({"op":"set","songs":[{"id":"fake","replayGain":{"trackGain":-6}}]}).to_string(),
    )
    .unwrap();
    core.set_volume(0.5).unwrap();
    let q: Value = serde_json::from_str(&core.queue_command("{}".into()).unwrap()).unwrap();
    assert!((q["gain"].as_f64().unwrap() - 0.5 * 10f64.powf(-6. / 20.)).abs() < 1e-9);
    assert!(core.set_volume(f64::NAN).is_err());
    drop(core);
    let core = root.core("http://127.0.0.1:9");
    assert_eq!(core.volume(), 0.5);
    core.set_volume(2.).unwrap();
    assert_eq!(core.volume(), 1.);
    core.set_volume(-1.).unwrap();
    assert_eq!(core.volume(), 0.);
}
#[test]
fn preferences_validate_public_addresses_and_have_safe_defaults() {
    let root = Root::new();
    let core = root.core("https://example.invalid");
    let p: Value = serde_json::from_str(&core.preferences().unwrap()).unwrap();
    assert_eq!(
        p["remoteBrowserUrl"],
        "https://example.invalid:8446/vnc.html"
    );
    for v in [
        "file:///tmp/x",
        "https://fake:password@example.invalid",
        "https://example.invalid/?token=fake",
    ] {
        assert!(core.set_preference("server".into(), v.into()).is_err());
    }
    assert!(core
        .set_preference("cookies".into(), "fake".into())
        .is_err());
    core.set_preference(
        "remoteBrowserUrl".into(),
        "https://browser.invalid/vnc.html".into(),
    )
    .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&core.preferences().unwrap()).unwrap()["remoteBrowserUrl"],
        "https://browser.invalid/vnc.html"
    );
}
#[test]
fn save_and_unsave_use_song_album_and_artist_keys_and_clear_cached_pages() {
    let root = Root::new();
    let (url, worker) = server(vec![
        ("/rest/star", ok()),
        ("/rest/unstar", ok()),
        ("/rest/star", ok()),
    ]);
    let core = root.core(&url);
    core.setting("browse:fake".into(), Some("cached".into()))
        .unwrap();
    for (kind, op) in [("songs", "save"), ("albums", "unsave"), ("artists", "save")] {
        core.library_command(json!({"op":op,"kind":kind,"item":{"id":"a & b"}}).to_string())
            .unwrap();
    }
    let captured = worker.join().unwrap();
    for (i, key) in ["id", "albumId", "artistId"].into_iter().enumerate() {
        let first = captured[i].lines().next().unwrap();
        let url = url::Url::parse(&format!(
            "http://fixture.invalid{}",
            first.split_whitespace().nth(1).unwrap()
        ))
        .unwrap();
        assert_eq!(
            url.query_pairs().find(|(k, _)| k == key).unwrap().1,
            "a & b"
        );
    }
    assert_eq!(core.setting("browse:fake".into(), None).unwrap(), "null");
}
#[test]
fn jobs_and_additive_import_actions_use_the_existing_contract() {
    let root = Root::new();
    let (url, worker) = server(vec![
        ("POST /api/fetch/jobs/fake%2Fid/retry", json!({})),
        ("POST /api/fetch/jobs/fake%2Fid/cancel", json!({})),
        ("DELETE /api/fetch/jobs/fake%2Fid", json!({})),
        ("POST /api/fetch/youtube/imports/fake%2Fid/pull", json!({})),
        ("POST /api/fetch/youtube/imports/fake%2Fid/sync", json!({})),
        ("POST /api/fetch/youtube/imports ", json!({})),
    ]);
    let core = root.core(&url);
    for op in [
        "retryJob",
        "cancelJob",
        "deleteJob",
        "pullImport",
        "syncImport",
        "importPlaylist",
    ] {
        core.management_command(json!({"op":op,"id":"fake/id"}).to_string())
            .unwrap();
    }
    let requests = worker.join().unwrap();
    assert!(requests
        .iter()
        .all(|r| r.to_lowercase().contains(&format!("origin: {url}"))));
    assert!(requests[5].ends_with("{\"playlistId\":\"fake/id\"}"));
    assert!(core
        .management_command(json!({"op":"deleteImport","id":"fake"}).to_string())
        .is_err());
}
#[test]
fn manual_cookies_are_transmitted_without_client_persistence() {
    let root = Root::new();
    let (url, worker) = server(vec![(
        "POST /api/fetch/youtube/cookies ",
        json!({"configured":true}),
    )]);
    let core = root.core(&url);
    core.management_command(
        json!({"op":"saveCookies","cookies":"fake-cookie-fixture"}).to_string(),
    )
    .unwrap();
    assert!(worker.join().unwrap()[0].ends_with("{\"cookies\":\"fake-cookie-fixture\"}"));
    let db = rusqlite::Connection::open(root.0.join("catalog.sqlite")).unwrap();
    let count: i64 = db
        .query_row(
            "SELECT count(*) FROM kv WHERE value LIKE '%fake-cookie-fixture%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}
#[test]
fn status_whitelists_fields_and_keeps_oauth_codes_memory_only() {
    let root = Root::new();
    let (url, worker) = server(vec![
        (
            "POST /api/fetch/youtube/oauth/start",
            json!({"user_code":"FAKE-CODE","verification_url":"https://example.invalid/verify","expires_in":600,"device_code":"must-not-escape"}),
        ),
        (
            "GET /api/fetch/youtube/cookies",
            json!({"configured":true,"oauth":{"configured":true,"connected":false},"email":"must-not-escape","userId":"must-not-escape"}),
        ),
        ("DELETE /api/fetch/youtube/oauth", json!({})),
    ]);
    let core = root.core(&url);
    let started = core
        .management_command("{\"op\":\"oauthStart\"}".into())
        .unwrap();
    assert!(!started.contains("FAKE-CODE"));
    let status = core.management_status().unwrap();
    assert!(!status.contains("must-not-escape"));
    assert_eq!(
        serde_json::from_str::<Value>(&status).unwrap()["oauthFlow"]["user_code"],
        "FAKE-CODE"
    );
    core.management_command("{\"op\":\"oauthDisconnect\"}".into())
        .unwrap();
    worker.join().unwrap();
    let db = rusqlite::Connection::open(root.0.join("catalog.sqlite")).unwrap();
    let count: i64 = db
        .query_row(
            "SELECT count(*) FROM kv WHERE value LIKE '%FAKE-CODE%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}
#[test]
fn recent_contexts_deduplicate_sort_bound_and_survive_restart() {
    let root = Root::new();
    let core = root.core("http://127.0.0.1:9");
    for i in 0..101 {
        core.record_context(
            json!({"kind":"playlists","id":format!("p{i}"),"name":format!("Playlist {i}")})
                .to_string(),
        )
        .unwrap();
    }
    core.record_context(json!({"kind":"playlists","id":"p50","name":"Updated"}).to_string())
        .unwrap();
    core.record_context(json!({"kind":"search","id":"not-a-context"}).to_string())
        .unwrap();
    let rows: Vec<Value> = serde_json::from_str(&core.recent_contexts().unwrap()).unwrap();
    assert_eq!(rows.len(), 100);
    assert_eq!(rows[0]["name"], "Updated");
    assert_eq!(rows[1]["id"], "p100");
    drop(core);
    assert_eq!(
        serde_json::from_str::<Vec<Value>>(
            &root.core("http://127.0.0.1:9").recent_contexts().unwrap()
        )
        .unwrap(),
        rows
    );
}
#[test]
fn offline_acquisition_failures_can_be_retried_and_cleared() {
    let root = Root::new();
    let core = root.core("http://127.0.0.1:9");
    assert!(core
        .acquire_song(json!({"id":"yt:fake-video","title":"Fake title"}).to_string())
        .is_err());
    let page: Value =
        serde_json::from_str(&core.browse("Jobs".into(), "".into()).unwrap()).unwrap();
    assert_eq!(page["rows"][0]["id"], "queue:yt:fake-video");
    drop(core);
    let (url, worker) = server(vec![(
        "POST /api/fetch/jobs ",
        json!({"job":{"id":"fake-job"}}),
    )]);
    let core = root.core(&url);
    core.management_command(json!({"op":"retryJob","id":"queue:yt:fake-video"}).to_string())
        .unwrap();
    worker.join().unwrap();
    assert_eq!(core.acquisition_failures().unwrap(), "[]");
}
#[test]
fn settings_page_cache_never_persists_oauth_codes() {
    let root = Root::new();
    let (url, worker) = server(vec![
        (
            "POST /api/fetch/youtube/oauth/start",
            json!({"user_code":"FAKE-CODE","verification_url":"https://example.invalid/verify","expires_in":600}),
        ),
        (
            "GET /api/fetch/youtube/cookies",
            json!({"configured":true,"oauth":{"configured":true,"connected":false}}),
        ),
    ]);
    let core = root.core(&url);
    core.management_command("{\"op\":\"oauthStart\"}".into())
        .unwrap();
    let page: Value =
        serde_json::from_str(&core.browse("Settings".into(), "".into()).unwrap()).unwrap();
    assert_eq!(page["extra"]["oauthFlow"]["user_code"], "FAKE-CODE");
    worker.join().unwrap();
    let db = rusqlite::Connection::open(root.0.join("catalog.sqlite")).unwrap();
    let count: i64 = db
        .query_row(
            "SELECT count(*) FROM kv WHERE value LIKE '%FAKE-CODE%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 0,
        "The offline Settings cache must not keep device codes"
    );
}
#[test]
fn attention_summary_refreshes_server_failures_without_opening_jobs() {
    let root = Root::new();
    let (url, worker) = server(vec![(
        "GET /api/fetch/jobs?limit=200",
        json!({"jobs":[{"id":"a","status":"failed"},{"id":"b","status":"error"},{"id":"c","status":"running"},{"id":"d","status":"done"}]}),
    )]);
    let core = root.core(&url);
    let first: Value = serde_json::from_str(&core.attention_summary(false).unwrap()).unwrap();
    assert_eq!(first["total"], 0);
    let fresh: Value = serde_json::from_str(&core.attention_summary(true).unwrap()).unwrap();
    assert_eq!((fresh["server"].as_u64(), fresh["total"].as_u64()), (Some(2), Some(2)));
    worker.join().unwrap();
    // The server is now gone: the last known count stays rather than dropping to zero.
    let offline: Value = serde_json::from_str(&core.attention_summary(true).unwrap()).unwrap();
    assert_eq!(offline["server"], 2);
}

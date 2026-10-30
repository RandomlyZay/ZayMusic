use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    sync::atomic::{AtomicU64, Ordering},
};
use zay_core::{
    catalog::{classify, preview, Input},
    NativeCore,
};
static NEXT: AtomicU64 = AtomicU64::new(0);
fn directory() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "zay-catalog-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}
fn server(responses: Vec<(&'static str, Value)>) -> (String, std::thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        let mut captured = vec![];
        for (expected, response) in responses {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut first = String::new();
            reader.read_line(&mut first).unwrap();
            assert!(first.contains(expected), "Unexpected fixture request");
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                if let Some(value) = line.to_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap();
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            captured.push(String::from_utf8(body).unwrap());
            let body = response.to_string();
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
        }
        captured
    });
    (base, handle)
}
macro_rules! classification {
    ($name:ident,$text:expr,$variant:ident) => {
        #[test]
        fn $name() {
            assert!(matches!(classify($text), Input::$variant { .. }));
        }
    };
}
classification!(empty, "  ", Empty);
classification!(text, "radiohead ok computer", LibraryQuery);
classification!(watch, "https://www.youtube.com/watch?v=fake", VideoUrl);
classification!(short_link, "https://youtu.be/fake", VideoUrl);
classification!(
    music_watch,
    "https://music.youtube.com/watch?v=fake",
    VideoUrl
);
classification!(
    playlist,
    "https://www.youtube.com/playlist?list=PLfake",
    PlaylistUrl
);
classification!(
    watch_list,
    "https://www.youtube.com/watch?v=fake&list=PLfake",
    PlaylistUrl
);
classification!(album, "https://music.youtube.com/browse/MPREfake", AlbumUrl);
classification!(shorts, "https://www.youtube.com/shorts/fake", VideoUrl);
classification!(
    external,
    "https://example.invalid/watch?v=fake",
    LibraryQuery
);
classification!(schemeless, "youtube.com/watch?v=fake", VideoUrl);
classification!(
    spoofed_host,
    "youtube.com.example.invalid/watch?v=fake",
    LibraryQuery
);
#[test]
fn candidate_carries_album_position_and_preview_metadata() {
    let value = preview(
        &json!({"videoId":"fake-video","title":"A & B","channel":"Fake Artist","album":"Fake Album","track":7,"durationSec":180,"thumbnails":[{"url":"https://example.invalid/art"}]}),
    );
    assert_eq!(value["id"], "yt:fake-video");
    assert_eq!(value["track"], 7);
    assert_eq!(value["album"], "Fake Album");
    let url = url::Url::parse(&format!(
        "https://example.invalid{}",
        value["previewUrl"].as_str().unwrap()
    ))
    .unwrap();
    assert!(url.query_pairs().any(|(k, v)| k == "title" && v == "A & B"));
}
#[test]
fn browse_falls_back_to_cached_library_when_server_is_offline() {
    let (base, handle) = server(vec![(
        "/rest/getStarred2",
        json!({"subsonic-response":{"status":"ok","starred2":{"song":[{"id":"fake","title":"Saved"}]}}}),
    )]);
    let dir = directory();
    let core = NativeCore::new(base, dir.to_string_lossy().into()).unwrap();
    let online: Value =
        serde_json::from_str(&core.browse("Liked Songs".into(), "".into()).unwrap()).unwrap();
    assert_eq!(online["rows"][0]["id"], "fake");
    handle.join().unwrap();
    let offline: Value =
        serde_json::from_str(&core.browse("Liked Songs".into(), "".into()).unwrap()).unwrap();
    assert_eq!(offline["offline"], true);
    assert_eq!(offline["rows"], online["rows"]);
    drop(core);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn shared_album_lookup_restores_missing_track_numbers() {
    let (base, handle) = server(vec![
        (
            "/rest/getAlbum",
            json!({"subsonic-response":{"status":"ok","album":{"name":"Fixture Album","song":[{"id":"second","title":"Second"},{"id":"first","title":"First"}]}}}),
        ),
        (
            "/api/fetch/library/albums/fixture/order",
            json!({"tracks":{"first":1,"second":2}}),
        ),
    ]);
    let dir = directory();
    let core = NativeCore::new(base, dir.to_string_lossy().into()).unwrap();
    let v: Value = serde_json::from_str(
        &core
            .open_collection("albums".into(), "fixture".into())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(v["rows"][0]["id"], "first");
    assert_eq!(v["rows"][1]["track"], 2);
    handle.join().unwrap();
    drop(core);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn new_music_uses_valid_source_and_explicit_acquisition() {
    let (base, handle) = server(vec![
        (
            "source=all",
            json!({"candidates":[{"videoId":"fake","title":"Title","channel":"Artist","album":"Album","track":2,"durationSec":120}]}),
        ),
        (
            "/api/fetch/jobs",
            json!({"job":{"id":"fake-job","status":"queued"}}),
        ),
    ]);
    let dir = directory();
    let core = NativeCore::new(base, dir.to_string_lossy().into()).unwrap();
    let value: Value = serde_json::from_str(
        &core
            .browse("New music".into(), "Fake Query".into())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(value["rows"][0]["id"], "yt:fake");
    core.acquire_song(value["rows"][0].to_string()).unwrap();
    let requests = handle.join().unwrap();
    let body: Value = serde_json::from_str(&requests[1]).unwrap();
    assert_eq!(body["overrides"]["track"], 2);
    assert_eq!(body["videoId"], "fake");
    drop(core);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn empty_new_music_does_not_contact_server() {
    let dir = directory();
    let core = NativeCore::new("http://127.0.0.1:9".into(), dir.to_string_lossy().into()).unwrap();
    let value: Value =
        serde_json::from_str(&core.browse("New music".into(), "".into()).unwrap()).unwrap();
    assert_eq!(value["rows"], json!([]));
    drop(core);
    std::fs::remove_dir_all(dir).unwrap();
}

use std::{
    io::{BufRead, BufReader, Write},
    net::TcpListener,
};
use zay_core::NativeCore;

/// Serves each scripted response once, capturing the request line.
fn server(
    responses: Vec<(u16, &'static str, &'static [u8])>,
) -> (String, std::thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        let mut captured = vec![];
        for (status, kind, body) in responses {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut first = String::new();
            reader.read_line(&mut first).unwrap();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
            }
            captured.push(first);
            write!(stream, "HTTP/1.1 {status} X\r\ncontent-type: {kind}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", body.len()).unwrap();
            stream.write_all(body).unwrap();
        }
        captured
    });
    (base, handle)
}
fn directory(name: &str) -> String {
    let path = std::env::temp_dir().join(format!("zay-art-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    path.to_string_lossy().into_owned()
}

#[test]
fn artwork_uses_gateway_cover_art_and_caches_on_disk() {
    let (base, handle) = server(vec![(200, "image/png", b"fake-png")]);
    let dir = directory("cache");
    let core = NativeCore::new(base.clone(), dir.clone()).unwrap();
    let song = r#"{"id":"s1","title":"Song","coverArt":"al-1","albumId":"a1"}"#;
    assert_eq!(core.artwork(song.into(), 96).unwrap(), b"fake-png");
    let requests = handle.join().unwrap();
    assert!(requests[0].starts_with("GET /rest/getCoverArt?id=al-1&size=96 "));
    // The fixture server has exited: a second request must come from the cache.
    assert_eq!(core.artwork(song.into(), 96).unwrap(), b"fake-png");
    let reopened = NativeCore::new(base, dir).unwrap();
    assert_eq!(reopened.artwork(song.into(), 96).unwrap(), b"fake-png");
    let local = reopened.artwork_cache_url(song.into(), 300).unwrap();
    let local = url::Url::parse(&local).unwrap().to_file_path().unwrap();
    assert_eq!(std::fs::read(local).unwrap(), b"fake-png");
}

#[test]
fn artwork_misses_are_not_refetched_and_non_images_are_rejected() {
    let (base, handle) = server(vec![
        (404, "application/json", b"{}"),
        (200, "text/html", b"<html>"),
    ]);
    let core = NativeCore::new(base, directory("miss")).unwrap();
    let missing = r#"{"id":"s1","albumId":"missing"}"#;
    assert!(core.artwork(missing.into(), 64).is_err());
    assert!(core.artwork(missing.into(), 64).is_err());
    assert!(core
        .artwork(r#"{"id":"s2","coverArt":"html"}"#.into(), 64)
        .is_err());
    assert_eq!(handle.join().unwrap().len(), 2);
    assert!(core
        .artwork(r#"{"id":"plain","title":"No art"}"#.into(), 64)
        .is_err());
}

#[test]
fn preview_thumbnails_are_fetched_from_their_own_url() {
    let (base, handle) = server(vec![(200, "image/jpeg", b"thumb")]);
    let core = NativeCore::new("https://example.invalid".into(), directory("thumb")).unwrap();
    let preview =
        format!(r#"{{"id":"yt:x","artworkUrl":"{base}/vi/x/hq.jpg","coverArt":"ignored"}}"#);
    assert_eq!(core.artwork(preview, 300).unwrap(), b"thumb");
    assert!(handle.join().unwrap()[0].starts_with("GET /vi/x/hq.jpg "));
}

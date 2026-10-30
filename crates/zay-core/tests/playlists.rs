use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    sync::Arc,
};
use zay_core::NativeCore;
struct Call {
    method: &'static str,
    path: &'static str,
    body: Value,
    response: Value,
}
fn fixture(
    calls: Vec<Call>,
) -> (
    Arc<NativeCore>,
    std::path::PathBuf,
    std::thread::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let dir = std::env::temp_dir().join(format!("zay-playlists-{}", rand::random::<u64>()));
    let core = NativeCore::new(
        format!("http://{}", listener.local_addr().unwrap()),
        dir.to_string_lossy().into(),
    )
    .unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        for call in calls {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut first = String::new();
            reader.read_line(&mut first).unwrap();
            let parts: Vec<_> = first.split_whitespace().collect();
            assert_eq!(parts[0], call.method);
            assert_eq!(parts[1], call.path);
            let mut length = 0;
            let mut content_type = String::new();
            let mut request_origin = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                let lower = line.to_lowercase();
                if let Some(v) = lower.strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap();
                }
                if let Some(v) = lower.strip_prefix("origin:") {
                    request_origin = v.trim().to_owned();
                }
                if let Some(v) = lower.strip_prefix("content-type:") {
                    content_type = v.trim().to_owned();
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            if call.method == "POST" && call.path.starts_with("/rest/") {
                assert_eq!(content_type, "application/x-www-form-urlencoded");
                assert_eq!(request_origin, origin);
                let mut actual = json!({});
                for (k, v) in url::form_urlencoded::parse(&body) {
                    if actual[&*k].is_null() {
                        actual[&*k] = json!([]);
                    }
                    actual[&*k].as_array_mut().unwrap().push(json!(v));
                }
                assert_eq!(actual, call.body);
            } else if !body.is_empty() {
                assert_eq!(serde_json::from_slice::<Value>(&body).unwrap(), call.body);
            }
            let body = if call.path.starts_with("/rest/") {
                json!({"subsonic-response":{"status":"ok"}})
                    .as_object()
                    .map(|_| {
                        let mut v = call.response.clone();
                        v["status"] = json!("ok");
                        json!({"subsonic-response":v})
                    })
                    .unwrap()
            } else {
                call.response
            };
            let text = body.to_string();
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",text.len()).unwrap();
        }
    });
    (core, dir, handle)
}
fn current() -> Value {
    json!({"playlist":{"id":"pl","name":"Fixture","entry":[{"id":"a","title":"A"},{"id":"b","title":"B"},{"id":"c","title":"C"}]}})
}
#[test]
fn create_preserves_repeated_song_ids_in_a_form_body() {
    let (core, dir, server) = fixture(vec![Call {
        method: "POST",
        path: "/rest/createPlaylist",
        body: json!({"name":["Fixture"],"songId":["a","b","c"]}),
        response: current(),
    }]);
    let v: Value = serde_json::from_str(
        &core
            .playlist_command(
                json!({"op":"create","name":" Fixture ","songIds":["a","b","c"]}).to_string(),
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(v["playlist"]["id"], "pl");
    server.join().unwrap();
    drop(core);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn reorder_rewrites_in_place_and_keeps_the_playlist_id() {
    let (core, dir, server) = fixture(vec![
        Call {
            method: "GET",
            path: "/rest/getPlaylist?id=pl",
            body: json!({}),
            response: current(),
        },
        Call {
            method: "POST",
            path: "/rest/createPlaylist",
            body: json!({"playlistId":["pl"],"songId":["c","a","b"]}),
            response: current(),
        },
    ]);
    core.playlist_command(json!({"op":"move","id":"pl","from":2,"to":0,"songId":"c"}).to_string())
        .unwrap();
    server.join().unwrap();
    drop(core);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn stale_row_cannot_remove_a_different_recording() {
    let (core, dir, server) = fixture(vec![Call {
        method: "GET",
        path: "/rest/getPlaylist?id=pl",
        body: json!({}),
        response: current(),
    }]);
    assert!(core
        .playlist_command(json!({"op":"remove","id":"pl","index":1,"songId":"c"}).to_string())
        .is_err());
    server.join().unwrap();
    drop(core);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn removal_uses_the_exact_playlist_index() {
    let (core, dir, server) = fixture(vec![
        Call {
            method: "GET",
            path: "/rest/getPlaylist?id=pl",
            body: json!({}),
            response: current(),
        },
        Call {
            method: "POST",
            path: "/rest/updatePlaylist",
            body: json!({"playlistId":["pl"],"songIndexToRemove":["1"]}),
            response: json!({}),
        },
    ]);
    core.playlist_command(json!({"op":"remove","id":"pl","index":1,"songId":"b"}).to_string())
        .unwrap();
    server.join().unwrap();
    drop(core);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn adding_previews_queues_acquisition_into_the_selected_playlist() {
    let (core, dir, server) = fixture(vec![
        Call {
            method: "POST",
            path: "/api/fetch/jobs",
            body: json!({"videoId":"video","playlistIds":["pl"],"overrides":{"title":"Preview","artist":"Artist","album":"Album","track":2}}),
            response: json!({"job":{"id":"fake-job"}}),
        },
        Call {
            method: "POST",
            path: "/rest/updatePlaylist",
            body: json!({"playlistId":["pl"],"songIdToAdd":["a","b"]}),
            response: json!({}),
        },
    ]);
    let result:Value=serde_json::from_str(&core.playlist_command(json!({"op":"add","id":"pl","songs":[{"id":"yt:video","title":"Preview","artist":"Artist","album":"Album","track":2},{"id":"a","title":"A"},{"id":"b","title":"B"}]}).to_string()).unwrap()).unwrap();
    assert_eq!(result["queued"], 1);
    assert_eq!(result["added"], 2);
    server.join().unwrap();
    drop(core);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn deleting_a_playlist_also_forgets_a_linked_import() {
    let (core, dir, server) = fixture(vec![
        Call {
            method: "POST",
            path: "/rest/deletePlaylist",
            body: json!({"id":["pl"]}),
            response: json!({}),
        },
        Call {
            method: "GET",
            path: "/api/fetch/youtube/imports",
            body: json!({}),
            response: json!({"imports":[{"id":"link","playlistId":"pl"}]}),
        },
        Call {
            method: "DELETE",
            path: "/api/fetch/youtube/imports/link",
            body: json!({}),
            response: json!({"ok":true}),
        },
    ]);
    core.playlist_command(json!({"op":"delete","id":"pl"}).to_string())
        .unwrap();
    server.join().unwrap();
    drop(core);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn rename_invalidates_cached_playlist_listing() {
    let (core, dir, server) = fixture(vec![
        Call {
            method: "GET",
            path: "/rest/getPlaylists",
            body: json!({}),
            response: json!({"playlists":{"playlist":[{"id":"pl","name":"Old"}]}}),
        },
        Call {
            method: "POST",
            path: "/rest/updatePlaylist",
            body: json!({"playlistId":["pl"],"name":["New"]}),
            response: json!({}),
        },
    ]);
    core.browse("Playlists".into(), "".into()).unwrap();
    core.playlist_command(json!({"op":"rename","id":"pl","name":"New"}).to_string())
        .unwrap();
    server.join().unwrap();
    assert!(core.browse("Playlists".into(), "".into()).is_err());
    drop(core);
    std::fs::remove_dir_all(dir).unwrap();
}

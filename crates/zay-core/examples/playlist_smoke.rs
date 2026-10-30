//! Destructive targets are limited to a playlist created by this invocation.
use serde_json::{json, Value};
use std::sync::Arc;
use zay_core::NativeCore;
struct Cleanup {
    core: Arc<NativeCore>,
    id: String,
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        if self
            .core
            .playlist_command(json!({"op":"delete","id":self.id}).to_string())
            .is_err()
        {
            eprintln!("Temporary native-validation playlist cleanup failed; remove the Native validation playlist in the dev app.");
        }
    }
}
fn main() {
    let dir = std::env::temp_dir().join(format!("zay-playlist-smoke-{}", std::process::id()));
    let core = NativeCore::new(
        std::env::var("ZAY_SERVER").unwrap_or_else(|_| "http://127.0.0.1:3847".into()),
        dir.to_string_lossy().into(),
    )
    .unwrap();
    let listing: Value =
        serde_json::from_str(&core.browse("Songs".into(), "".into()).unwrap()).unwrap();
    let songs: Vec<_> = listing["rows"]
        .as_array()
        .unwrap()
        .iter()
        .take(3)
        .cloned()
        .collect();
    assert_eq!(
        songs.len(),
        3,
        "Three development library tracks are needed"
    );
    let created: Value = serde_json::from_str(
        &core
            .playlist_command(
                json!({"op":"create","name":format!("Native validation {}",std::process::id())})
                    .to_string(),
            )
            .unwrap(),
    )
    .unwrap();
    let id = created["playlist"]["id"].as_str().unwrap().to_owned();
    let cleanup = Cleanup {
        core: core.clone(),
        id: id.clone(),
    };
    let command = |v: Value| {
        core.playlist_command(v.to_string()).unwrap();
    };
    command(json!({"op":"add","id":id,"songs":songs}));
    let ids = || -> Vec<String> {
        let v: Value = serde_json::from_str(
            &core
                .open_collection("playlists".into(), id.clone())
                .unwrap(),
        )
        .unwrap();
        v["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["id"].as_str().unwrap().to_owned())
            .collect()
    };
    let expected: Vec<String> = songs
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        ids() == expected,
        "Repeated form fields must preserve every song"
    );
    command(json!({"op":"rename","id":id,"name":"Native validation renamed"}));
    command(json!({"op":"move","id":id,"from":2,"to":0,"songId":expected[2]}));
    assert!(
        ids()
            == vec![
                expected[2].clone(),
                expected[0].clone(),
                expected[1].clone()
            ],
        "Playlist order differed after move"
    );
    command(json!({"op":"remove","id":id,"index":1,"songId":expected[0]}));
    assert!(
        ids() == vec![expected[2].clone(), expected[1].clone()],
        "Playlist order differed after removal"
    );
    drop(cleanup);
    let listing: Value =
        serde_json::from_str(&core.browse("Playlists".into(), "".into()).unwrap()).unwrap();
    assert!(!listing["rows"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["id"] == id));
    println!("Development playlist create/add/rename/reorder/remove/delete passed; temporary playlist removed");
    drop(core);
    std::fs::remove_dir_all(dir).unwrap();
}

use serde_json::{json, Value};
use zay_core::NativeCore;
fn main() {
    let directory = std::env::temp_dir().join(format!("zay-native-smoke-{}", std::process::id()));
    let core = NativeCore::new(
        "http://127.0.0.1:3847".into(),
        directory.to_string_lossy().into(),
    )
    .unwrap();
    for (service, endpoint, parameters) in [
        ("rest", "ping", json!({})),
        ("rest", "getPlaylists", json!({})),
        ("rest", "getStarred2", json!({})),
        ("rest", "search3", json!({"query":"","songCount":5})),
        ("fetch", "home", json!({})),
    ] {
        let response = core
            .api(
                service.into(),
                endpoint.into(),
                "GET".into(),
                parameters.to_string(),
            )
            .unwrap();
        let _: Value = serde_json::from_str(&response).unwrap();
        println!("{service}/{endpoint}: valid response");
    }
    for page in [
        "Home",
        "Liked Songs",
        "Albums",
        "Artists",
        "Playlists",
        "All Albums",
        "All Artists",
        "Songs",
        "Genres",
        "Recently Added",
        "Recently Played",
        "Downloads",
        "Queue",
    ] {
        let text = core.browse(page.into(), "".into()).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        println!(
            "catalog/{page}: {} rows",
            value["rows"].as_array().unwrap().len()
        );
        if page == "All Albums" {
            if let Some(id) = value["rows"]
                .as_array()
                .and_then(|r| r.first())
                .and_then(|r| r["id"].as_str())
            {
                let album: Value = serde_json::from_str(
                    &core.open_collection("albums".into(), id.into()).unwrap(),
                )
                .unwrap();
                assert_eq!(album["kind"], "songs");
                println!("catalog/album: ordered track response verified");
                if let Some(song) = album["rows"].as_array().and_then(|rows| rows.first()) {
                    core.download(song.to_string()).unwrap();
                    assert!(core
                        .media_url(song.to_string(), false)
                        .unwrap()
                        .starts_with("file:"));
                    let stats: Value =
                        serde_json::from_str(&core.storage_stats().unwrap()).unwrap();
                    println!(
                        "offline/download: {} bytes stored",
                        stats["readyBytes"].as_u64().unwrap()
                    );
                }
            }
        }
    }
    drop(core);
    std::fs::remove_dir_all(directory).unwrap();
}

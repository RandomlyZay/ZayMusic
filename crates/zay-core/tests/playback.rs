mod common;
use common::{ids, parse, rest, Root, Server};
use serde_json::{json, Value};

fn songs() -> Value {
    json!([{"id":"a","title":"A"},{"id":"b","title":"B"},{"id":"c","title":"C"}])
}

#[test]
fn saved_position_survives_restart_rejects_stale_reports_and_clears_on_next() {
    let root = Root::new("position");
    let core = root.core("http://127.0.0.1:9");
    let state = parse(&core.play_rows(songs().to_string(), 1, "{}".into()).unwrap());
    let epoch = state["playbackEpoch"].as_u64().unwrap();
    core.save_playback_position(epoch, "b".into(), 42.25)
        .unwrap();
    core.save_playback_position(epoch + 1, "b".into(), 99.)
        .unwrap();
    core.save_playback_position(epoch, "a".into(), 99.).unwrap();
    core.save_playback_position(epoch, "b".into(), f64::NAN)
        .unwrap();
    drop(core);
    let core = root.core("http://127.0.0.1:9");
    let state = parse(&core.queue_command("{}".into()).unwrap());
    assert_eq!(state["current"]["id"], "b");
    assert_eq!(state["resumePosition"], 42.25);
    let next = parse(
        &core
            .queue_command(json!({"op":"next"}).to_string())
            .unwrap(),
    );
    assert_eq!(next["current"]["id"], "c");
    assert_eq!(next["resumePosition"], 0.);
    core.save_playback_position(epoch, "b".into(), 15.).unwrap();
    assert_eq!(parse(&core.playback_position().unwrap()), Value::Null);
}

#[test]
fn previous_restarts_after_three_seconds_and_goes_back_at_the_boundary() {
    let root = Root::new("previous");
    let core = root.core("http://127.0.0.1:9");
    let start = parse(&core.play_rows(songs().to_string(), 1, "{}".into()).unwrap());
    core.save_playback_position(start["playbackEpoch"].as_u64().unwrap(), "b".into(), 20.)
        .unwrap();
    let restart = parse(
        &core
            .queue_command(json!({"op":"previous","position":3.01}).to_string())
            .unwrap(),
    );
    assert_eq!(restart["restart"], true);
    assert_eq!(restart["current"]["id"], "b");
    assert_eq!(restart["playbackEpoch"], start["playbackEpoch"]);
    assert_eq!(restart["resumePosition"], 0.);
    let previous = parse(
        &core
            .queue_command(json!({"op":"previous","position":3.0}).to_string())
            .unwrap(),
    );
    assert_eq!(previous["restart"], false);
    assert_eq!(previous["current"]["id"], "a");
}

#[test]
fn collection_play_and_track_taps_share_the_web_shuffle_rules() {
    let root = Root::new("play-policy");
    let core = root.core("http://127.0.0.1:9");
    for context in [
        json!({"kind":"liked","id":"liked"}),
        json!({"kind":"playlists","id":"p"}),
    ] {
        let state = parse(
            &core
                .play_collection(songs().to_string(), context.to_string())
                .unwrap(),
        );
        assert_eq!(state["state"]["shuffle"], true);
        assert_eq!(state["state"]["queue"].as_array().unwrap().len(), 3);
    }
    let tap = parse(&core.play_rows(songs().to_string(), 1, "{}".into()).unwrap());
    assert_eq!(tap["state"]["shuffle"], true);
    assert_eq!(tap["current"]["id"], "b");
    for kind in ["albums", "newAlbums"] {
        let album = parse(
            &core
                .play_collection(
                    songs().to_string(),
                    json!({"kind":kind,"id":"album"}).to_string(),
                )
                .unwrap(),
        );
        assert_eq!(album["state"]["shuffle"], false);
        assert_eq!(album["current"]["id"], "a");
    }
}

#[test]
fn library_filters_sort_and_text_use_the_same_shared_rows() {
    let server = Server::new(|r| match r.path.as_str() {
        "/rest/getStarred2" => Some(rest(
            json!({"starred2":{"song":[{"id":"s"}],"album":[{"id":"al","name":"Zulu","starred":"2026-01-01T00:00:00Z"}],"artist":[{"id":"ar","name":"Artist","starred":"2026-02-01T00:00:00Z"}]}}),
        )),
        "/rest/getPlaylists" => Some(rest(
            json!({"playlists":{"playlist":[{"id":"p","name":"Playlist","created":"2026-03-01T00:00:00Z"}]}}),
        )),
        _ => None,
    });
    let root = Root::new("library-policy");
    let core = root.core(&server.url);
    let added = parse(
        &core
            .browse("Library".into(), json!({"sort":"added"}).to_string())
            .unwrap(),
    );
    assert_eq!(ids(&added["rows"]), ["liked", "p", "ar", "al"]);
    core.record_context(json!({"kind":"albums","id":"al","name":"Zulu"}).to_string())
        .unwrap();
    let recent = parse(&core.browse("Library".into(), "{}".into()).unwrap());
    assert_eq!(ids(&recent["rows"]), ["liked", "al", "p", "ar"]);
    let filtered = parse(
        &core
            .browse(
                "Library".into(),
                json!({"filter":"albums","text":"zul"}).to_string(),
            )
            .unwrap(),
    );
    assert_eq!(ids(&filtered["rows"]), ["al"]);
}

#[test]
fn tag_edits_preserve_explicit_clears_and_update_queue_and_offline_pages() {
    let server = Server::new(|r| match r.path.as_str() {
        "/api/fetch/music/a/tags" => Some(common::fetch(json!({"ok":true}))),
        _ => None,
    });
    let root = Root::new("tag-cache");
    let core = root.core(&server.url);
    core.play_rows(songs().to_string(), 0, "{}".into()).unwrap();
    let key = format!("browse:{}", json!(["Liked Songs", ""]));
    core.setting(
        key.clone(),
        Some(
            json!({"title":"Liked Songs","rows":[{"id":"a","title":"Old","track":3,"year":2020}]})
                .to_string(),
        ),
    )
    .unwrap();
    core.library_command(
        json!({"op":"editTags","item":{"id":"a"},"edits":{"title":"New","track":null,"year":null}})
            .to_string(),
    )
    .unwrap();
    assert_eq!(
        server.take()[0].json(),
        json!({"title":"New","track":null,"year":null})
    );
    let queue = parse(&core.queue_command("{}".into()).unwrap());
    assert_eq!(queue["current"]["title"], "New");
    assert_eq!(queue["current"]["track"], Value::Null);
    let cached = parse(&core.setting(key, None).unwrap());
    assert_eq!(cached["rows"][0]["title"], "New");
    assert_eq!(cached["rows"][0]["year"], Value::Null);
}

#[test]
fn native_offline_downloads_require_library_songs_and_skip_mixed_preview_rows() {
    let root = Root::new("preview-download");
    let core = root.core("http://127.0.0.1:9");
    assert!(core
        .queue_download(json!({"id":"yt:video","title":"Preview"}).to_string())
        .is_err());
    let outcome = parse(
        &core
            .download_many(
                json!([
                    {"id":"album","browseKind":"albums"},
                    {"id":"yt:video","previewUrl":"https://example.invalid/audio"}
                ])
                .to_string(),
            )
            .unwrap(),
    );
    assert_eq!(
        outcome,
        json!({"completed":0,"failed":0,"cancelled":0,"pending":0})
    );
    assert_eq!(parse(&core.downloads().unwrap()), json!([]));
}

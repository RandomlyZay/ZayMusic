mod common;
use common::{fetch, ids, parse, rest, Reply, Root, Server};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

fn queue(core: &zay_core::NativeCore, command: Value) -> Value {
    parse(&core.queue_command(command.to_string()).unwrap())
}
fn songs(names: &[&str]) -> Value {
    json!(names
        .iter()
        .map(|n| json!({"id":n,"title":n.to_uppercase()}))
        .collect::<Vec<_>>())
}
fn queue_ids(state: &Value) -> Vec<String> {
    ids(&state["state"]["queue"])
}
/// Main-lane rows of the shared Queue page, in the order they will play.
fn upcoming(core: &zay_core::NativeCore, lane: &str) -> Vec<(String, u64)> {
    parse(&core.browse("Queue".into(), "".into()).unwrap())["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["queueLane"] == lane)
        .map(|r| {
            (
                r["id"].as_str().unwrap().to_owned(),
                r["queueIndex"].as_u64().unwrap(),
            )
        })
        .collect()
}

#[test]
fn playing_a_mixed_row_queues_only_songs_and_maps_the_selected_index() {
    let root = Root::new("playable");
    let core = root.core("http://127.0.0.1:9");
    let rows = json!([
        {"id":"al","title":"Album","browseKind":"albums"},
        {"id":"s1","title":"One"},
        {"id":"ar","title":"Artist","browseKind":"artists"},
        {"id":"yt:v","title":"Preview","previewUrl":"/api/fetch/preview/v"},
        {"id":"s2","title":"Two","browseKind":"songs"},
        {"id":"na","title":"New album","browseKind":"newAlbums"}
    ]);
    let state = queue(&core, json!({"op":"set","songs":rows,"index":4}));
    assert_eq!(queue_ids(&state), ["s1", "yt:v", "s2"]);
    assert_eq!(state["current"]["id"], "s2");
    let state = queue(&core, json!({"op":"set","songs":rows,"index":3}));
    assert_eq!(state["current"]["id"], "yt:v");
    let state = queue(&core, json!({"op":"playNext","songs":rows}));
    assert_eq!(
        ids(&state["state"]["secondaryQueue"]),
        ["s1", "yt:v", "s2"],
        "Collection rows never enter the priority queue"
    );
    queue(&core, json!({"op":"set","songs":[{"id":"x"}]}));
    let state = queue(&core, json!({"op":"append","songs":rows}));
    assert_eq!(queue_ids(&state), ["x", "s1", "yt:v", "s2"]);
}
#[test]
fn queue_page_lists_priority_then_upcoming_main_songs_only() {
    let root = Root::new("upcoming");
    let core = root.core("http://127.0.0.1:9");
    queue(
        &core,
        json!({"op":"set","songs":songs(&["a","b","c","d","e"]),"index":1}),
    );
    queue(&core, json!({"op":"playNext","songs":songs(&["p"])}));
    assert_eq!(upcoming(&core, "priority"), [("p".into(), 0)]);
    assert_eq!(
        upcoming(&core, "main"),
        [("c".into(), 2), ("d".into(), 3), ("e".into(), 4)],
        "History and the current song are not upcoming"
    );
    let rows = parse(&core.browse("Queue".into(), "".into()).unwrap());
    assert_eq!(
        ids(&rows["rows"]),
        ["p", "c", "d", "e"],
        "Priority plays before the main queue"
    );
}
#[test]
fn moving_upcoming_songs_preserves_the_current_song() {
    let root = Root::new("move-upcoming");
    let core = root.core("http://127.0.0.1:9");
    let before = queue(
        &core,
        json!({"op":"set","songs":songs(&["a","b","c","d","e"]),"index":1}),
    );
    let epoch = before["playbackEpoch"].clone();
    let state = queue(&core, json!({"op":"moveUpcoming","from":3,"direction":-1}));
    assert_eq!(queue_ids(&state), ["a", "b", "d", "c", "e"]);
    assert_eq!(state["current"]["id"], "b");
    assert_eq!(state["state"]["index"], 1);
    assert_eq!(
        state["playbackEpoch"], epoch,
        "Reordering upcoming songs must not restart playback"
    );
    // The first upcoming song cannot jump over the current one, and the last cannot move down.
    for (from, direction) in [(2, -1), (4, 1)] {
        let state = queue(
            &core,
            json!({"op":"moveUpcoming","from":from,"direction":direction}),
        );
        assert_eq!(queue_ids(&state), ["a", "b", "d", "c", "e"]);
        assert_eq!(state["current"]["id"], "b");
    }
}
#[test]
fn moving_upcoming_songs_in_shuffle_reorders_play_order_not_the_list() {
    let root = Root::new("move-shuffle");
    let core = root.core("http://127.0.0.1:9");
    queue(
        &core,
        json!({"op":"set","songs":songs(&["a","b","c","d","e","f"]),"index":0}),
    );
    let shuffled = queue(&core, json!({"op":"shuffle"}));
    let current = shuffled["current"]["id"].clone();
    let list = queue_ids(&shuffled);
    let order = upcoming(&core, "main");
    assert_eq!(order.len(), 5, "Every other song is still upcoming");
    assert!(order.iter().all(|(id, _)| *id != current));
    let (moved, from) = order[2].clone();
    let state = queue(
        &core,
        json!({"op":"moveUpcoming","from":from,"direction":-1}),
    );
    assert_eq!(state["current"], shuffled["current"]);
    assert_eq!(
        queue_ids(&state),
        list,
        "Shuffle edits the play order, not the library order"
    );
    let after = upcoming(&core, "main");
    let mut expected = order.clone();
    expected.swap(1, 2);
    assert_eq!(after, expected);
    assert_eq!(after[1].0, moved);
    let (first, from) = after[0].clone();
    queue(
        &core,
        json!({"op":"moveUpcoming","from":from,"direction":-1}),
    );
    assert_eq!(
        upcoming(&core, "main")[0].0,
        first,
        "Nothing moves ahead of the current song"
    );
    let next = queue(&core, json!({"op":"next"}));
    assert_eq!(next["current"]["id"], first.as_str());
    assert_eq!(
        upcoming(&core, "main")[0].0,
        moved,
        "Playback follows the edited order"
    );
}
#[test]
fn recent_collections_are_most_recent_first_and_forget_deleted_playlists() {
    let server = Server::new(|r| match r.path.as_str() {
        "/rest/deletePlaylist" => Some(rest(json!({}))),
        "/api/fetch/youtube/imports" => Some(fetch(json!({"imports":[]}))),
        "/api/fetch/home" => Some(fetch(json!({"topSongs":[],"rediscover":[]}))),
        _ => None,
    });
    let root = Root::new("contexts");
    let core = root.core(&server.url);
    for (kind, id) in [
        ("album", "al"),
        ("playlists", "pl"),
        ("liked", "liked"),
        ("albums", "al2"),
    ] {
        core.record_context(json!({"type":kind,"id":id,"name":id}).to_string())
            .unwrap();
    }
    core.record_context(json!({"kind":"albums","id":"al","name":"Replayed"}).to_string())
        .unwrap();
    let contexts = parse(&core.recent_contexts().unwrap());
    assert_eq!(ids(&contexts), ["al", "al2", "liked", "pl"]);
    assert_eq!(
        contexts[0]["kind"], "albums",
        "Singular kinds are normalized"
    );
    assert_eq!(contexts[0]["name"], "Replayed");
    assert!(core
        .record_context(json!({"kind":"albums","id":""}).to_string())
        .is_err());
    core.playlist_command(json!({"op":"delete","id":"pl"}).to_string())
        .unwrap();
    let contexts = parse(&core.recent_contexts().unwrap());
    assert_eq!(ids(&contexts), ["al", "al2", "liked"]);
    let page = parse(&core.browse("Recent Collections".into(), "".into()).unwrap());
    assert_eq!(page["kind"], "contexts");
    assert_eq!(ids(&page["rows"]), ["al", "al2", "liked"]);
    let home = parse(&core.browse("Home".into(), "".into()).unwrap());
    assert_eq!(ids(&home["extra"]["contexts"]), ["al", "al2", "liked"]);
}

fn album_server(jobs_ok: Arc<AtomicBool>) -> Server {
    Server::new(move |r| match r.route().as_str() {
        "POST /api/fetch/album-saves" => Some(fetch(json!({"albumId":null}))),
        "GET /api/fetch/album" => Some(fetch(json!({
            "album":{"browseId":"MPREx","title":"Fixture Album","artist":"Fixture Artist"},
            "candidates":[
                {"videoId":"track1","title":"One","channel":"Fixture Artist","track":1},
                {"videoId":"track2","title":"Two","channel":"Fixture Artist"},
                {"videoId":"track3","title":"Three","channel":"Fixture Artist","track":3}]}))),
        "POST /api/fetch/jobs" if jobs_ok.load(Ordering::SeqCst) => {
            Some(fetch(json!({"job":{"id":"fake-job"}})))
        }
        "POST /api/fetch/jobs" => Some(Reply::Status(503)),
        "POST /rest/updatePlaylist" => Some(rest(json!({}))),
        _ => None,
    })
}
fn jobs(server: &Server) -> Vec<Value> {
    server
        .take()
        .into_iter()
        .filter(|r| r.route() == "POST /api/fetch/jobs")
        .map(|r| r.json())
        .collect()
}
#[test]
fn acquiring_an_album_queues_tracks_in_order_before_saving_it() {
    let server = album_server(Arc::new(AtomicBool::new(true)));
    let root = Root::new("acquire-album");
    let core = root.core(&server.url);
    core.setting("browse:fake".into(), Some("cached".into()))
        .unwrap();
    let result = parse(
        &core
            .library_command(
                json!({"op":"acquireAlbum","kind":"newAlbums","item":{"browseId":"MPREx","title":"Fixture Album","artist":"Fixture Artist","browseKind":"newAlbums"}})
                    .to_string(),
            )
            .unwrap(),
    );
    assert_eq!(result, json!({"queued":3,"failed":0}));
    let requests = server.take();
    assert_eq!(requests[4].route(), "POST /api/fetch/album-saves");
    assert_eq!(
        requests[4].json(),
        json!({"title":"Fixture Album","artist":"Fixture Artist"})
    );
    assert_eq!(requests[0].route(), "GET /api/fetch/album");
    assert_eq!(requests[0].param("browseId"), Some("MPREx"));
    let jobs: Vec<Value> = requests[1..4].iter().map(|r| r.json()).collect();
    assert_eq!(
        jobs.iter()
            .map(|j| j["videoId"].clone())
            .collect::<Vec<_>>(),
        ["track1", "track2", "track3"]
    );
    assert_eq!(
        jobs.iter()
            .map(|j| j["overrides"]["track"].clone())
            .collect::<Vec<_>>(),
        [1, 2, 3],
        "Missing positions follow album order"
    );
    assert!(jobs
        .iter()
        .all(|j| j["overrides"]["album"] == "Fixture Album"));
    assert_eq!(core.setting("browse:fake".into(), None).unwrap(), "null");
}
#[test]
fn acquiring_an_album_into_a_playlist_targets_every_track() {
    let server = album_server(Arc::new(AtomicBool::new(true)));
    let root = Root::new("acquire-album-playlist");
    let core = root.core(&server.url);
    core.library_command(
        json!({"op":"acquireAlbum","kind":"newAlbums","playlistId":"pl","item":{"browseId":"MPREx","title":"Fixture Album","artist":"Fixture Artist"}})
            .to_string(),
    )
    .unwrap();
    let jobs = jobs(&server);
    assert_eq!(jobs.len(), 3);
    assert!(
        jobs.iter().all(|j| j["playlistIds"] == json!(["pl"])),
        "{jobs:?}"
    );
}
#[test]
fn failed_song_acquisition_retries_the_identical_request() {
    let available = Arc::new(AtomicBool::new(false));
    let server = album_server(available.clone());
    let root = Root::new("acquire-retry");
    let core = root.core(&server.url);
    let song =
        json!({"id":"yt:video1","title":"Preview","artist":"Artist","album":"Album","track":4});
    assert!(core.acquire_song(song.to_string()).is_err());
    let first = jobs(&server);
    let failures = parse(&core.acquisition_failures().unwrap());
    assert_eq!(failures.as_array().unwrap().len(), 1);
    available.store(true, Ordering::SeqCst);
    core.management_command(json!({"op":"retryJob","id":failures[0]["id"]}).to_string())
        .unwrap();
    assert_eq!(jobs(&server), first, "Retry re-sends the original job");
    assert_eq!(core.acquisition_failures().unwrap(), "[]");
}
#[test]
fn failed_playlist_targeted_save_retries_into_the_same_playlist() {
    let available = Arc::new(AtomicBool::new(false));
    let server = album_server(available.clone());
    let root = Root::new("playlist-retry");
    let core = root.core(&server.url);
    let _ = core.playlist_command(
        json!({"op":"add","id":"pl","songs":[{"id":"yt:video1","title":"Preview","artist":"Artist","album":"Album","track":2}]})
            .to_string(),
    );
    let attempted = jobs(&server);
    assert_eq!(attempted.len(), 1);
    assert_eq!(attempted[0]["playlistIds"], json!(["pl"]));
    let failures = parse(&core.acquisition_failures().unwrap());
    assert_eq!(
        failures.as_array().map(Vec::len),
        Some(1),
        "A failed playlist save is kept for retry"
    );
    let page = parse(&core.browse("Jobs".into(), "".into()).unwrap());
    assert_eq!(page["rows"][0]["name"], "Preview");
    assert_eq!(page["rows"][0]["status"], "failed");
    available.store(true, Ordering::SeqCst);
    core.management_command(json!({"op":"retryJob","id":failures[0]["id"]}).to_string())
        .unwrap();
    let retried = jobs(&server);
    assert_eq!(retried.len(), 1);
    assert_eq!(retried[0]["videoId"], "video1");
    assert_eq!(
        retried[0]["playlistIds"],
        json!(["pl"]),
        "Retry must keep the playlist target"
    );
    assert_eq!(retried[0]["overrides"]["title"], "Preview");
    assert_eq!(core.acquisition_failures().unwrap(), "[]");
}
#[test]
fn dismissing_a_local_failure_forgets_it_without_contacting_the_server() {
    let root = Root::new("dismiss");
    let core = root.core("http://127.0.0.1:9");
    assert!(core
        .acquire_song(json!({"id":"yt:video1","title":"Preview"}).to_string())
        .is_err());
    let failures = parse(&core.acquisition_failures().unwrap());
    core.management_command(json!({"op":"deleteJob","id":failures[0]["id"]}).to_string())
        .unwrap();
    assert_eq!(core.acquisition_failures().unwrap(), "[]");
}

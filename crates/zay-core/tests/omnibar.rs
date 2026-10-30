mod common;
use common::{fetch, ids, parse, rest, sections, Root, Server};
use serde_json::{json, Value};
use zay_core::search::rank_library;

fn titles(rows: &[Value]) -> Vec<&str> {
    rows.iter()
        .map(|r| r["title"].as_str().or(r["name"].as_str()).unwrap())
        .collect()
}
// Mirrors apps/web/src/lib/api/ranking.test.ts.
#[test]
fn ranking_puts_exact_artist_title_matches_ahead_of_loose_titles() {
    let rows = vec![
        json!({"title":"Dreams (cover)","artist":"Someone else"}),
        json!({"title":"Dreams","artist":"Fleetwood Mac"}),
        json!({"title":"Fleetwood Mac Dreams","artist":"Tribute band"}),
    ];
    let mut all = rows.clone();
    rank_library(&mut all, "Fleetwood Mac Dreams");
    assert_eq!(all[0], rows[1]);
    let mut two = rows[..2].to_vec();
    rank_library(&mut two, "Fleetwood Mac Dreams");
    assert_eq!(two[0], rows[1]);
}
#[test]
fn ranking_ignores_accents_and_case() {
    let mut rows = vec![json!({"name":"Other"}), json!({"name":"Beyoncé"})];
    rank_library(&mut rows, "beyonce");
    assert_eq!(titles(&rows), ["Beyoncé", "Other"]);
    let mut rows = vec![json!({"name":"Other"}), json!({"name":"Beyonce"})];
    rank_library(&mut rows, "BEYONCÉ");
    assert_eq!(titles(&rows)[0], "Beyonce");
}
#[test]
fn ranking_is_stable_for_equally_relevant_rows() {
    let mut rows = vec![
        json!({"title":"Song B","artist":"X"}),
        json!({"title":"Song A","artist":"X"}),
    ];
    rank_library(&mut rows, "song");
    assert_eq!(titles(&rows), ["Song B", "Song A"]);
}

fn discovery_server() -> Server {
    Server::new(|r| match r.path.as_str() {
        "/rest/search3" => Some(rest(json!({"searchResult3":{
            "song":[{"id":"loose","title":"Dreams (cover)","artist":"Someone else"},{"id":"exact","title":"Dreams","artist":"Fleetwood Mac"}],
            "album":[{"id":"al","name":"Rumours","artist":"Fleetwood Mac","coverArt":"al-al"}],
            "artist":[{"id":"ar","name":"Fleetwood Mac"}]}}))),
        "/api/fetch/search" => Some(fetch(json!({
            "candidates":[{"videoId":"newvid1","title":"Dreams (live)","channel":"Fleetwood Mac","durationSec":200.0,"thumbnails":[{"url":"https://img.invalid/c"}]}],
            "albums":[{"browseId":"MPREx","title":"Rumours (Deluxe)","artist":"Fleetwood Mac","thumbnails":[{"url":"https://img.invalid/s"},{"url":"https://img.invalid/l"}]}],
            "correction":{"artist":"Fleetwood Mac","title":"Dreams"},
            "suggestions":["fleetwood mac dreams live"]}))),
        "/api/fetch/home" => Some(fetch(
            json!({"topSongs":[{"id":"top","title":"Top"}],"rediscover":[]}),
        )),
        _ => None,
    })
}
#[test]
fn omnibar_mixes_ranked_library_collections_and_discovery_sections() {
    let server = discovery_server();
    let root = Root::new("omnibar");
    let core = root.core(&server.url);
    let page = parse(
        &core
            .browse("Omnibar".into(), "Fleetwood Mac Dreams".into())
            .unwrap(),
    );
    assert_eq!(
        ids(&page["rows"]),
        ["exact", "loose", "al", "ar", "yt:newvid1", "MPREx"]
    );
    assert_eq!(
        sections(&page),
        [
            ("In your library".into(), 0, 2),
            ("Albums".into(), 2, 1),
            ("Artists".into(), 3, 1),
            ("New music".into(), 4, 1),
            ("New albums".into(), 5, 1),
        ]
    );
    let rows = page["rows"].as_array().unwrap();
    assert!(rows[0]["browseKind"].is_null() && rows[4]["browseKind"].is_null());
    assert_eq!(rows[2]["browseKind"], "albums");
    assert_eq!(
        rows[2]["title"], "Rumours",
        "Collections expose a display title"
    );
    assert_eq!(rows[3]["browseKind"], "artists");
    assert_eq!(rows[5]["browseKind"], "newAlbums");
    assert_eq!(rows[5]["artworkUrl"], "https://img.invalid/l");
    assert!(rows[4]["previewUrl"]
        .as_str()
        .unwrap()
        .starts_with("/api/fetch/preview/newvid1?"));
    assert_eq!(
        page["extra"]["correction"],
        json!({"artist":"Fleetwood Mac","title":"Dreams"})
    );
    assert_eq!(
        page["extra"]["suggestions"],
        json!(["fleetwood mac dreams live"])
    );
    let search = server
        .take()
        .into_iter()
        .find(|r| r.path == "/rest/search3")
        .unwrap();
    assert_eq!(search.param("query"), Some("Fleetwood Mac Dreams"));
}
#[test]
fn library_pages_never_call_discovery_and_new_music_never_calls_the_library() {
    let server = discovery_server();
    let root = Root::new("omnibar-pages");
    let core = root.core(&server.url);
    for page in ["Search", "Songs"] {
        let result = parse(&core.browse(page.into(), "Dreams".into()).unwrap());
        assert!(
            ids(&result["rows"]).iter().all(|id| !id.starts_with("yt:")),
            "{page} stays in the library"
        );
    }
    assert!(server
        .routes()
        .iter()
        .all(|r| !r.starts_with("GET /api/fetch/search")));
    server.take();
    let result = parse(&core.browse("New music".into(), "Dreams".into()).unwrap());
    assert_eq!(ids(&result["rows"]), ["yt:newvid1", "MPREx"]);
    assert!(server.routes().iter().all(|r| !r.contains("search3")));
}
#[test]
fn omnibar_keeps_discovery_when_the_library_search_fails() {
    let server = Server::new(|r| match r.path.as_str() {
        "/api/fetch/search" => Some(fetch(
            json!({"candidates":[{"videoId":"newvid1","title":"Dreams (live)","channel":"Artist"}],"albums":[]}),
        )),
        _ => Some(common::Reply::Status(503)),
    });
    let root = Root::new("omnibar-partial");
    let core = root.core(&server.url);
    let page = parse(&core.browse("Omnibar".into(), "Dreams".into()).unwrap());
    assert_eq!(ids(&page["rows"]), ["yt:newvid1"]);
    assert_eq!(sections(&page), [("New music".into(), 0, 1)]);
    assert_eq!(page["offline"], false);
}
#[test]
fn empty_queries_show_recent_songs_except_on_the_songs_page() {
    let server = discovery_server();
    let root = Root::new("omnibar-empty");
    let core = root.core(&server.url);
    root.played(&[("older", 10), ("newer", 20)]);
    for page in ["Omnibar", "Search"] {
        let result = parse(&core.browse(page.into(), "".into()).unwrap());
        assert_eq!(
            ids(&result["rows"]),
            ["newer", "older"],
            "{page} opens on recent songs"
        );
    }
    let songs = parse(&core.browse("Songs".into(), "".into()).unwrap());
    assert_eq!(
        ids(&songs["rows"]),
        ["loose", "exact"],
        "Songs lists the library"
    );
    assert!(server
        .routes()
        .iter()
        .all(|r| !r.starts_with("GET /api/fetch/search")));
}
#[test]
fn liked_songs_are_newest_first_with_undated_saves_on_top() {
    // Same order as apps/web/src/routes/library/liked/+page.svelte.
    let server = Server::new(|r| {
        (r.path == "/rest/getStarred2").then(|| {
            rest(json!({"starred2":{"song":[
                {"id":"a","title":"A","starred":"2026-01-01T00:00:00Z"},
                {"id":"b","title":"B","starred":"2026-03-01T00:00:00Z"},
                {"id":"c","title":"C"},
                {"id":"d","title":"D","starred":"2026-02-01T00:00:00Z"}]}}))
        })
    });
    let root = Root::new("liked");
    let core = root.core(&server.url);
    let page = parse(&core.browse("Liked Songs".into(), "".into()).unwrap());
    assert_eq!(ids(&page["rows"]), ["c", "b", "d", "a"]);
}
#[test]
fn offline_home_shows_current_listening_history_with_cached_server_shelves() {
    let server = discovery_server();
    let root = Root::new("home-offline");
    let core = root.core(&server.url);
    root.played(&[("first", 10)]);
    let online = parse(&core.browse("Home".into(), "".into()).unwrap());
    assert_eq!(ids(&online["rows"]), ["first", "top"]);
    drop(core);
    drop(server);
    let core = root.core("http://127.0.0.1:9");
    root.played(&[("second", 20)]);
    let offline = parse(&core.browse("Home".into(), "".into()).unwrap());
    assert_eq!(offline["offline"], true);
    assert_eq!(
        ids(&offline["rows"]),
        ["second", "first", "top"],
        "Recently played must not be the stale cached copy"
    );
    assert_eq!(
        sections(&offline),
        [
            ("Recently played".into(), 0, 2),
            ("Your top songs".into(), 2, 1)
        ]
    );
}

#[test]
fn large_search_pages_each_type_to_exhaustion_without_a_fixed_song_cap() {
    let server = Server::new(|r| {
        if r.path != "/rest/search3" {
            return None;
        }
        let offset = r
            .param("songOffset")
            .unwrap_or("0")
            .parse::<usize>()
            .unwrap();
        let count = r
            .param("songCount")
            .unwrap_or("0")
            .parse::<usize>()
            .unwrap();
        let songs: Vec<_> = (offset..(offset + count).min(535))
            .map(|i| json!({"id":format!("s{i}"),"title":format!("Song {i}")}))
            .collect();
        Some(rest(json!({"searchResult3":{"song":songs}})))
    });
    let root = Root::new("search-paging");
    let core = root.core(&server.url);
    let page = parse(&core.browse("Songs".into(), "song".into()).unwrap());
    assert_eq!(page["rows"].as_array().unwrap().len(), 535);
    assert!(ids(&page["rows"]).contains(&"s534".into()));
    let requests = server.take();
    assert_eq!(requests.len(), 14);
    assert_eq!(requests[1].param("songOffset"), Some("40"));
    assert_eq!(requests[1].param("albumCount"), Some("0"));
}

#[test]
fn a_server_ignoring_offsets_cannot_create_duplicates_or_an_endless_search() {
    let server = Server::new(|r| {
        (r.path=="/rest/search3").then(|| rest(json!({"searchResult3":{"song":(0..40).map(|i|json!({"id":format!("s{i}")})).collect::<Vec<_>>()}})))
    });
    let root = Root::new("search-repeat");
    let core = root.core(&server.url);
    let page = parse(&core.browse("Songs".into(), String::new()).unwrap());
    assert_eq!(page["rows"].as_array().unwrap().len(), 40);
    assert_eq!(server.take().len(), 2);
}

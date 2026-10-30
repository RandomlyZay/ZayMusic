use zay_core::{
    model::{fresh_radio, sort_album, ReplayGain, Song},
    queue::{weave, Queue, Repeat},
    NativeCore,
};
fn songs(n: usize) -> Vec<Song> {
    (0..n)
        .map(|i| Song {
            id: format!("s{i}"),
            title: format!("Song {i}"),
            duration: 100.,
            ..Default::default()
        })
        .collect()
}
fn state(n: usize, start: usize) -> Queue {
    let mut q = Queue::default();
    q.set(songs(n), start);
    q
}
#[test]
fn thousand_shuffles_are_permutations() {
    for _ in 0..1000 {
        let mut q = state(50, 0);
        q.toggle_shuffle();
        let mut seen = std::collections::HashSet::new();
        seen.insert(q.current().unwrap().id.clone());
        for _ in 0..49 {
            assert!(q.advance());
            assert!(seen.insert(q.current().unwrap().id.clone()));
        }
        assert!(!q.advance());
        assert_eq!(seen.len(), 50);
    }
}
#[test]
fn repeat_off_stops() {
    assert!(!state(3, 2).advance());
}
#[test]
fn repeat_all_wraps() {
    let mut q = state(3, 2);
    q.repeat = Repeat::All;
    assert!(q.advance());
    assert_eq!(q.index, 0);
}
#[test]
fn repeat_one_holds() {
    let mut q = state(5, 2);
    q.repeat = Repeat::One;
    assert!(q.advance());
    assert_eq!(q.index, 2);
}
#[test]
fn previous_uses_history() {
    let mut q = state(5, 0);
    q.advance();
    q.advance();
    q.previous();
    assert_eq!(q.index, 1);
    assert_eq!(q.history.len(), 1);
}
#[test]
fn priority_resumes_main_in_both_modes() {
    for shuffle in [false, true] {
        let tracks = songs(5);
        let mut q = state(5, 0);
        if shuffle {
            q.shuffle = true;
            q.shuffle_order = vec![0, 3, 1, 4, 2];
            q.shuffle_pos = 0;
        }
        q.priority(vec![tracks[2].clone()], false);
        q.priority(vec![tracks[4].clone()], false);
        q.priority(vec![tracks[1].clone()], true);
        for id in ["s4", "s2", "s1", "s3"] {
            assert!(q.advance());
            assert_eq!(q.current().unwrap().id, id);
        }
        assert!(!q.advance());
    }
}
#[test]
fn priority_deduplicates_and_repositions() {
    let t = songs(4);
    let mut q = state(4, 0);
    q.priority(vec![t[1].clone(), t[2].clone()], false);
    q.priority(vec![t[2].clone()], false);
    assert_eq!(q.secondary_queue[0].id, "s2");
    q.priority(vec![t[2].clone(), t[2].clone()], true);
    assert_eq!(q.secondary_queue.len(), 2);
    assert_eq!(q.secondary_queue[1].id, "s2");
}
#[test]
fn append_moves_out_of_priority() {
    let t = songs(3);
    let mut q = state(3, 0);
    q.priority(vec![t[1].clone()], false);
    q.append(vec![t[1].clone()]);
    assert!(q.secondary_queue.is_empty());
    assert_eq!(q.queue[2].id, "s1");
}
#[test]
fn moving_current_preserves_audio() {
    let mut q = state(3, 0);
    q.priority(vec![songs(3)[0].clone()], false);
    assert_eq!(q.current().unwrap().id, "s0");
    q.advance();
    assert_eq!(q.current().unwrap().id, "s0");
    q.advance();
    assert_eq!(q.current().unwrap().id, "s1");
}
#[test]
fn jump_priority_preserves_main() {
    let t = songs(4);
    let mut q = state(4, 0);
    q.priority(vec![t[1].clone(), t[3].clone()], false);
    q.jump_secondary(1);
    assert_eq!(q.current().unwrap().id, "s3");
    q.advance();
    assert_eq!(q.current().unwrap().id, "s1");
    q.advance();
    assert_eq!(q.current().unwrap().id, "s2");
}
#[test]
fn previous_through_priority_then_forward() {
    let t = songs(4);
    let mut q = state(4, 0);
    q.priority(vec![t[1].clone(), t[2].clone()], false);
    q.advance();
    q.advance();
    q.previous();
    assert_eq!(q.current().unwrap().id, "s1");
    q.advance();
    assert_eq!(q.current().unwrap().id, "s2");
    q.advance();
    assert_eq!(q.current().unwrap().id, "s3");
}
#[test]
fn remove_adjusts_cursor() {
    let mut q = state(5, 3);
    q.remove(1);
    assert_eq!(q.index, 2);
    q.remove(2);
    assert_eq!(q.queue.len(), 3);
}
#[test]
fn move_remaps_shuffle() {
    let mut q = state(5, 0);
    q.toggle_shuffle();
    q.move_song(0, 3);
    assert_eq!(q.index, 3);
    assert_eq!(q.queue[3].id, "s0");
    assert_eq!(q.shuffle_order[0], 3);
}
#[test]
fn empty_queue_is_safe() {
    let mut q = Queue::default();
    assert!(!q.advance());
    q.remove(7);
    q.move_song(1, 8);
    assert!(q.current().is_none());
}
#[test]
fn weave_preserves_library_once() {
    for dense in [true, false] {
        for _ in 0..100 {
            let fresh = (0..12)
                .map(|i| Song {
                    id: format!("f{i}"),
                    ..Default::default()
                })
                .collect();
            let out = weave(songs(60), fresh, dense);
            assert!(out[0].id.starts_with('s'));
            let mine: std::collections::HashSet<_> = out
                .iter()
                .filter(|s| s.id.starts_with('s'))
                .map(|s| &s.id)
                .collect();
            assert_eq!(mine.len(), 60);
            let mut gap = 0;
            for s in out {
                if s.id.starts_with('f') {
                    assert!((if dense { 2..=4 } else { 5..=9 }).contains(&gap));
                    gap = 0;
                } else {
                    gap += 1;
                }
            }
        }
    }
}
#[test]
fn weave_short_list_adds_nothing() {
    assert_eq!(weave(songs(2), songs(5), false).len(), 2);
}
#[test]
fn album_order_disc_then_track_unknown_last() {
    let mut t = songs(4);
    t[0].track = None;
    t[1].track = Some(2);
    t[2].track = Some(1);
    t[3].track = Some(1);
    t[3].disc_number = Some(2);
    sort_album(&mut t);
    assert_eq!(
        t.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
        vec!["s2", "s1", "s0", "s3"]
    );
}
#[test]
fn gain_never_boosts() {
    let mut s = Song::default();
    assert_eq!(s.gain(), 1.);
    s.replay_gain = Some(ReplayGain {
        track_gain: Some(6.),
        ..Default::default()
    });
    assert_eq!(s.gain(), 1.);
    s.replay_gain.as_mut().unwrap().track_gain = Some(-6.);
    assert!((s.gain() - 0.501187).abs() < 0.00001);
}
#[test]
fn radio_deduplicates_recordings_retains_versions() {
    let a = Song {
        id: "a".into(),
        title: "A Song".into(),
        artist: "Artist - Topic".into(),
        ..Default::default()
    };
    let mut b = a.clone();
    b.id = "b".into();
    b.artist = "artist".into();
    b.title = "a song!".into();
    let mut c = b.clone();
    c.id = "c".into();
    c.title = "a song live".into();
    assert_eq!(fresh_radio(&[a], vec![b, c.clone(), c]).len(), 1);
}
#[test]
fn sqlite_queue_survives_restart_paused() {
    let dir = std::env::temp_dir().join(format!("zay-core-test-{}", std::process::id()));
    let core =
        NativeCore::new("http://127.0.0.1:3847".into(), dir.to_string_lossy().into()).unwrap();
    core.queue_command(serde_json::json!({"op":"set","songs":songs(3),"index":1}).to_string())
        .unwrap();
    drop(core);
    let core =
        NativeCore::new("http://127.0.0.1:3847".into(), dir.to_string_lossy().into()).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&core.queue_command("{}".into()).unwrap()).unwrap();
    assert_eq!(v["current"]["id"], "s1");
    drop(core);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn rejects_embedded_credentials() {
    assert!(NativeCore::new(
        "https://fake:fake@example.invalid".into(),
        "/tmp/unused-zay".into()
    )
    .is_err());
}

#[test]
fn prepared_shuffle_repeat_is_the_cycle_actually_played() {
    let dir = std::env::temp_dir().join(format!("zay-prepared-{}", std::process::id()));
    let core = NativeCore::new(
        "https://example.invalid".into(),
        dir.to_string_lossy().into(),
    )
    .unwrap();
    let cmd = |v: serde_json::Value| -> serde_json::Value {
        serde_json::from_str(&core.queue_command(v.to_string()).unwrap()).unwrap()
    };
    cmd(serde_json::json!({"op":"set","songs":songs(3),"index":2}));
    cmd(serde_json::json!({"op":"shuffle"}));
    cmd(serde_json::json!({"op":"repeat"}));
    for _ in 0..30 {
        let snapshot = cmd(serde_json::json!({}));
        let second_snapshot = cmd(serde_json::json!({}));
        assert_eq!(snapshot["upcoming"], second_snapshot["upcoming"]);
        let advanced = cmd(serde_json::json!({"op":"next"}));
        assert_eq!(snapshot["upcoming"][0], advanced["current"]);
    }
    drop(core);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn priority_mutation_invalidates_preparation() {
    let dir = std::env::temp_dir().join(format!("zay-invalidate-{}", std::process::id()));
    let core = NativeCore::new(
        "https://example.invalid".into(),
        dir.to_string_lossy().into(),
    )
    .unwrap();
    core.queue_command(serde_json::json!({"op":"set","songs":songs(3)}).to_string())
        .unwrap();
    let result = core
        .queue_command(
            serde_json::json!({"op":"playNext","songs":[{"id":"priority","title":"Priority"}]})
                .to_string(),
        )
        .unwrap();
    let result: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(result["upcoming"][0]["id"], "priority");
    let result: serde_json::Value =
        serde_json::from_str(&core.queue_command("{\"op\":\"next\"}".into()).unwrap()).unwrap();
    assert_eq!(result["current"]["id"], "priority");
    drop(core);
    std::fs::remove_dir_all(dir).unwrap();
}

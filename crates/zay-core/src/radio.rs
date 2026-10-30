//! Shared autoplay coordination. Network work never holds the queue mutex.
use crate::{
    catalog, err,
    model::{self, Song},
    queue::{Queue, Repeat},
    NativeCore, Result,
};
use rand::{seq::SliceRandom, RngExt};
use serde_json::{json, Value};
use std::{
    sync::atomic::Ordering,
    time::{Duration, Instant},
};
#[derive(Default)]
pub(crate) struct RadioState {
    pub running: bool,
    pub error: String,
    generation: u64,
    epoch: u64,
    result: bool,
    attempted: Option<Instant>,
    waiters: usize,
}
fn eligible(q: &Queue, autoplay: bool) -> bool {
    autoplay
        && q.repeat == Repeat::Off
        && q.sprinkled.is_none()
        && q.current().is_some()
        && q.remaining() <= 2
}
#[uniffi::export]
impl NativeCore {
    pub fn set_autoplay(&self, enabled: bool) -> Result<()> {
        self.autoplay.store(enabled, Ordering::SeqCst);
        self.radio_epoch.fetch_add(1, Ordering::SeqCst);
        self.setting("autoplay".into(), Some(json!(enabled).to_string()))?;
        Ok(())
    }
    /// Called off the UI thread on resume/advance or periodically near the queue end.
    /// Concurrent callers await the same request; stale responses never change a new queue.
    pub fn ensure_radio(&self) -> Result<bool> {
        let (seed, generation, epoch) = {
            let q = self.queue.lock().map_err(|_| err("Queue unavailable"))?;
            if !eligible(&q, self.autoplay.load(Ordering::SeqCst)) {
                return Ok(false);
            }
            (
                q.current().cloned().unwrap(),
                self.generation.load(Ordering::SeqCst),
                self.radio_epoch.load(Ordering::SeqCst),
            )
        };
        let mut state = self.radio.lock().map_err(|_| err("Radio unavailable"))?;
        if state.running {
            state.waiters += 1;
            self.radio_done.notify_all();
            while state.running {
                state = self
                    .radio_done
                    .wait(state)
                    .map_err(|_| err("Radio unavailable"))?;
            }
            state.waiters -= 1;
            return Ok(state.epoch == epoch
                && self.radio_epoch.load(Ordering::SeqCst) == epoch
                && state.generation == generation
                && state.result
                && self.generation.load(Ordering::SeqCst) == generation);
        }
        if state.generation == generation
            && state.epoch == epoch
            && state
                .attempted
                .is_some_and(|at| at.elapsed() < Duration::from_secs(10))
        {
            return Ok(false);
        }
        state.running = true;
        state.generation = generation;
        state.epoch = epoch;
        state.result = false;
        state.error.clear();
        state.attempted = Some(Instant::now());
        drop(state);
        let outcome = (|| -> Result<bool> {
            let recommendations = self.fetch_radio(&seed, 15, false)?;
            let mut q = self.queue.lock().map_err(|_| err("Queue unavailable"))?;
            if self.generation.load(Ordering::SeqCst) != generation
                || self.radio_epoch.load(Ordering::SeqCst) != epoch
                || !self.autoplay.load(Ordering::SeqCst)
                || q.repeat != Repeat::Off
                || q.sprinkled.is_some()
            {
                return Ok(false);
            }
            let mut existing = q.queue.clone();
            existing.extend(q.secondary_queue.clone());
            existing.extend(q.active_song.clone());
            let fresh = model::fresh_radio(&existing, recommendations);
            if fresh.is_empty() {
                return Ok(false);
            }
            q.append(fresh);
            self.prepared
                .lock()
                .map_err(|_| err("Playback plan unavailable"))?
                .clear();
            self.persist_native_queue(&q)?;
            Ok(true)
        })();
        let mut state = self.radio.lock().map_err(|_| err("Radio unavailable"))?;
        state.running = false;
        state.result = outcome.as_ref().copied().unwrap_or(false);
        if outcome.is_err() {
            state.error = "Recommendations unavailable; your queue is preserved.".into();
        }
        self.radio_done.notify_all();
        Ok(state.result)
    }
    /// Start a shown list shuffled, optionally weaving in not-yet-owned recommendations.
    pub fn start_shuffle(&self, songs_json: String, density: String) -> Result<String> {
        let songs = crate::search::playable(
            serde_json::from_str(&songs_json).map_err(|_| err("Invalid songs"))?,
        )?;
        if songs.is_empty() {
            return Err(err("Nothing to shuffle"));
        }
        if !["off", "sparse", "dense"].contains(&density.as_str()) {
            return Err(err("Invalid fresh-song density"));
        }
        let generation = {
            let mut q = self.queue.lock().map_err(|_| err("Queue unavailable"))?;
            let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
            q.shuffle = true;
            q.set(songs.clone(), rand::rng().random_range(0..songs.len()));
            self.playback_epoch.fetch_add(1, Ordering::SeqCst);
            *self
                .resume_position
                .lock()
                .map_err(|_| err("Playback unavailable"))? = Value::Null;
            self.setting("playbackPosition".into(), Some("null".into()))?;
            self.prepared
                .lock()
                .map_err(|_| err("Playback plan unavailable"))?
                .clear();
            self.persist_native_queue(&q)?;
            generation
        };
        let mut warning = String::new();
        let mut fresh = vec![];
        if density != "off" {
            let divisor = if density == "dense" { 3 } else { 7 };
            let need = songs.len().div_ceil(divisor) + 2;
            let seed_count = need.div_ceil(40).clamp(1, 8);
            let mut seeds = songs.clone();
            seeds.shuffle(&mut rand::rng());
            for seed in seeds.iter().take(seed_count) {
                if self.generation.load(Ordering::SeqCst) != generation {
                    return Ok(json!({"applied":false}).to_string());
                }
                match self.fetch_radio(seed, 50, true) {
                    Ok(more) => fresh.extend(more),
                    Err(_) => {
                        warning = "New songs unavailable. Playing your songs shuffled.".into()
                    }
                }
            }
            fresh = model::fresh_radio(&songs, fresh);
            fresh.shuffle(&mut rand::rng());
            fresh.truncate(need);
            if fresh.is_empty() && warning.is_empty() {
                warning = "No new songs arrived. Playing your songs shuffled.".into();
            }
        }
        {
            let mut q = self.queue.lock().map_err(|_| err("Queue unavailable"))?;
            if self.generation.load(Ordering::SeqCst) != generation {
                return Ok(json!({"applied":false}).to_string());
            }
            if !fresh.is_empty() {
                q.set_woven(songs, fresh, density == "dense");
                self.playback_epoch.fetch_add(1, Ordering::SeqCst);
                *self
                    .resume_position
                    .lock()
                    .map_err(|_| err("Playback unavailable"))? = Value::Null;
                self.setting("playbackPosition".into(), Some("null".into()))?;
            }
            self.prepared
                .lock()
                .map_err(|_| err("Playback plan unavailable"))?
                .clear();
            self.persist_native_queue(&q)?;
        }
        Ok(json!({"applied":true,"generation":generation,"warning":warning}).to_string())
    }
}
impl NativeCore {
    fn persist_native_queue(&self, q: &Queue) -> Result<()> {
        self.db
            .lock()
            .map_err(|_| err("Catalog unavailable"))?
            .execute(
                "INSERT OR REPLACE INTO kv VALUES('queue',?1)",
                [serde_json::to_string(q).map_err(|_| err("Invalid queue"))?],
            )
            .map_err(|_| err("Cannot save queue"))?;
        Ok(())
    }
    fn fetch_radio(&self, seed: &Song, limit: usize, exclude: bool) -> Result<Vec<Song>> {
        let mut p = json!({"artist":seed.artist,"title":seed.title,"durationSec":seed.duration.round(),"limit":limit});
        if let Some(id) = seed.id.strip_prefix("yt:") {
            p["videoId"] = json!(id);
        } else {
            p["songId"] = json!(seed.id);
        }
        if exclude {
            p["excludeLibrary"] = json!(1);
        }
        let body: Value = serde_json::from_str(&self.api(
            "fetch".into(),
            "radio".into(),
            "GET".into(),
            p.to_string(),
        )?)
        .map_err(|_| err("Invalid recommendations"))?;
        catalog::list(&body["candidates"])
            .iter()
            .map(|candidate| {
                let mut song: Song = serde_json::from_value(catalog::preview(candidate))
                    .map_err(|_| err("Invalid recommended song"))?;
                song.via_radio = !exclude;
                Ok(song)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        sync::{mpsc, Arc},
        thread,
    };
    fn fixture() -> (
        Arc<NativeCore>,
        std::path::PathBuf,
        mpsc::Receiver<()>,
        mpsc::SyncSender<()>,
        thread::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let dir = std::env::temp_dir().join(format!("zay-radio-{}", rand::random::<u64>()));
        let core = NativeCore::new(
            format!("http://{}", listener.local_addr().unwrap()),
            dir.to_string_lossy().into(),
        )
        .unwrap();
        core.queue_command(
            json!({"op":"set","songs":[{"id":"seed","title":"Seed","artist":"Artist"}]})
                .to_string(),
        )
        .unwrap();
        let (started_tx, started) = mpsc::sync_channel(1);
        let (finish, finish_rx) = mpsc::sync_channel(1);
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut first = String::new();
            reader.read_line(&mut first).unwrap();
            assert!(first.contains("/api/fetch/radio?"));
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
            }
            started_tx.send(()).unwrap();
            finish_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            let body=json!({"candidates":[{"videoId":"fake-next","title":"New recording","channel":"Artist","durationSec":100}]}).to_string();
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        });
        (core, dir, started, finish, handle)
    }
    #[test]
    fn concurrent_radio_callers_share_one_request() {
        let (core, dir, started, finish, server) = fixture();
        let c = core.clone();
        let first = thread::spawn(move || c.ensure_radio().unwrap());
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        let c = core.clone();
        let second = thread::spawn(move || c.ensure_radio().unwrap());
        let mut state = core.radio.lock().unwrap();
        while state.waiters == 0 {
            let (next, timeout) = core
                .radio_done
                .wait_timeout(state, Duration::from_secs(5))
                .unwrap();
            state = next;
            assert!(!timeout.timed_out());
        }
        drop(state);
        finish.send(()).unwrap();
        assert!(first.join().unwrap());
        assert!(second.join().unwrap());
        server.join().unwrap();
        let queue = core.queue.lock().unwrap();
        assert_eq!(queue.queue.len(), 2);
        assert_eq!(queue.queue[1].id, "yt:fake-next");
        drop(queue);
        assert!(
            !core.ensure_radio().unwrap(),
            "Refill cooldown avoids repeated requests"
        );
        drop(core);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn disabling_autoplay_discards_an_inflight_response() {
        let (core, dir, started, finish, server) = fixture();
        let c = core.clone();
        let work = thread::spawn(move || c.ensure_radio().unwrap());
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        core.set_autoplay(false).unwrap();
        finish.send(()).unwrap();
        assert!(!work.join().unwrap());
        server.join().unwrap();
        assert_eq!(core.queue.lock().unwrap().queue.len(), 1);
        drop(core);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn starting_another_list_discards_old_radio() {
        let (core, dir, started, finish, server) = fixture();
        let c = core.clone();
        let work = thread::spawn(move || c.ensure_radio().unwrap());
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        core.queue_command(
            json!({"op":"set","songs":[{"id":"album","title":"New album"}]}).to_string(),
        )
        .unwrap();
        finish.send(()).unwrap();
        assert!(!work.join().unwrap());
        server.join().unwrap();
        assert_eq!(core.queue.lock().unwrap().current().unwrap().id, "album");
        drop(core);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn repeat_and_sprinkle_and_full_queues_suppress_radio() {
        let dir =
            std::env::temp_dir().join(format!("zay-radio-suppress-{}", rand::random::<u64>()));
        let core =
            NativeCore::new("http://127.0.0.1:9".into(), dir.to_string_lossy().into()).unwrap();
        core.queue_command(json!({"op":"set","songs":[{"id":"seed","title":"Seed"}]}).to_string())
            .unwrap();
        for repeat in [Repeat::All, Repeat::One] {
            core.queue.lock().unwrap().repeat = repeat;
            assert!(!core.ensure_radio().unwrap());
            assert!(core.radio.lock().unwrap().attempted.is_none());
        }
        {
            let mut q = core.queue.lock().unwrap();
            q.repeat = Repeat::Off;
            q.sprinkled = Some("sparse".into());
        }
        assert!(!core.ensure_radio().unwrap());
        assert!(core.radio.lock().unwrap().attempted.is_none());
        {
            let mut q = core.queue.lock().unwrap();
            q.sprinkled = None;
            let tracks = (0..5)
                .map(|i| Song {
                    id: format!("s{i}"),
                    ..Default::default()
                })
                .collect();
            q.set(tracks, 0);
        }
        assert!(!core.ensure_radio().unwrap());
        assert!(core.radio.lock().unwrap().attempted.is_none());
        drop(core);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn slow_sprinkle_cannot_replace_a_new_album() {
        let (core, dir, started, finish, server) = fixture();
        let c = core.clone();
        let work = thread::spawn(move || {
            c.start_shuffle(
                json!([{"id":"seed","title":"Seed"}]).to_string(),
                "dense".into(),
            )
            .unwrap()
        });
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        core.queue_command(
            json!({"op":"set","songs":[{"id":"album","title":"Album"}]}).to_string(),
        )
        .unwrap();
        finish.send(()).unwrap();
        let result: Value = serde_json::from_str(&work.join().unwrap()).unwrap();
        assert_eq!(result["applied"], false);
        server.join().unwrap();
        assert_eq!(core.queue.lock().unwrap().current().unwrap().id, "album");
        drop(core);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn weave_order_survives_native_queue_preparation() {
        let dir = std::env::temp_dir().join(format!("zay-weave-plan-{}", rand::random::<u64>()));
        let core =
            NativeCore::new("http://127.0.0.1:9".into(), dir.to_string_lossy().into()).unwrap();
        let mine: Vec<_> = (0..30)
            .map(|i| json!({"id":format!("mine{i}"),"title":format!("Mine {i}")}))
            .collect();
        let fresh: Vec<_> = (0..6)
            .map(|i| json!({"id":format!("fresh{i}"),"title":format!("Fresh {i}")}))
            .collect();
        let result: Value = serde_json::from_str(
            &core
                .queue_command(
                    json!({"op":"weave","songs":mine,"fresh":fresh,"dense":true}).to_string(),
                )
                .unwrap(),
        )
        .unwrap();
        let woven = result["state"]["queue"].as_array().unwrap();
        for (i, song) in woven.iter().enumerate() {
            let snapshot: Value =
                serde_json::from_str(&core.queue_command("{}".into()).unwrap()).unwrap();
            assert_eq!(snapshot["current"]["id"], song["id"]);
            if i + 1 < woven.len() {
                core.queue_command("{\"op\":\"next\"}".into()).unwrap();
            }
        }
        assert!(!core.ensure_radio().unwrap());
        drop(core);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn disabling_then_reenabling_still_discards_the_old_request() {
        let (core, dir, started, finish, server) = fixture();
        let c = core.clone();
        let work = thread::spawn(move || c.ensure_radio().unwrap());
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        core.set_autoplay(false).unwrap();
        core.set_autoplay(true).unwrap();
        finish.send(()).unwrap();
        assert!(!work.join().unwrap());
        server.join().unwrap();
        assert_eq!(core.queue.lock().unwrap().queue.len(), 1);
        drop(core);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn failed_sprinkler_preserves_a_playable_plain_shuffle() {
        let dir =
            std::env::temp_dir().join(format!("zay-shuffle-fallback-{}", rand::random::<u64>()));
        let core =
            NativeCore::new("http://127.0.0.1:9".into(), dir.to_string_lossy().into()).unwrap();
        let result: Value = serde_json::from_str(
            &core
                .start_shuffle(
                    json!([{"id":"a","title":"A"},{"id":"b","title":"B"}]).to_string(),
                    "dense".into(),
                )
                .unwrap(),
        )
        .unwrap();
        assert_eq!(result["applied"], true);
        assert!(result["warning"].as_str().unwrap().contains("shuffled"));
        let queue = core.queue.lock().unwrap();
        assert!(queue.shuffle);
        assert!(queue.sprinkled.is_none());
        assert!(queue.current().is_some());
        assert_eq!(queue.queue.len(), 2);
        drop(queue);
        drop(core);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

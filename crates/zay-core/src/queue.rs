use crate::model::Song;
use rand::{seq::SliceRandom, RngExt};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Queue {
    pub queue: Vec<Song>,
    pub secondary_queue: Vec<Song>,
    pub active_song: Option<Song>,
    pub index: isize,
    pub history: Vec<Song>,
    pub shuffle: bool,
    pub repeat: Repeat,
    pub shuffle_order: Vec<usize>,
    pub shuffle_pos: isize,
    pub sprinkled: Option<String>,
}
impl Default for Queue {
    fn default() -> Self {
        Self {
            queue: vec![],
            secondary_queue: vec![],
            active_song: None,
            index: -1,
            history: vec![],
            shuffle: false,
            repeat: Repeat::Off,
            shuffle_order: vec![],
            shuffle_pos: -1,
            sprinkled: None,
        }
    }
}
impl Queue {
    pub fn current(&self) -> Option<&Song> {
        self.active_song.as_ref().or_else(|| {
            usize::try_from(self.index)
                .ok()
                .and_then(|i| self.queue.get(i))
        })
    }
    pub fn set(&mut self, songs: Vec<Song>, start: usize) {
        self.index = if songs.is_empty() {
            -1
        } else {
            start.min(songs.len() - 1) as isize
        };
        self.queue = songs;
        self.sprinkled = None;
        self.secondary_queue.clear();
        self.active_song = None;
        self.history.clear();
        if self.shuffle {
            self.rebuild();
        } else {
            self.shuffle_order.clear();
            self.shuffle_pos = -1;
        }
    }
    pub fn remaining(&self) -> usize {
        self.secondary_queue.len()
            + if self.shuffle {
                (self.shuffle_order.len() as isize - self.shuffle_pos - 1).max(0) as usize
            } else {
                (self.queue.len() as isize - self.index - 1).max(0) as usize
            }
    }
    pub fn jump(&mut self, i: usize) {
        if i >= self.queue.len() {
            return;
        }
        if let Some(song) = self.current().cloned() {
            self.history.push(song);
        }
        self.index = i as isize;
        self.active_song = None;
        self.shuffle_pos = self
            .shuffle_order
            .iter()
            .position(|n| *n == i)
            .map(|p| p as isize)
            .unwrap_or(-1);
    }
    pub fn set_woven(&mut self, songs: Vec<Song>, fresh: Vec<Song>, dense: bool) {
        self.shuffle = false;
        let has_fresh = !fresh.is_empty();
        self.set(weave(songs, fresh, dense), 0);
        self.shuffle = true;
        self.shuffle_order = (0..self.queue.len()).collect();
        self.shuffle_pos = if self.queue.is_empty() { -1 } else { 0 };
        self.sprinkled = has_fresh.then(|| if dense { "dense" } else { "sparse" }.into());
    }
    pub fn rebuild(&mut self) {
        self.shuffle_order = (0..self.queue.len()).collect();
        self.shuffle_order.shuffle(&mut rand::rng());
        self.shuffle_pos = -1;
        if let Some(p) = self
            .shuffle_order
            .iter()
            .position(|i| *i as isize == self.index)
        {
            self.shuffle_order.swap(0, p);
            self.shuffle_pos = 0;
        }
    }
    pub fn toggle_shuffle(&mut self) {
        self.sprinkled = None;
        self.shuffle = !self.shuffle;
        if self.shuffle {
            self.rebuild();
        } else {
            self.shuffle_order.clear();
            self.shuffle_pos = -1;
        }
    }
    pub fn advance(&mut self) -> bool {
        if self.queue.is_empty() && self.secondary_queue.is_empty() {
            return false;
        }
        if self.repeat == Repeat::One {
            return self.current().is_some();
        }
        if let Some(s) = self.current().cloned() {
            self.history.push(s);
        }
        if !self.secondary_queue.is_empty() {
            self.active_song = Some(self.secondary_queue.remove(0));
            return true;
        }
        let index = if self.shuffle && self.shuffle_order.len() == self.queue.len() {
            let mut pos = self.shuffle_pos + 1;
            if pos as usize >= self.shuffle_order.len() {
                if self.repeat != Repeat::All {
                    self.shuffle_pos = pos;
                    return false;
                }
                self.rebuild();
                pos = 0;
            }
            self.shuffle_pos = pos;
            self.shuffle_order
                .get(pos as usize)
                .copied()
                .map(|i| i as isize)
        } else {
            let mut i = self.index + 1;
            if i as usize >= self.queue.len() {
                if self.repeat != Repeat::All {
                    return false;
                }
                i = 0;
            }
            Some(i)
        };
        if let Some(i) = index {
            self.index = i;
            self.active_song = None;
            true
        } else {
            false
        }
    }
    pub fn previous(&mut self) {
        if let Some(last) = self.history.pop() {
            if let Some(active) = self.active_song.take() {
                if !self.queue.iter().any(|s| s.id == active.id) {
                    self.secondary_queue.retain(|s| s.id != active.id);
                    self.secondary_queue.insert(0, active);
                }
            }
            self.secondary_queue.retain(|s| s.id != last.id);
            if let Some(i) = self.queue.iter().position(|s| s.id == last.id) {
                self.index = i as isize;
                if let Some(p) = self.shuffle_order.iter().position(|v| *v == i) {
                    self.shuffle_pos = p as isize;
                }
            } else {
                self.active_song = Some(last);
            }
        } else {
            self.index = (self.index - if self.shuffle { 0 } else { 1 }).max(0);
        }
    }
    pub fn append(&mut self, songs: Vec<Song>) {
        self.secondary_queue
            .retain(|s| !songs.iter().any(|n| n.id == s.id));
        let old = self.queue.len();
        self.queue.extend(songs);
        if self.index < 0
            && self.active_song.is_none()
            && self.secondary_queue.is_empty()
            && !self.queue.is_empty()
        {
            self.index = 0;
        }
        if self.shuffle {
            self.shuffle_order.extend(old..self.queue.len());
        }
    }
    pub fn priority(&mut self, songs: Vec<Song>, last: bool) {
        let mut unique: Vec<Song> = vec![];
        for s in songs {
            if let Some(i) = unique.iter().position(|v| v.id == s.id) {
                unique[i] = s;
            } else {
                unique.push(s);
            }
        }
        let keep: Vec<usize> = (0..self.queue.len())
            .filter(|i| !unique.iter().any(|s| s.id == self.queue[*i].id))
            .collect();
        let playing = self.current().cloned();
        if self.active_song.is_none()
            && playing
                .as_ref()
                .is_some_and(|s| unique.iter().any(|n| n.id == s.id))
        {
            self.active_song = playing;
        }
        let old = self.index;
        self.index = keep.iter().filter(|i| **i as isize <= old).count() as isize - 1;
        self.shuffle_pos = self
            .shuffle_order
            .iter()
            .take((self.shuffle_pos + 1).max(0) as usize)
            .filter(|i| keep.contains(i))
            .count() as isize
            - 1;
        self.shuffle_order = self
            .shuffle_order
            .iter()
            .filter_map(|i| keep.iter().position(|v| v == i))
            .collect();
        self.queue = keep.iter().map(|i| self.queue[*i].clone()).collect();
        self.secondary_queue
            .retain(|s| !unique.iter().any(|n| n.id == s.id));
        if last {
            self.secondary_queue.extend(unique);
        } else {
            unique.append(&mut self.secondary_queue);
            self.secondary_queue = unique;
        }
    }
    pub fn jump_secondary(&mut self, i: usize) {
        if i < self.secondary_queue.len() {
            if let Some(s) = self.current().cloned() {
                self.history.push(s);
            }
            self.active_song = Some(self.secondary_queue.remove(i));
        }
    }
    pub fn remove(&mut self, i: usize) {
        if i >= self.queue.len() {
            return;
        }
        self.queue.remove(i);
        if (i as isize) < self.index {
            self.index -= 1;
        } else if i as isize == self.index {
            self.index = self.index.min(self.queue.len() as isize - 1);
        }
        self.shuffle_order = self
            .shuffle_order
            .iter()
            .filter(|v| **v != i)
            .map(|v| if *v > i { *v - 1 } else { *v })
            .collect();
        self.shuffle_pos = self
            .shuffle_order
            .iter()
            .position(|v| *v as isize == self.index)
            .map(|p| p as isize)
            .unwrap_or(self.shuffle_pos.min(self.shuffle_order.len() as isize - 1));
    }
    pub fn move_song(&mut self, from: usize, to: usize) {
        if from >= self.queue.len() || to >= self.queue.len() {
            return;
        }
        let remap = |i: usize| {
            if i == from {
                to
            } else if from < to && i > from && i <= to {
                i - 1
            } else if from > to && i >= to && i < from {
                i + 1
            } else {
                i
            }
        };
        let s = self.queue.remove(from);
        self.queue.insert(to, s);
        if self.index >= 0 {
            self.index = remap(self.index as usize) as isize;
        }
        self.shuffle_order = self.shuffle_order.iter().map(|i| remap(*i)).collect();
    }
}
pub fn weave(mut songs: Vec<Song>, fresh: Vec<Song>, dense: bool) -> Vec<Song> {
    let mut rng = rand::rng();
    songs.shuffle(&mut rng);
    let (min, max) = if dense { (2, 4) } else { (5, 9) };
    let mut gap = rng.random_range(min..=max);
    let mut out = vec![];
    let mut f = fresh.into_iter();
    for s in songs {
        out.push(s);
        gap -= 1;
        if gap == 0 {
            if let Some(s) = f.next() {
                out.push(s);
            }
            gap = rng.random_range(min..=max);
        }
    }
    out
}

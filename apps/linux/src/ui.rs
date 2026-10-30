//! Presentation helpers: artwork, rows and styling around stock GTK/libadwaita widgets.
use relm4::gtk::{self, gdk, gio, glib, prelude::*};
use serde_json::Value;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    mpsc, Arc, Mutex,
};
use zay_core::NativeCore;

pub const CSS: &str = "
@define-color accent_bg_color #1db954;
@define-color accent_color #1ed760;
@define-color accent_fg_color #000000;
window.main-window, .content-pane, .content-pane headerbar, .player-bar { background-color: #121212; }
.sidebar-pane, .sidebar-pane headerbar { background-color: #000000; }
.navigation-sidebar { background: transparent; }
.navigation-sidebar > row:selected { background-color: alpha(white, 0.1); }
.sidebar-heading { color: alpha(white, 0.6); font-size: 0.8em; font-weight: 700; letter-spacing: 0.06em; margin: 14px 12px 4px 12px; }
.app-title { font-weight: 800; }
.hero { padding: 8px 0 4px 0; }
.hero-kind { font-size: 0.85em; font-weight: 700; }
.hero-title { font-size: 44px; font-weight: 800; letter-spacing: -0.02em; }
.hero-meta { color: alpha(white, 0.7); }
.section-title { font-size: 1.35em; font-weight: 800; margin-top: 16px; }
.track-list { background: transparent; }
.track-list > row { border-radius: 6px; }
.track-list > row:hover { background-color: alpha(white, 0.07); }
.track-title { font-weight: 600; }
.track-title.now-playing { color: #1ed760; }
.dim { color: alpha(white, 0.62); }
.badge { font-size: 0.75em; font-weight: 700; padding: 1px 6px; border-radius: 4px; background-color: alpha(white, 0.14); }
.badge.ready { background-color: alpha(#1ed760, 0.18); color: #1ed760; }
.badge.failed { background-color: alpha(#ff7b63, 0.18); color: #ff7b63; }
.play-hero { min-width: 56px; min-height: 56px; background-color: #1ed760; color: #000000; }
.play-hero:hover { background-color: #3be477; }
.play-main { min-width: 36px; min-height: 36px; background-color: #ffffff; color: #000000; }
.play-main:hover { background-color: #e8e8e8; }
.player-bar { border-top: 1px solid alpha(white, 0.08); }
.player-title { font-weight: 700; }
togglebutton.flat:checked { background: transparent; color: #1ed760; }
switch:checked { background-color: #1db954; }
.art { border-radius: 4px; }
.art.large { border-radius: 8px; box-shadow: 0 8px 24px alpha(black, 0.5); }
.art.round { border-radius: 9999px; }
.art-glyph { color: alpha(white, 0.78); }
.art-hue-0 { background-image: linear-gradient(135deg, #4b3a8f, #1f1b3a); }
.art-hue-1 { background-image: linear-gradient(135deg, #8f3a5c, #2e1424); }
.art-hue-2 { background-image: linear-gradient(135deg, #2f6f8f, #102633); }
.art-hue-3 { background-image: linear-gradient(135deg, #8f6a2f, #33240f); }
.art-hue-4 { background-image: linear-gradient(135deg, #2f8f63, #0f3322); }
.art-hue-5 { background-image: linear-gradient(135deg, #6f2f8f, #261033); }
.art-hue-6 { background-image: linear-gradient(135deg, #8f3f2f, #33150f); }
.art-hue-7 { background-image: linear-gradient(135deg, #3f4f6f, #151b26); }
.art-liked { background-image: linear-gradient(135deg, #450af5, #8e8ee5 60%, #c4efd9); }
.art-liked .art-glyph { color: #ffffff; }
.shortcut { background-color: alpha(white, 0.08); border-radius: 6px; }
.shortcut:hover { background-color: alpha(white, 0.14); }
.shortcut > button.flat { padding: 0; border-radius: 6px 0 0 6px; }
.shortcut-play { min-width: 32px; min-height: 32px; padding: 0; background-color: #1ed760; color: #000000; }
.shortcut-play:hover { background-color: #3be477; }
button.liked { color: #1ed760; }
.page-bar { padding: 8px 32px; }
.warning-text { color: #ffb44d; }
.correction { color: alpha(white, 0.7); }
.correction button { font-weight: 700; color: #1ed760; }
.sidebar-badge { font-size: 0.75em; font-weight: 700; padding: 0 6px; border-radius: 9999px; background-color: #ff7b63; color: #000000; }
";

/// What an artwork tile depicts; selects the placeholder glyph and shape.
#[derive(Clone, Copy, PartialEq)]
pub enum Art {
    Song,
    Album,
    Artist,
    Playlist,
    Genre,
    Liked,
    Other,
}
impl Art {
    pub fn for_kind(kind: &str) -> Self {
        match kind {
            "songs" => Art::Song,
            "albums" | "newAlbums" => Art::Album,
            "artists" => Art::Artist,
            "playlists" => Art::Playlist,
            "genres" => Art::Genre,
            _ => Art::Other,
        }
    }
    fn icon(self) -> &'static str {
        match self {
            Art::Song => "audio-x-generic-symbolic",
            Art::Album => "media-optical-symbolic",
            Art::Artist => "avatar-default-symbolic",
            Art::Playlist => "view-list-bullet-symbolic",
            Art::Genre => "starred-symbolic",
            Art::Liked => "emote-love-symbolic",
            Art::Other => "folder-music-symbolic",
        }
    }
}

struct Job {
    item: String,
    px: u32,
    generation: Option<u64>,
    target: glib::SendWeakRef<gtk::Picture>,
}

/// A small dedicated worker pool so artwork never occupies GIO's shared task threads.
/// Page-scoped requests are dropped once the user navigates elsewhere.
#[derive(Clone)]
pub struct ArtLoader {
    jobs: mpsc::Sender<Job>,
    generation: Arc<AtomicU64>,
}
impl ArtLoader {
    pub fn new(core: Arc<NativeCore>) -> Self {
        let (jobs, receiver) = mpsc::channel::<Job>();
        let receiver = Arc::new(Mutex::new(receiver));
        let generation = Arc::new(AtomicU64::new(0));
        for _ in 0..4 {
            let receiver = receiver.clone();
            let core = core.clone();
            let generation = generation.clone();
            std::thread::spawn(move || loop {
                let Ok(job) = receiver.lock().unwrap().recv() else {
                    return;
                };
                if job
                    .generation
                    .is_some_and(|g| g != generation.load(Ordering::SeqCst))
                {
                    continue;
                }
                let Ok(bytes) = core.artwork(job.item, job.px) else {
                    continue;
                };
                let Ok(texture) = gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes)) else {
                    continue;
                };
                let target = job.target;
                glib::idle_add_once(move || {
                    if let Some(picture) = target.upgrade() {
                        picture.set_paintable(Some(&texture));
                    }
                });
            });
        }
        Self { jobs, generation }
    }
    /// Invalidate queued page artwork after navigation.
    pub fn next_page(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }
    /// A square (or round, for artists) tile. A deterministic gradient with a symbolic
    /// glyph shows immediately; real cover art replaces it when the gateway provides one.
    pub fn tile(&self, item: &Value, art: Art, size: i32, page_scoped: bool) -> gtk::Widget {
        let frame = gtk::Overlay::new();
        frame.set_size_request(size, size);
        frame.set_halign(gtk::Align::Start);
        frame.set_valign(gtk::Align::Center);
        frame.set_overflow(gtk::Overflow::Hidden);
        frame.add_css_class("art");
        if size >= 120 {
            frame.add_css_class("large");
        }
        if art == Art::Artist {
            frame.add_css_class("round");
        }
        let name = item["title"]
            .as_str()
            .or(item["name"].as_str())
            .or(item["value"].as_str())
            .unwrap_or("");
        if art == Art::Liked {
            frame.add_css_class("art-liked");
        } else {
            let hue = name
                .bytes()
                .fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32))
                % 8;
            frame.add_css_class(&format!("art-hue-{hue}"));
        }
        let glyph = gtk::Image::from_icon_name(art.icon());
        glyph.set_pixel_size((size * 2 / 5).max(12));
        glyph.add_css_class("art-glyph");
        frame.set_child(Some(&glyph));
        frame.update_property(&[gtk::accessible::Property::Label(name)]);
        frame.set_accessible_role(gtk::AccessibleRole::Img);
        let has_source = ["artworkUrl", "coverArt", "albumId"]
            .iter()
            .any(|k| item[*k].as_str().is_some_and(|s| !s.is_empty()));
        if has_source && art != Art::Liked {
            let picture = gtk::Picture::new();
            picture.set_content_fit(gtk::ContentFit::Cover);
            picture.set_can_shrink(true);
            frame.add_overlay(&picture);
            // Size buckets keep the cache small while staying sharp at 2x scale.
            let px = match size {
                0..=48 => 96,
                49..=120 => 240,
                _ => 600,
            };
            let _ = self.jobs.send(Job {
                item: item.to_string(),
                px,
                generation: page_scoped.then(|| self.generation.load(Ordering::SeqCst)),
                target: glib::SendWeakRef::from(picture.downgrade()),
            });
        }
        frame.upcast()
    }
}

pub fn label(text: &str, classes: &[&str]) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    for class in classes {
        label.add_css_class(class);
    }
    label
}

pub fn clock(seconds: f64) -> String {
    let total = seconds.max(0.) as u64;
    if total >= 3600 {
        format!("{}:{:02}:{:02}", total / 3600, total / 60 % 60, total % 60)
    } else {
        format!("{}:{:02}", total / 60, total % 60)
    }
}

/// "1 hr 5 min" style totals for collection headers.
pub fn total_duration(seconds: f64) -> String {
    let minutes = (seconds / 60.).round() as u64;
    if minutes >= 60 {
        format!("{} hr {} min", minutes / 60, minutes % 60)
    } else {
        format!("{minutes} min")
    }
}

pub fn count(n: usize, singular: &str) -> String {
    format!("{n} {singular}{}", if n == 1 { "" } else { "s" })
}

pub fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::from_icon_name(icon);
    button.set_tooltip_text(Some(tooltip));
    button.update_property(&[gtk::accessible::Property::Label(tooltip)]);
    button.add_css_class("flat");
    button.add_css_class("circular");
    button.set_valign(gtk::Align::Center);
    button
}

/// A native popover menu whose items emit `group.act(<op>)` string actions.
pub fn action_menu(
    sections: &[Vec<(String, String)>],
    group: &str,
    on_action: impl Fn(String) + 'static,
) -> gtk::MenuButton {
    let menu = gio::Menu::new();
    for section in sections.iter().filter(|s| !s.is_empty()) {
        let part = gio::Menu::new();
        for (text, op) in section {
            let item = gio::MenuItem::new(Some(text), None);
            item.set_action_and_target_value(Some(&format!("{group}.act")), Some(&op.to_variant()));
            part.append_item(&item);
        }
        menu.append_section(None, &part);
    }
    let actions = gio::SimpleActionGroup::new();
    let act = gio::SimpleAction::new("act", Some(glib::VariantTy::STRING));
    act.connect_activate(move |_, value| {
        if let Some(op) = value.and_then(|v| v.get::<String>()) {
            on_action(op);
        }
    });
    actions.add_action(&act);
    let button = gtk::MenuButton::new();
    button.set_icon_name("view-more-symbolic");
    button.set_menu_model(Some(&menu));
    button.insert_action_group(group, Some(&actions));
    button.add_css_class("flat");
    button.add_css_class("circular");
    button.set_valign(gtk::Align::Center);
    button
}

/// JSON null and empty strings both mean "absent".
pub fn present(value: &Value) -> bool {
    !(value.is_null() || value.as_str().is_some_and(str::is_empty))
}

/// Mixed rows carry a non-song `browseKind`; only song rows enter the queue.
pub fn is_song(row: &Value) -> bool {
    row["browseKind"]
        .as_str()
        .is_none_or(|kind| kind == "songs")
}

/// Server-library songs; YouTube previews must be added before saving, downloading or deleting.
pub fn is_library_song(row: &Value) -> bool {
    is_song(row)
        && row["id"]
            .as_str()
            .is_some_and(|id| !id.is_empty() && !id.starts_with("yt:"))
}

/// The album or artist a library song belongs to, as a row the collection opener accepts.
pub fn album_link(song: &Value) -> Option<Value> {
    let id = song["albumId"]
        .as_str()
        .filter(|s| !s.is_empty() && is_library_song(song))?;
    let mut row = serde_json::json!({"id":id,"name":song["album"].as_str().unwrap_or("Album")});
    for key in ["coverArt", "artist"] {
        if present(&song[key]) {
            row[key] = song[key].clone();
        }
    }
    Some(row)
}
pub fn artist_link(song: &Value) -> Option<Value> {
    let id = song["artistId"]
        .as_str()
        .filter(|s| !s.is_empty() && is_library_song(song))?;
    Some(serde_json::json!({"id":id,"name":song["artist"].as_str().unwrap_or("Artist")}))
}

/// Search filters narrow the core's ranked mixed sections, keeping order and section titles.
pub fn filter_search(rows: &[Value], sections: &Value, filter: &str) -> (Vec<Value>, Value) {
    let keep = |row: &Value| {
        let kind = row["browseKind"].as_str().unwrap_or("songs");
        let preview = row["id"].as_str().is_some_and(|id| id.starts_with("yt:"));
        match filter {
            "songs" => kind == "songs",
            "albums" => kind == "albums" || kind == "newAlbums",
            "library" => kind != "newAlbums" && !preview,
            _ => true,
        }
    };
    let mut ranges: Vec<(String, usize, usize)> = sections
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|s| {
                    let start = s["start"].as_u64()? as usize;
                    let count = s["count"].as_u64()? as usize;
                    (start + count <= rows.len()).then(|| {
                        (
                            s["title"].as_str().unwrap_or("").to_owned(),
                            start,
                            start + count,
                        )
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    if ranges.is_empty() {
        ranges.push((String::new(), 0, rows.len()));
    }
    let mut kept = vec![];
    let mut kept_sections = vec![];
    for (title, start, end) in ranges {
        let matches: Vec<Value> = rows[start..end]
            .iter()
            .filter(|r| keep(r))
            .cloned()
            .collect();
        if matches.is_empty() {
            continue;
        }
        if !title.is_empty() {
            kept_sections
                .push(serde_json::json!({"title":title,"start":kept.len(),"count":matches.len()}));
        }
        kept.extend(matches);
    }
    (kept, Value::Array(kept_sections))
}

/// "3 songs · 2 albums · 1 artist" for mixed search and library rows.
pub fn mixed_count(rows: &[Value]) -> String {
    let mut counts = [0usize; 4];
    for row in rows {
        match row["browseKind"].as_str().unwrap_or("songs") {
            "albums" | "newAlbums" => counts[1] += 1,
            "artists" => counts[2] += 1,
            "playlists" | "liked" => counts[3] += 1,
            _ => counts[0] += 1,
        }
    }
    let parts: Vec<String> = counts
        .iter()
        .zip(["song", "album", "artist", "playlist"])
        .filter(|(n, _)| **n > 0)
        .map(|(n, noun)| count(*n, noun))
        .collect();
    if parts.is_empty() {
        "No results".into()
    } else {
        parts.join(" · ")
    }
}

/// ZayFetch job states grouped as the Jobs filters show them.
pub fn job_bucket(status: &str) -> &'static str {
    match status {
        "queued" | "pending" | "running" | "downloading" => "active",
        "failed" | "cancelled" | "error" => "attention",
        _ => "done",
    }
}

/// Copy for a YouTube Music connection that blocks imports and new downloads.
pub fn youtube_attention(youtube: &Value) -> String {
    if !youtube.is_object() {
        return String::new();
    }
    if youtube["configured"] != true {
        return "Connect YouTube Music in Settings to import playlists and add new songs.".into();
    }
    if youtube["verified"] != true {
        return "YouTube Music needs attention. Reconnect it in Settings.".into();
    }
    String::new()
}

/// What the player says when an item cannot play. Never includes URLs or raw errors.
pub fn playback_failure(offline: bool, preview: bool, local: bool) -> &'static str {
    match (local, offline, preview) {
        (true, _, _) => {
            "The downloaded file can't be played. Remove the download and download it again."
        }
        (_, true, true) => "Previews need a connection to your server.",
        (_, true, false) => {
            "This song isn't downloaded. Connect to your server, or download it to play offline."
        }
        (_, false, true) => {
            "This preview couldn't be played. Try again, or add the song to your library."
        }
        _ => "This song couldn't be played. Check the connection and try again.",
    }
}

/// Keep the reader's place when a page is rebuilt with fresh data.
pub fn restore_scroll(adjustment: &gtk::Adjustment, value: f64) {
    adjustment.set_value(value);
    let handler: std::rc::Rc<std::cell::Cell<Option<glib::SignalHandlerId>>> = Default::default();
    let slot = handler.clone();
    let id = adjustment.connect_changed(move |adj| {
        if adj.upper() - adj.page_size() >= value {
            adj.set_value(value);
            if let Some(id) = slot.take() {
                adj.disconnect(id);
            }
        }
    });
    handler.set(Some(id));
    let adjustment = adjustment.downgrade();
    glib::timeout_add_local_once(std::time::Duration::from_millis(600), move || {
        if let (Some(adj), Some(id)) = (adjustment.upgrade(), handler.take()) {
            adj.disconnect(id);
        }
    });
}

/// A linked row of stock toggle buttons acting as one exclusive choice.
pub fn toggle_group(
    options: &[(String, String)],
    selected: &str,
    on_select: impl Fn(String) + Clone + 'static,
) -> gtk::Box {
    let group = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    group.add_css_class("linked");
    let mut first: Option<gtk::ToggleButton> = None;
    for (id, title) in options {
        let button = gtk::ToggleButton::with_label(title);
        if let Some(first) = &first {
            button.set_group(Some(first));
        } else {
            first = Some(button.clone());
        }
        button.set_active(id == selected);
        let id = id.clone();
        let on_select = on_select.clone();
        button.connect_toggled(move |b| {
            if b.is_active() {
                on_select(id.clone());
            }
        });
        group.append(&button);
    }
    group
}

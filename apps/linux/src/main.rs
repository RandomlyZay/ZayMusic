use std::collections::{HashSet, VecDeque};
mod manage;
mod mpris;
mod queue_view;
mod tags;
mod ui;
use gstreamer::{self as gst, prelude::*};
use relm4::adw::{self, prelude::*};
use relm4::RelmWidgetExt;
use relm4::{gtk, gtk::gio, gtk::glib, ComponentParts, ComponentSender, RelmApp, SimpleComponent};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use ui::{Art, ArtLoader};
use zay_core::NativeCore;
struct App {
    core: Arc<NativeCore>,
    art: ArtLoader,
    window: adw::ApplicationWindow,
    player: gst::Element,
    _bus_watch: gst::bus::BusWatchGuard,
    queue_box: gtk::Box,
    queue_dialog: Option<adw::Dialog>,
    split: adw::OverlaySplitView,
    body: gtk::Box,
    scroll: gtk::ScrolledWindow,
    page_bar: gtk::Box,
    page_bar_key: String,
    sidebar: gtk::ListBox,
    jobs_badge: gtk::Label,
    search: gtk::SearchEntry,
    now_art: gtk::Box,
    toasts: adw::ToastOverlay,
    autoplay_action: gio::SimpleAction,
    title: String,
    artist: String,
    current_id: String,
    current: Value,
    starred: bool,
    preview: bool,
    status: String,
    playing: bool,
    page: String,
    header: String,
    query: String,
    context: Value,
    rows: Vec<Value>,
    source_rows: Vec<Value>,
    kind: String,
    extra: Value,
    source_extra: Value,
    search_filter: String,
    library_options: Value,
    job_filter: String,
    connection_warning: String,
    attention: usize,
    attention_polls: u8,
    selecting: bool,
    selected: HashSet<String>,
    trail: Vec<Value>,
    offline: bool,
    loading: bool,
    request: u64,
    wants_playback: bool,
    failed: bool,
    pending_seek: Option<f64>,
    radio_inflight: bool,
    resume_after_radio: bool,
    radio_tick: u8,
    last_radio_error: String,
    input: relm4::Sender<Msg>,
    pending: Arc<Mutex<VecDeque<String>>>,
    stream_started: Arc<AtomicBool>,
    autoplay_enabled: bool,
    volume: f64,
    position: f64,
    duration: f64,
    shuffle: bool,
    repeat: String,
    playlists: Vec<Value>,
}
/// What a playlist picker adds: songs (one or a bulk selection) or an album the core acquires.
#[derive(Debug, Clone)]
enum Selection {
    Songs(Vec<Value>),
    Album(Value),
}
#[derive(Debug)]
enum Bus {
    Error { offline: bool },
    Eos,
    Ready,
}
/// How a page load presents itself: a spinner, a quiet user refresh, or a background poll.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Full,
    Refresh,
    Poll,
}
#[derive(Debug)]
enum Msg {
    Page(String),
    Sidebar(String),
    SidebarPlaylists(Vec<Value>),
    Search(String, bool),
    SearchFilter(String),
    LibraryOptions(String, String),
    JobFilter(String),
    Loaded(u64, String, Value),
    LoadError(u64, String),
    Error(String),
    PlayAt(usize),
    PlayCollection,
    PlayContext(Value),
    StartPlayback,
    SaveCurrent,
    Toggle,
    Next,
    Previous,
    Shuffle,
    Repeat,
    Seek(f64),
    SeekBy(i64),
    SetPosition(String, i64),
    SetLoop(String),
    SetShuffle(bool),
    Volume(f64),
    ManagementDone(String),
    LibraryCommand(Value),
    LibraryDone(Value, Value),
    CreateForAlbum(String, Value),
    RefreshCatalog,
    QueueToggle,
    QueueClosed,
    ToggleSidebar,
    Back,
    Action(Value, String),
    Tick,
    Bus(Bus),
    RadioDone(bool),
    Reload,
    Autoplay(bool),
    ShuffleList(String),
    ShuffleReady(Value),
    DownloadAll,
    AskClearDownloads,
    ClearDownloads,
    DownloadsChanged,
    PlaylistEdit(Option<Value>, Option<Selection>),
    PlaylistDelete(Value),
    PlaylistCommand(Value),
    PlaylistDone(u64, Value, Value),
    PickPlaylist(Selection),
    PlaylistChoices(Selection, Vec<Value>),
    Selecting(bool),
    ToggleSelected(String),
    SelectAll,
    ConnectionWarning(String),
    AttentionChecked,
    ShowFailedJobs,
}
fn parse(s: &str) -> Value {
    serde_json::from_str(s).unwrap_or(Value::Null)
}
fn songs(v: &Value) -> Vec<Value> {
    v.as_array().cloned().unwrap_or_default()
}
/// Sidebar destinations: (page, icon). `None` entries are section headings.
const NAVIGATION: &[(&str, Option<&str>)] = &[
    ("Home", Some("go-home-symbolic")),
    ("Search", Some("system-search-symbolic")),
    ("Library", Some("view-app-grid-symbolic")),
    ("Liked Songs", Some("emote-love-symbolic")),
    ("Playlists", Some("view-list-bullet-symbolic")),
    ("Albums", Some("media-optical-symbolic")),
    ("Artists", Some("avatar-default-symbolic")),
    ("Downloads", Some("folder-download-symbolic")),
    ("Queue", Some("view-list-ordered-symbolic")),
    ("Browse", None),
    ("New music", Some("star-new-symbolic")),
    ("All Albums", Some("folder-music-symbolic")),
    ("All Artists", Some("audio-headphones-symbolic")),
    ("Settings", Some("emblem-system-symbolic")),
    ("Recent Songs", Some("document-open-recent-symbolic")),
    ("Recent Collections", Some("folder-recent-symbolic")),
    ("Recently Added", Some("document-new-symbolic")),
    ("Recently Played", Some("document-open-recent-symbolic")),
    ("Genres", Some("view-grid-symbolic")),
    ("Songs", Some("audio-x-generic-symbolic")),
    ("Jobs", Some("folder-download-symbolic")),
    ("Imports", Some("folder-download-symbolic")),
    ("YouTube Playlists", Some("folder-music-symbolic")),
];
/// Pages that show the omnibar's results and search filters.
const SEARCH_PAGES: &[&str] = &["Search", "Omnibar", "New music"];
const MANAGEMENT_PAGES: &[&str] = &["Jobs", "Imports", "YouTube Playlists"];
fn display_title(page: &str) -> String {
    match page {
        "Omnibar" => "Search",
        "Library" => "Your Library",
        other => other,
    }
    .into()
}
fn liked_context() -> Value {
    json!({"kind":"liked","id":"liked","name":"Liked Songs"})
}
/// The notice shown after a shared library command succeeds.
fn library_notice(op: &str, kind: &str, result: &Value) -> String {
    match (op, kind) {
        ("save", "songs") => "Added to Liked Songs".into(),
        ("unsave", "songs") => "Removed from Liked Songs".into(),
        ("save", "artists") => "Following artist".into(),
        ("unsave", "artists") => "Unfollowed artist".into(),
        ("save", _) => "Saved to Your Library".into(),
        ("unsave", _) => "Removed from Your Library".into(),
        ("acquireAlbum", _) => {
            let queued = result["queued"].as_u64().unwrap_or(0);
            let failed = result["failed"].as_u64().unwrap_or(0);
            if failed > 0 {
                format!("{queued} songs queued, {failed} failed. Open Jobs to retry.")
            } else {
                format!("Album saved. {queued} songs queued for your library.")
            }
        }
        ("deleteSong", _) => "Deleted from server".into(),
        ("editTags", _) => "Tags updated".into(),
        _ => "Library updated".into(),
    }
}
#[relm4::component]
impl SimpleComponent for App {
    type Init = Arc<NativeCore>;
    type Input = Msg;
    type Output = ();
    view! {
     adw::ApplicationWindow {
      set_title:Some("ZayMusic"), set_default_width:1100, set_default_height:760, add_css_class:"main-window",
       gtk::Box { set_orientation:gtk::Orientation::Vertical,
        #[local_ref] split -> adw::OverlaySplitView { set_vexpand:true, set_min_sidebar_width:220., set_max_sidebar_width:260., set_sidebar_width_fraction:0.22,
         #[wrap(Some)] set_sidebar = &adw::ToolbarView { add_css_class:"sidebar-pane",
          add_top_bar = &adw::HeaderBar { set_show_end_title_buttons:false,
           #[wrap(Some)] set_title_widget = &gtk::Label { set_label:"ZayMusic", add_css_class:"app-title" },
          },
          #[wrap(Some)] set_content = &gtk::ScrolledWindow { set_hscrollbar_policy:gtk::PolicyType::Never,
           #[local_ref] sidebar -> gtk::ListBox { add_css_class:"navigation-sidebar",
            connect_row_activated[sender] => move |_, row| sender.input(Msg::Sidebar(row.widget_name().to_string())),
           },
          },
         },
         #[wrap(Some)] set_content = &adw::ToolbarView { add_css_class:"content-pane",
          add_top_bar = &adw::HeaderBar { set_show_start_title_buttons:false,
           pack_start = &gtk::Button {set_icon_name:"sidebar-show-symbolic",set_tooltip_text:Some("Your Library"),connect_clicked=>Msg::ToggleSidebar},
           pack_start = &gtk::Button {set_icon_name:"go-previous-symbolic",set_tooltip_text:Some("Back"),#[watch] set_visible:!model.trail.is_empty(),connect_clicked=>Msg::Back},
           #[wrap(Some)] set_title_widget = &adw::Clamp { set_maximum_size:520, set_hexpand:true,
            #[local_ref] search -> gtk::SearchEntry { set_hexpand:true, set_placeholder_text:Some("Search or paste a YouTube URL"), set_search_delay:250,
             connect_search_changed[sender] => move |entry| { sender.input(Msg::Search(entry.text().to_string(), false)); },
             connect_activate[sender] => move |entry| { sender.input(Msg::Search(entry.text().to_string(), true)); },
            },
           },
           pack_end = &gtk::MenuButton { set_icon_name:"open-menu-symbolic", set_tooltip_text:Some("Main menu"), set_menu_model:Some(&primary_menu), set_primary:true },
          },
          add_top_bar = &adw::Banner { set_title:"Offline · showing your saved library", #[watch] set_revealed:model.offline },
          #[wrap(Some)] set_content = &gtk::Box {
           #[local_ref] toasts -> adw::ToastOverlay { set_hexpand:true, set_vexpand:true,
            #[wrap(Some)] set_child = &gtk::Box { set_orientation:gtk::Orientation::Vertical,
             #[local_ref] page_bar -> gtk::Box { set_spacing:12, add_css_class:"page-bar", set_visible:false },
             #[local_ref] scroll -> gtk::ScrolledWindow { set_vexpand:true, set_hscrollbar_policy:gtk::PolicyType::Never,
              #[local_ref] body -> gtk::Box { set_orientation:gtk::Orientation::Vertical, set_spacing:16, set_margin_start:32, set_margin_end:32, set_margin_top:20, set_margin_bottom:24 },
             },
            },
           },
          },
         },
        },
        gtk::CenterBox { add_css_class:"player-bar", set_height_request:84,
         #[wrap(Some)] set_start_widget = &gtk::Box { set_spacing:12, set_margin_start:16, set_width_request:220,
          #[local_ref] now_art -> gtk::Box { set_valign:gtk::Align::Center },
          gtk::Box { set_orientation:gtk::Orientation::Vertical, set_valign:gtk::Align::Center, set_spacing:2,
           gtk::Label { #[watch] set_label:&model.title, add_css_class:"player-title", set_xalign:0., set_ellipsize:gtk::pango::EllipsizeMode::End, set_max_width_chars:26 },
           gtk::Label { #[watch] set_label:&model.artist, add_css_class:"dim", add_css_class:"caption", set_xalign:0., set_ellipsize:gtk::pango::EllipsizeMode::End, set_max_width_chars:26 },
          },
          gtk::Button { set_valign:gtk::Align::Center, add_css_class:"flat", add_css_class:"circular",
           #[watch] set_visible:!model.current_id.is_empty(),
           #[watch] set_icon_name:if model.preview {"list-add-symbolic"} else {"emote-love-symbolic"},
           #[watch] set_tooltip_text:Some(if model.preview {"Add to library"} else if model.starred {"Remove from Liked Songs"} else {"Add to Liked Songs"}),
           #[watch] set_class_active:("liked", model.starred && !model.preview),
           connect_clicked=>Msg::SaveCurrent,
          },
         },
         #[wrap(Some)] set_center_widget = &gtk::Box { set_orientation:gtk::Orientation::Vertical, set_spacing:2, set_width_request:380, set_valign:gtk::Align::Center,
          gtk::Box { set_spacing:16, set_halign:gtk::Align::Center,
           gtk::ToggleButton { set_icon_name:"media-playlist-shuffle-symbolic", add_css_class:"flat", add_css_class:"circular", set_tooltip_text:Some("Shuffle"), #[watch] set_active:model.shuffle, connect_clicked=>Msg::Shuffle },
           gtk::Button { set_icon_name:"media-skip-backward-symbolic", add_css_class:"flat", add_css_class:"circular", set_tooltip_text:Some("Previous"), connect_clicked=>Msg::Previous },
           gtk::Button { #[watch] set_icon_name:if model.playing {"media-playback-pause-symbolic"} else {"media-playback-start-symbolic"}, add_css_class:"circular", add_css_class:"play-main", #[watch] set_tooltip_text:Some(if model.playing {"Pause"} else {"Play"}), connect_clicked=>Msg::Toggle },
           gtk::Button { set_icon_name:"media-skip-forward-symbolic", add_css_class:"flat", add_css_class:"circular", set_tooltip_text:Some("Next"), connect_clicked=>Msg::Next },
           gtk::ToggleButton { #[watch] set_icon_name:if model.repeat == "one" {"media-playlist-repeat-song-symbolic"} else {"media-playlist-repeat-symbolic"}, add_css_class:"flat", add_css_class:"circular", #[watch] set_tooltip_text:Some(match model.repeat.as_str() {"all" => "Repeat: all", "one" => "Repeat: one", _ => "Repeat: off"}), #[watch] set_active:model.repeat != "off", connect_clicked=>Msg::Repeat },
          },
          gtk::Box { set_spacing:8,
           gtk::Label { #[watch] set_label:&ui::clock(model.position), add_css_class:"dim", add_css_class:"caption", add_css_class:"numeric", set_width_chars:5, set_xalign:1. },
           gtk::Scale { set_hexpand:true, set_draw_value:false, set_tooltip_text:Some("Seek"), #[watch] set_sensitive:model.duration > 0., #[watch] set_range:(0., model.duration.max(1.)), #[watch] set_value:model.position,
            connect_change_value[sender] => move |_, _, value| { sender.input(Msg::Seek(value)); glib::Propagation::Proceed },
           },
           gtk::Label { #[watch] set_label:&ui::clock(model.duration), add_css_class:"dim", add_css_class:"caption", add_css_class:"numeric", set_width_chars:5, set_xalign:0. },
          },
         },
         #[wrap(Some)] set_end_widget = &gtk::Box { set_margin_end:16, set_halign:gtk::Align::End, set_width_request:120,
          gtk::MenuButton { set_icon_name:"audio-volume-high-symbolic", set_valign:gtk::Align::Center, set_tooltip_text:Some("Volume"),
           #[wrap(Some)] set_popover = &gtk::Popover {
            gtk::Scale { set_orientation:gtk::Orientation::Horizontal, set_range:(0.,1.), set_width_request:160, set_draw_value:false, #[watch] set_value:model.volume,
             connect_change_value[sender] => move |_, _, value| {sender.input(Msg::Volume(value));glib::Propagation::Proceed},
            },
           },
          },
          gtk::ToggleButton { set_icon_name:"view-list-ordered-symbolic", add_css_class:"flat", add_css_class:"circular", set_valign:gtk::Align::Center, set_tooltip_text:Some("Queue"), #[watch] set_active:model.page == "Queue", connect_clicked=>Msg::QueueToggle },
         },
        },
       },
     }
    }
    fn init(
        core: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);
        gst::init().expect("GStreamer initialization");
        let player = gst::ElementFactory::make("playbin3")
            .build()
            .expect("playbin3 required");
        if std::env::var_os("ZAY_SILENT").is_some() {
            let sink = gst::ElementFactory::make("fakesink")
                .property("sync", true)
                .build()
                .unwrap();
            player.set_property("audio-sink", &sink);
        }
        let pending: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));
        let stream_started = Arc::new(AtomicBool::new(false));
        let next_core = core.clone();
        let next_pending = pending.clone();
        player.connect("about-to-finish", false, move |values| {
            if let Ok(player) = values[0].get::<gst::Element>() {
                let uri = {
                    let mut pending = next_pending.lock().unwrap();
                    next_core
                        .queue_command("{}".into())
                        .ok()
                        .and_then(|snapshot| {
                            let value = parse(&snapshot);
                            let next = &value["upcoming"][pending.len()];
                            if next.is_null() {
                                return None;
                            }
                            let uri = next_core.media_url(next.to_string(), false).ok()?;
                            pending.push_back(next["id"].as_str()?.to_owned());
                            Some(uri)
                        })
                };
                if let Some(uri) = uri {
                    player.set_property("uri", uri);
                }
            }
            None
        });
        let bus_core = core.clone();
        let bus_pending = pending.clone();
        let bus_started = stream_started.clone();
        let weak_player = player.downgrade();
        let input = sender.input_sender().clone();
        let bus = player.bus().unwrap();
        bus.set_sync_handler(move |_, message| {
            if matches!(message.view(), gst::MessageView::StreamStart(_)) {
                if bus_started.swap(true, Ordering::SeqCst) {
                    let mut pending = bus_pending.lock().unwrap();
                    let expected = pending.pop_front();
                    if let Some(expected) = expected {
                        if let Ok(snapshot) = bus_core.queue_command("{}".into()) {
                            let old = parse(&snapshot);
                            let song = &old["current"];
                            let duration = song["duration"].as_f64().unwrap_or(0.);
                            let _ = bus_core.clone().playback_progress(
                                old["playbackEpoch"].as_u64().unwrap_or(0),
                                song["id"].as_str().unwrap_or("").into(),
                                duration,
                                duration,
                                true,
                            );
                        }
                        if let Ok(result) = bus_core.queue_command("{\"op\":\"next\"}".into()) {
                            if parse(&result)["current"]["id"].as_str() != Some(&expected) {
                                input.emit(Msg::Reload);
                            }
                        }
                    }
                }
                if let (Some(player), Ok(snapshot)) =
                    (weak_player.upgrade(), bus_core.queue_command("{}".into()))
                {
                    player.set_property("volume", parse(&snapshot)["gain"].as_f64().unwrap_or(1.));
                }
            }
            gst::BusSyncReply::Pass
        });
        // Errors, end of stream and preroll completion reach the UI thread as messages.
        let tx = sender.input_sender().clone();
        let bus_watch = bus
            .add_watch_local(move |_, message| {
                match message.view() {
                    gst::MessageView::Error(error) => {
                        let offline = matches!(
                            error.error().kind::<gst::ResourceError>(),
                            Some(
                                gst::ResourceError::OpenRead
                                    | gst::ResourceError::OpenReadWrite
                                    | gst::ResourceError::NotFound
                                    | gst::ResourceError::Read
                            )
                        );
                        tx.emit(Msg::Bus(Bus::Error { offline }));
                    }
                    gst::MessageView::Eos(_) => tx.emit(Msg::Bus(Bus::Eos)),
                    gst::MessageView::AsyncDone(_) => tx.emit(Msg::Bus(Bus::Ready)),
                    _ => {}
                }
                glib::ControlFlow::Continue
            })
            .expect("GStreamer bus watch");
        let autoplay_enabled = core
            .queue_command("{}".into())
            .map(|s| parse(&s)["autoplay"].as_bool().unwrap_or(true))
            .unwrap_or(true);
        // The primary menu's Autoplay item is a stateful action, rendered as a native check item.
        let autoplay_action =
            gio::SimpleAction::new_stateful("autoplay", None, &autoplay_enabled.to_variant());
        let tx = sender.input_sender().clone();
        autoplay_action.connect_change_state(move |action, value| {
            if let Some(value) = value.and_then(|v| v.get::<bool>()) {
                action.set_state(&value.to_variant());
                tx.emit(Msg::Autoplay(value));
            }
        });
        let settings_action = gio::SimpleAction::new("settings", None);
        let tx = sender.input_sender().clone();
        settings_action.connect_activate(move |_, _| tx.emit(Msg::Page("Settings".into())));
        let app_actions = gio::SimpleActionGroup::new();
        app_actions.add_action(&autoplay_action);
        app_actions.add_action(&settings_action);
        root.insert_action_group("zay", Some(&app_actions));
        let primary_menu = gio::Menu::new();
        primary_menu.append(Some("Autoplay"), Some("zay.autoplay"));
        primary_menu.append(Some("Settings"), Some("zay.settings"));
        let core_volume = core.volume();
        let jobs_badge = ui::label("", &["sidebar-badge"]);
        jobs_badge.set_visible(false);
        jobs_badge.set_hexpand(true);
        jobs_badge.set_halign(gtk::Align::End);
        let model = App {
            art: ArtLoader::new(core.clone()),
            core,
            window: root.clone(),
            player,
            _bus_watch: bus_watch,
            queue_box: gtk::Box::new(gtk::Orientation::Vertical, 12),
            queue_dialog: None,
            split: adw::OverlaySplitView::new(),
            body: gtk::Box::default(),
            scroll: gtk::ScrolledWindow::new(),
            page_bar: gtk::Box::default(),
            page_bar_key: String::new(),
            sidebar: gtk::ListBox::new(),
            jobs_badge,
            search: gtk::SearchEntry::new(),
            now_art: gtk::Box::default(),
            toasts: adw::ToastOverlay::new(),
            autoplay_action,
            title: "Nothing playing".into(),
            artist: String::new(),
            current_id: String::new(),
            current: Value::Null,
            starred: false,
            preview: false,
            status: String::new(),
            playing: false,
            page: "Home".into(),
            header: "Home".into(),
            query: String::new(),
            context: Value::Null,
            rows: vec![],
            source_rows: vec![],
            kind: "songs".into(),
            extra: Value::Null,
            source_extra: Value::Null,
            search_filter: "all".into(),
            library_options: json!({"filter":"all","sort":"recent","text":""}),
            job_filter: "all".into(),
            connection_warning: String::new(),
            attention: 0,
            attention_polls: 0,
            selecting: false,
            selected: HashSet::new(),
            trail: vec![],
            offline: false,
            loading: true,
            request: 0,
            wants_playback: false,
            failed: false,
            pending_seek: None,
            radio_inflight: false,
            resume_after_radio: false,
            radio_tick: 0,
            last_radio_error: String::new(),
            input: sender.input_sender().clone(),
            pending,
            stream_started,
            autoplay_enabled,
            volume: core_volume,
            position: 0.,
            duration: 0.,
            shuffle: false,
            repeat: "off".into(),
            playlists: vec![],
        };
        mpris::register(
            model.core.clone(),
            model.player.clone(),
            sender.input_sender().clone(),
        );
        let toasts = &model.toasts;
        let sidebar = &model.sidebar;
        let search = &model.search;
        let body = &model.body;
        let scroll = &model.scroll;
        let page_bar = &model.page_bar;
        let now_art = &model.now_art;
        let split = &model.split;
        let widgets = view_output!();
        let narrow =
            adw::Breakpoint::new(adw::BreakpointCondition::parse("max-width: 800sp").unwrap());
        narrow.add_setter(split, "collapsed", Some(&true.to_value()));
        root.add_breakpoint(narrow);
        model.build_sidebar();
        let tick = sender.input_sender().clone();
        gtk::glib::timeout_add_seconds_local(1, move || {
            tick.emit(Msg::Tick);
            gtk::glib::ControlFlow::Continue
        });
        // The listener's place survives quitting mid-song.
        let close_core = model.core.clone();
        let close_player = model.player.clone();
        root.connect_close_request(move |_| {
            if let Some(position) = close_player
                .query_position::<gst::ClockTime>()
                .map(|t| t.seconds_f64())
                .filter(|p| *p > 0.)
            {
                if let Ok(text) = close_core.queue_command("{}".into()) {
                    let v = parse(&text);
                    let _ = close_core.save_playback_position(
                        v["playbackEpoch"].as_u64().unwrap_or(0),
                        v["current"]["id"].as_str().unwrap_or("").into(),
                        position,
                    );
                }
            }
            glib::Propagation::Proceed
        });
        let mut model = model;
        model.refresh_now(true);
        // Restore the saved queue paused; the saved position applies once GStreamer prerolls.
        if !model.current.is_null() {
            model.start(false);
        }
        model.refresh_sidebar_playlists();
        model.refresh_attention();
        model.check_server_attention();
        sender.input(Msg::Page("Home".into()));
        ComponentParts { model, widgets }
    }
    fn update(&mut self, msg: Msg, sender: ComponentSender<Self>) {
        match msg {
            Msg::Page(page) => {
                self.trail.clear();
                self.job_filter = "all".into();
                let query = self.page_query(&page);
                self.load(page, query, Mode::Full);
            }
            Msg::Sidebar(name) => {
                self.trail.clear();
                self.selecting = false;
                if let Some(page) = name.strip_prefix("page:") {
                    if page == "Search" || page == "New music" {
                        self.search.grab_focus();
                    }
                    // The Jobs badge leads straight to what needs attention.
                    self.job_filter = if page == "Jobs" && self.attention > 0 {
                        "attention"
                    } else {
                        "all"
                    }
                    .into();
                    let query = self.page_query(page);
                    self.load(page.into(), query, Mode::Full);
                } else if let Some(index) = name
                    .strip_prefix("playlist:")
                    .and_then(|i| i.parse::<usize>().ok())
                {
                    if let Some(playlist) = self.playlists.get(index).cloned() {
                        self.open("playlists".into(), playlist, Mode::Full);
                    }
                }
            }
            Msg::SidebarPlaylists(playlists) => {
                if playlists != self.playlists {
                    self.playlists = playlists;
                    self.build_sidebar();
                }
            }
            Msg::Search(query, immediate) => {
                let on_search = SEARCH_PAGES.contains(&self.page.as_str());
                // The entry echoes programmatic text and Enter after a pause; search once.
                if on_search && query == self.query && (!immediate || self.loading) {
                    return;
                }
                if !on_search && query.trim().is_empty() && !immediate {
                    return;
                }
                let page = if self.page == "New music" {
                    "New music"
                } else {
                    "Omnibar"
                };
                self.selecting = false;
                self.selected.clear();
                if on_search && self.page == page && !self.loading {
                    self.load(page.into(), query, Mode::Refresh);
                } else {
                    self.trail.clear();
                    self.search_filter = "all".into();
                    self.load(page.into(), query, Mode::Full);
                }
            }
            Msg::SearchFilter(filter) => {
                if filter != self.search_filter {
                    self.search_filter = filter;
                    self.apply_filter();
                    self.render(false);
                }
            }
            Msg::LibraryOptions(key, value) => {
                if self.library_options[&key] != json!(value) {
                    self.library_options[&key] = json!(value);
                    self.load(
                        "Library".into(),
                        self.library_options.to_string(),
                        Mode::Refresh,
                    );
                }
            }
            Msg::JobFilter(filter) => {
                self.job_filter = if self.job_filter == filter {
                    "all".into()
                } else {
                    filter
                };
                self.render(true);
            }
            Msg::Loaded(request, page, data) => {
                if request != self.request {
                    return;
                }
                let background = !self.loading;
                let rows = songs(&data["rows"]);
                let unchanged = background
                    && self.status.is_empty()
                    && rows == self.source_rows
                    && data["extra"] == self.source_extra
                    && data["context"] == self.context
                    && (data["offline"] == true) == self.offline;
                if !unchanged {
                    self.context = data["context"].clone();
                    self.header = display_title(data["title"].as_str().unwrap_or(&page));
                    self.kind = data["kind"].as_str().unwrap_or("songs").into();
                    self.source_extra = data["extra"].clone();
                    self.offline = data["offline"] == true;
                    self.source_rows = rows;
                    self.loading = false;
                    self.status = String::new();
                    self.apply_filter();
                    if self.kind == "jobs" {
                        // Once nothing needs attention (e.g. after Retry), show every job rather than an empty filter.
                        if self.job_filter == "attention"
                            && !self.source_rows.iter().any(|r| {
                                ui::job_bucket(r["status"].as_str().unwrap_or("")) == "attention"
                            })
                        {
                            self.job_filter = "all".into();
                        }
                        // Opening Jobs re-read the server's failures in the core.
                        self.refresh_attention();
                    }
                    self.render(background);
                }
                self.record_ui_state(false);
            }
            Msg::PlayCollection => {
                if self
                    .core
                    .play_collection(
                        json!(self.rows).to_string(),
                        self.play_context().to_string(),
                    )
                    .is_ok()
                {
                    self.start(true);
                }
            }
            Msg::PlayAt(index) => {
                let mut start = 0;
                let mut count = self.rows.len();
                for section in songs(&self.extra["sections"]) {
                    let s = section["start"].as_u64().unwrap_or(0) as usize;
                    let n = section["count"].as_u64().unwrap_or(0) as usize;
                    if index >= s && index < s + n {
                        start = s;
                        count = n;
                        break;
                    }
                }
                let end = (start + count).min(self.rows.len());
                if index >= start
                    && index < end
                    && self
                        .core
                        .play_rows(
                            json!(self.rows[start..end]).to_string(),
                            (index - start) as u32,
                            self.play_context().to_string(),
                        )
                        .is_ok()
                {
                    self.start(true);
                }
            }
            Msg::PlayContext(item) => {
                let core = self.core.clone();
                let tx = self.input.clone();
                std::thread::spawn(move || match core.play_context(item.to_string()) {
                    Ok(_) => tx.emit(Msg::StartPlayback),
                    Err(_) => tx.emit(Msg::Error(
                        "Couldn't play this collection. Check the connection and retry.".into(),
                    )),
                });
            }
            Msg::StartPlayback => self.start(true),
            Msg::SaveCurrent => {
                let song = self.current.clone();
                if song.is_null() {
                    return;
                }
                if self.preview {
                    sender.input(Msg::Action(song, "acquire".into()));
                } else {
                    let op = if self.starred { "unsave" } else { "save" };
                    sender.input(Msg::LibraryCommand(
                        json!({"op":op,"kind":"songs","item":song}),
                    ));
                }
            }
            Msg::Toggle => {
                if self.playing {
                    let position = self.clock();
                    let _ = self.player.set_state(gst::State::Paused);
                    self.playing = false;
                    self.wants_playback = false;
                    self.save_position(position);
                } else if self.player.property::<Option<String>>("uri").is_some() && !self.failed {
                    let _ = self.player.set_state(gst::State::Playing);
                    self.playing = true;
                    self.wants_playback = true;
                    self.request_radio();
                    let core = self.core.clone();
                    std::thread::spawn(move || {
                        let _ = core.warm_upcoming();
                    });
                } else {
                    self.start(true);
                }
                mpris::notify(&self.core, &self.player);
            }
            Msg::Next | Msg::Previous => {
                let op = if matches!(msg, Msg::Next) {
                    "next"
                } else {
                    "previous"
                };
                if let Ok(v) = self
                    .core
                    .queue_command(json!({"op":op,"position":self.clock()}).to_string())
                {
                    let v = parse(&v);
                    if v["restart"] == true {
                        self.seek_to(0.);
                    } else if v["continue"] == true {
                        self.start(self.wants_playback);
                    } else {
                        let _ = self.player.set_state(gst::State::Paused);
                        self.playing = false;
                    }
                }
                self.refresh_now(false);
            }
            Msg::Shuffle => {
                let _ = self.core.queue_command("{\"op\":\"shuffle\"}".into());
                self.refresh_now(false);
                mpris::notify(&self.core, &self.player);
            }
            Msg::Repeat => {
                let _ = self.core.queue_command("{\"op\":\"repeat\"}".into());
                self.refresh_now(false);
                mpris::notify(&self.core, &self.player);
            }
            Msg::SetShuffle(on) => {
                if on != self.shuffle {
                    let _ = self.core.queue_command("{\"op\":\"shuffle\"}".into());
                }
                self.refresh_now(false);
                mpris::notify(&self.core, &self.player);
            }
            Msg::SetLoop(target) => {
                // The core cycles off → all → one; step until the requested mode.
                for _ in 0..3 {
                    let mode = self
                        .core
                        .queue_command("{}".into())
                        .map(|s| {
                            parse(&s)["state"]["repeat"]
                                .as_str()
                                .unwrap_or("off")
                                .to_owned()
                        })
                        .unwrap_or_default();
                    if mode == target {
                        break;
                    }
                    let _ = self.core.queue_command("{\"op\":\"repeat\"}".into());
                }
                self.refresh_now(false);
                mpris::notify(&self.core, &self.player);
            }
            Msg::Seek(seconds) => self.seek_to(seconds),
            Msg::SeekBy(offset) => {
                let target = self.clock() + offset as f64 / 1e6;
                if self.duration > 0. && target > self.duration {
                    sender.input(Msg::Next);
                } else {
                    self.seek_to(target.max(0.));
                }
            }
            Msg::SetPosition(track, position) => {
                let seconds = position as f64 / 1e6;
                if !self.current_id.is_empty()
                    && track == mpris::track_path(&self.current_id)
                    && seconds >= 0.
                    && (self.duration <= 0. || seconds <= self.duration)
                {
                    self.seek_to(seconds);
                }
            }
            Msg::LibraryCommand(command) => {
                let core = self.core.clone();
                let tx = self.input.clone();
                std::thread::spawn(move || match core.library_command(command.to_string()) {
                    Ok(text) => tx.emit(Msg::LibraryDone(command, parse(&text))),
                    Err(_) => tx.emit(Msg::Error(
                        match command["op"].as_str().unwrap_or("") {
                            "deleteSong" => {
                                "Song could not be deleted. Check the connection and retry."
                            }
                            "editTags" => {
                                "Tags could not be saved. Check the connection and retry."
                            }
                            "acquireAlbum" => "Album could not be saved. Open Jobs to retry.",
                            _ => "Library could not be updated. Check the connection and retry.",
                        }
                        .into(),
                    )),
                });
            }
            Msg::LibraryDone(command, result) => {
                self.toast(&library_notice(
                    command["op"].as_str().unwrap_or(""),
                    command["kind"].as_str().unwrap_or("songs"),
                    &result,
                ));
                // Likes and tag edits update the playing song without touching audio.
                self.refresh_now(false);
                mpris::notify(&self.core, &self.player);
                sender.input(Msg::RefreshCatalog);
            }
            Msg::CreateForAlbum(name, album) => {
                let core = self.core.clone();
                let tx = self.input.clone();
                std::thread::spawn(move || {
                    // Albums are acquired by the core; create the empty target first.
                    let created = core
                        .playlist_command(json!({"op":"create","name":name,"songs":[]}).to_string())
                        .map(|t| parse(&t));
                    let Some(id) = created
                        .ok()
                        .and_then(|c| c["playlist"]["id"].as_str().map(str::to_owned))
                    else {
                        tx.emit(Msg::Error("Playlist could not be created.".into()));
                        return;
                    };
                    let command = json!({"op":"acquireAlbum","kind":"newAlbums","item":album,"playlistId":id});
                    match core.library_command(command.to_string()) {
                        Ok(text) => tx.emit(Msg::LibraryDone(command, parse(&text))),
                        Err(_) => tx.emit(Msg::Error(
                            "Album could not be saved. Open Jobs to retry.".into(),
                        )),
                    }
                });
            }
            Msg::RefreshCatalog => {
                self.refresh_page(Mode::Refresh);
                self.refresh_sidebar_playlists();
            }
            Msg::Volume(value) => {
                if self.core.set_volume(value).is_ok() {
                    self.volume = self.core.volume();
                    if let Ok(text) = self.core.queue_command("{}".into()) {
                        self.player
                            .set_property("volume", parse(&text)["gain"].as_f64().unwrap_or(1.));
                    }
                }
            }
            Msg::ManagementDone(notice) => {
                self.toast(&notice);
                self.refresh_page(Mode::Refresh);
                self.check_server_attention();
            }
            Msg::AttentionChecked => self.refresh_attention(),
            Msg::ToggleSidebar => self.split.set_show_sidebar(!self.split.shows_sidebar()),
            Msg::QueueClosed => {
                self.queue_dialog = None;
            }
            Msg::QueueToggle => self.show_queue(),
            Msg::Back => {
                let Some(entry) = self.trail.pop() else {
                    return;
                };
                self.selecting = false;
                self.job_filter = entry["jobFilter"].as_str().unwrap_or("all").into();
                if let Some(kind) = entry["open"][0].as_str() {
                    self.open(kind.into(), entry["open"][1].clone(), Mode::Full);
                } else {
                    let page = entry["page"].as_str().unwrap_or("Home").to_owned();
                    let query = entry["query"].as_str().unwrap_or("").to_owned();
                    self.search_filter = entry["filter"].as_str().unwrap_or("all").into();
                    if SEARCH_PAGES.contains(&page.as_str()) {
                        self.query = query.clone();
                        self.search.set_text(&query);
                    }
                    self.load(page, query, Mode::Full);
                }
            }
            Msg::ShowFailedJobs => {
                self.push_trail();
                self.job_filter = "attention".into();
                self.load("Jobs".into(), String::new(), Mode::Full);
            }
            Msg::Action(song, op) => self.action(song, op, &sender),
            Msg::Error(e) => self.toast(&e),
            Msg::LoadError(request, error) => {
                if request != self.request {
                    return;
                }
                if self.loading {
                    self.status = error;
                    self.loading = false;
                    self.rows.clear();
                    self.source_rows.clear();
                    self.render(false);
                }
                self.record_ui_state(false);
            }
            Msg::PlaylistEdit(target, seed) => {
                let target = target.map(|v| if v.is_null() { self.context.clone() } else { v });
                let dialog = adw::AlertDialog::new(
                    Some(if target.is_some() {
                        "Rename playlist"
                    } else {
                        "New playlist"
                    }),
                    None,
                );
                dialog.add_responses(&[("cancel", "Cancel"), ("save", "Save")]);
                dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
                dialog.set_default_response(Some("save"));
                dialog.set_close_response("cancel");
                let entry = gtk::Entry::new();
                entry.set_activates_default(true);
                entry.set_placeholder_text(Some("Playlist name"));
                if let Some(target) = &target {
                    entry.set_text(target["name"].as_str().unwrap_or(""));
                }
                entry.select_region(0, -1);
                dialog.set_extra_child(Some(&entry));
                dialog.set_focus(Some(&entry));
                let tx = self.input.clone();
                dialog.connect_response(None, move |_, response| {
                    if response != "save" {
                        return;
                    }
                    let name = entry.text().to_string();
                    tx.emit(match &seed {
                        Some(Selection::Album(album)) => Msg::CreateForAlbum(name, album.clone()),
                        Some(Selection::Songs(songs)) => {
                            Msg::PlaylistCommand(json!({"op":"create","name":name,"songs":songs}))
                        }
                        None => Msg::PlaylistCommand(
                            json!({"op":if target.is_some(){"rename"}else{"create"},"id":target.as_ref().map(|v|v["id"].clone()).unwrap_or(Value::Null),"name":name}),
                        ),
                    });
                });
                dialog.present(Some(&self.window));
            }
            Msg::PlaylistDelete(target) => {
                let target = if target.is_null() {
                    self.context.clone()
                } else {
                    target
                };
                let dialog = adw::AlertDialog::new(
                    Some(&format!(
                        "Delete {}?",
                        target["name"].as_str().unwrap_or("playlist")
                    )),
                    Some("This removes the playlist from your library. Its songs are kept."),
                );
                dialog.add_responses(&[("cancel", "Cancel"), ("delete", "Delete")]);
                dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
                dialog.set_close_response("cancel");
                let tx = self.input.clone();
                dialog.connect_response(None, move |_, response| {
                    if response == "delete" {
                        tx.emit(Msg::PlaylistCommand(
                            json!({"op":"delete","id":target["id"]}),
                        ));
                    }
                });
                dialog.present(Some(&self.window));
            }
            Msg::PickPlaylist(selection) => {
                let core = self.core.clone();
                let tx = self.input.clone();
                std::thread::spawn(move || match core.browse("Playlists".into(), "".into()) {
                    Ok(text) => tx.emit(Msg::PlaylistChoices(
                        selection,
                        songs(&parse(&text)["rows"]),
                    )),
                    Err(_) => tx.emit(Msg::Error("Could not load playlists.".into())),
                });
            }
            Msg::PlaylistChoices(selection, choices) => self.playlist_picker(selection, choices),
            Msg::PlaylistCommand(command) => {
                let core = self.core.clone();
                let tx = self.input.clone();
                let request = self.request;
                std::thread::spawn(move || match core.playlist_command(command.to_string()) {
                    Ok(text) => tx.emit(Msg::PlaylistDone(request, command, parse(&text))),
                    Err(error) => tx.emit(Msg::Error(error.to_string())),
                });
            }
            Msg::PlaylistDone(request, command, value) => {
                let op = command["op"].as_str().unwrap_or("");
                self.refresh_sidebar_playlists();
                if request != self.request {
                    return;
                }
                self.toast(value["warning"].as_str().unwrap_or(
                    if value["queued"].as_u64().unwrap_or(0) > 0 {
                        "Queued for download and playlist."
                    } else {
                        "Playlist updated."
                    },
                ));
                if self.selecting {
                    self.selecting = false;
                    self.selected.clear();
                    self.render(true);
                }
                if self.context["kind"] == "playlists" && ["rename", "move", "remove"].contains(&op)
                {
                    self.open("playlists".into(), self.context.clone(), Mode::Refresh);
                } else if ["delete", "rename"].contains(&op)
                    || (op == "create" && command["songs"].is_null())
                {
                    self.trail.clear();
                    self.load("Playlists".into(), String::new(), Mode::Full);
                }
            }
            Msg::DownloadAll => {
                let library: Vec<Value> = self
                    .rows
                    .iter()
                    .filter(|r| ui::is_library_song(r))
                    .cloned()
                    .collect();
                if library.is_empty() {
                    return;
                }
                let core = self.core.clone();
                let tx = self.input.clone();
                let source = json!(library).to_string();
                self.toast(&format!("Downloading {}", ui::count(library.len(), "song")));
                std::thread::spawn(move || {
                    if let Err(error) = core.download_many(source) {
                        tx.emit(Msg::Error(error.to_string()));
                    }
                    tx.emit(Msg::DownloadsChanged);
                });
            }
            Msg::AskClearDownloads => {
                let dialog = adw::AlertDialog::new(
                    Some("Remove all offline downloads?"),
                    Some("Music on your server is kept."),
                );
                dialog.add_responses(&[("cancel", "Cancel"), ("remove", "Remove all")]);
                dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
                dialog.set_close_response("cancel");
                let tx = self.input.clone();
                dialog.connect_response(None, move |_, response| {
                    if response == "remove" {
                        tx.emit(Msg::ClearDownloads);
                    }
                });
                dialog.present(Some(&self.window));
            }
            Msg::ClearDownloads => {
                if let Err(error) = self.core.delete_all_downloads() {
                    self.toast(&error.to_string());
                }
                self.load("Downloads".into(), String::new(), Mode::Refresh);
            }
            Msg::DownloadsChanged => {
                if self.page == "Downloads" {
                    self.load("Downloads".into(), String::new(), Mode::Refresh);
                }
                self.refresh_attention();
            }
            Msg::Reload => self.start(self.wants_playback),
            Msg::RadioDone(extended) => {
                self.radio_inflight = false;
                if self.resume_after_radio && self.wants_playback && extended {
                    self.resume_after_radio = false;
                    if let Ok(text) = self.core.queue_command("{\"op\":\"next\"}".into()) {
                        if parse(&text)["continue"] == true {
                            self.start(true);
                        }
                    }
                }
            }
            Msg::Autoplay(enabled) => {
                if self.autoplay_enabled == enabled {
                    return;
                }
                self.autoplay_enabled = enabled;
                self.autoplay_action.set_state(&enabled.to_variant());
                let _ = self.core.set_autoplay(enabled);
                if enabled {
                    self.request_radio();
                }
            }
            Msg::ShuffleList(density) => {
                if self.kind != "songs" || !self.rows.iter().any(ui::is_song) {
                    self.toast("Open a track list to shuffle it.");
                    return;
                }
                self.wants_playback = true;
                let _ = self.core.record_context(self.play_context().to_string());
                let core = self.core.clone();
                let tx = self.input.clone();
                let songs = json!(self.rows).to_string();
                if density != "off" {
                    self.toast("Preparing shuffle…");
                }
                std::thread::spawn(move || match core.start_shuffle(songs, density) {
                    Ok(text) => tx.emit(Msg::ShuffleReady(parse(&text))),
                    Err(error) => tx.emit(Msg::Error(error.to_string())),
                });
            }
            Msg::ShuffleReady(value) => {
                let snapshot = self
                    .core
                    .queue_command("{}".into())
                    .map(|s| parse(&s))
                    .unwrap_or(Value::Null);
                if value["applied"] == true && value["generation"] == snapshot["generation"] {
                    if let Some(warning) = value["warning"].as_str().filter(|w| !w.is_empty()) {
                        self.toast(warning);
                    }
                    self.start(self.wants_playback);
                }
            }
            Msg::Selecting(on) => {
                self.selecting = on;
                self.selected = if on {
                    self.rows
                        .iter()
                        .filter(|r| ui::is_song(r))
                        .filter_map(|r| r["id"].as_str().map(str::to_owned))
                        .collect()
                } else {
                    HashSet::new()
                };
                self.render(true);
            }
            Msg::ToggleSelected(id) => {
                if !self.selected.remove(&id) {
                    self.selected.insert(id);
                }
                self.render(true);
            }
            Msg::SelectAll => {
                let all: HashSet<String> = self
                    .rows
                    .iter()
                    .filter(|r| ui::is_song(r))
                    .filter_map(|r| r["id"].as_str().map(str::to_owned))
                    .collect();
                self.selected = if self.selected.len() == all.len() {
                    HashSet::new()
                } else {
                    all
                };
                self.render(true);
            }
            Msg::ConnectionWarning(warning) => {
                if warning != self.connection_warning {
                    self.connection_warning = warning;
                    if MANAGEMENT_PAGES.contains(&self.page.as_str()) && !self.loading {
                        self.render(true);
                    }
                }
            }
            Msg::Bus(Bus::Ready) => {
                if let Some(position) = self.pending_seek.take() {
                    if self
                        .player
                        .seek_simple(
                            gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT,
                            gst::ClockTime::from_nseconds((position * 1e9) as u64),
                        )
                        .is_ok()
                    {
                        self.position = position;
                        mpris::seeked(position);
                    }
                }
            }
            Msg::Bus(Bus::Error { offline }) => {
                let uri = self
                    .player
                    .property::<Option<String>>("uri")
                    .unwrap_or_default();
                let audible = self.wants_playback || self.playing;
                let _ = self.player.set_state(gst::State::Null);
                self.failed = true;
                self.pending_seek = None;
                self.playing = false;
                self.wants_playback = false;
                self.resume_after_radio = false;
                // A restored, paused queue fails quietly; playing it again explains why.
                if audible {
                    self.toast(ui::playback_failure(
                        offline || self.offline,
                        self.preview,
                        uri.starts_with("file:"),
                    ));
                }
                mpris::notify(&self.core, &self.player);
            }
            Msg::Bus(Bus::Eos) => {
                if self.wants_playback {
                    if let Ok(text) = self.core.queue_command("{}".into()) {
                        let snapshot = parse(&text);
                        let song = &snapshot["current"];
                        let duration = song["duration"].as_f64().unwrap_or(self.duration);
                        let _ = self.core.clone().playback_progress(
                            snapshot["playbackEpoch"].as_u64().unwrap_or(0),
                            song["id"].as_str().unwrap_or("").into(),
                            duration,
                            duration,
                            true,
                        );
                    }
                }
                let _ = self.player.set_state(gst::State::Paused);
                self.playing = false;
                self.resume_after_radio = self.wants_playback;
                if self.wants_playback {
                    self.request_radio();
                }
            }
            Msg::Tick => {
                self.radio_tick = (self.radio_tick + 1) % 5;
                if self.radio_tick == 0 && !self.loading {
                    // Poll only while something on the page is still changing.
                    let active = match self.page.as_str() {
                        "Jobs" => self.source_rows.iter().any(|r| {
                            ui::job_bucket(r["status"].as_str().unwrap_or("")) == "active"
                        }),
                        "Imports" => self
                            .source_rows
                            .iter()
                            .any(|r| r["pending"].as_u64().unwrap_or(0) > 0),
                        "Settings" => self.source_extra["oauthFlow"].is_object(),
                        _ => false,
                    };
                    if active {
                        self.refresh_page(Mode::Poll);
                    }
                    self.refresh_attention();
                    // Server failures change without the app's help; re-read them about once a minute.
                    self.attention_polls = (self.attention_polls + 1) % 12;
                    if self.attention_polls == 0 {
                        self.check_server_attention();
                    }
                }
                if self.page == "Downloads"
                    && self.radio_tick.is_multiple_of(2)
                    && !self.loading
                    && self.source_rows.iter().any(|r| {
                        ["queued", "downloading"]
                            .contains(&r["download"]["state"].as_str().unwrap_or(""))
                    })
                {
                    self.refresh_page(Mode::Poll);
                }
                if self.wants_playback && self.radio_tick == 0 {
                    self.request_radio();
                }
                self.refresh_now(false);
                if self.radio_tick == 0
                    && self.playing
                    && self.player.current_state() == gst::State::Playing
                {
                    self.save_position(self.position);
                }
                self.record_ui_state(self.loading);
                mpris::notify(&self.core, &self.player);
            }
        }
    }
}
impl App {
    fn record_ui_state(&self, loading: bool) {
        if let Some(path) = std::env::var_os("ZAY_UI_READY_FILE") {
            let mut state = json!({"page":self.page,"loading":loading,"gain":self.player.property::<f64>("volume")});
            if loading {
                let _ = std::fs::write(path, state.to_string());
            } else {
                let window = self.window.clone();
                gtk::glib::timeout_add_local_once(
                    std::time::Duration::from_millis(150),
                    move || {
                        let mut controls = vec![];
                        collect_controls(
                            window.upcast_ref(),
                            window.upcast_ref(),
                            false,
                            &mut controls,
                        );
                        state["controls"] = json!(controls);
                        let _ = std::fs::write(path, state.to_string());
                    },
                );
            }
        }
    }
    fn toast(&self, message: &str) {
        let toast = adw::Toast::new(message);
        toast.set_timeout(3);
        self.toasts.add_toast(toast);
    }
    fn is_search(&self) -> bool {
        SEARCH_PAGES.contains(&self.page.as_str())
    }
    /// Resolved YouTube playlist or album links, whose tracks can be picked in bulk.
    fn resolved(&self) -> bool {
        self.is_search()
            && ["playlist", "album"].contains(&self.extra["kind"].as_str().unwrap_or(""))
    }
    fn page_query(&self, page: &str) -> String {
        if page == "Library" {
            self.library_options.to_string()
        } else {
            String::new()
        }
    }
    fn play_context(&self) -> Value {
        if self.page == "Liked Songs" {
            liked_context()
        } else {
            self.context.clone()
        }
    }
    fn apply_filter(&mut self) {
        self.rows = self.source_rows.clone();
        self.extra = self.source_extra.clone();
        if self.is_search() && self.search_filter != "all" && !self.resolved() {
            let (rows, sections) = ui::filter_search(
                &self.source_rows,
                &self.source_extra["sections"],
                &self.search_filter,
            );
            self.rows = rows;
            if self.extra.is_object() {
                self.extra["sections"] = sections;
            }
        }
    }
    fn begin_loading(&mut self) {
        self.loading = true;
        self.status = String::new();
        self.record_ui_state(true);
        self.art.next_page();
        self.render(false);
    }
    fn push_trail(&mut self) {
        let open = if self.page == "Collection" {
            json!([self.context["kind"], self.context])
        } else {
            Value::Null
        };
        self.trail.push(json!({"page":self.page,"query":self.query,"filter":self.search_filter,"jobFilter":self.job_filter,"open":open}));
        if self.trail.len() > 30 {
            self.trail.remove(0);
        }
    }
    fn load(&mut self, page: String, query: String, mode: Mode) {
        let mode = if page != self.page { Mode::Full } else { mode };
        self.request += 1;
        let request = self.request;
        self.query = query.clone();
        if mode == Mode::Full {
            self.page = page.clone();
            self.header = display_title(&page);
            self.context = Value::Null;
            self.source_rows.clear();
            self.source_extra = Value::Null;
            self.select_sidebar();
            if MANAGEMENT_PAGES.contains(&page.as_str()) {
                self.check_connection();
            }
            self.begin_loading();
        } else if mode == Mode::Refresh {
            self.record_ui_state(true);
        }
        let core = self.core.clone();
        let tx = self.input.clone();
        std::thread::spawn(move || match core.browse(page.clone(), query) {
            Ok(text) => tx.emit(Msg::Loaded(request, page, parse(&text))),
            Err(error) => tx.emit(Msg::LoadError(request, error.to_string())),
        });
    }
    /// Open an album, artist, playlist, genre or YouTube album.
    fn open(&mut self, kind: String, item: Value, mode: Mode) {
        self.request += 1;
        let request = self.request;
        if mode == Mode::Full || self.page != "Collection" {
            self.page = "Collection".into();
            self.header = item["name"]
                .as_str()
                .or(item["title"].as_str())
                .or(item["value"].as_str())
                .unwrap_or("")
                .into();
            self.context = Value::Null;
            self.source_rows.clear();
            self.select_sidebar();
            self.begin_loading();
        } else if mode == Mode::Refresh {
            self.record_ui_state(true);
        }
        let id = item["id"]
            .as_str()
            .or(item["value"].as_str())
            .unwrap_or("")
            .to_owned();
        let core = self.core.clone();
        let tx = self.input.clone();
        std::thread::spawn(move || match core.open_collection(kind, id) {
            Ok(text) => tx.emit(Msg::Loaded(request, "Collection".into(), parse(&text))),
            Err(error) => tx.emit(Msg::LoadError(request, error.to_string())),
        });
    }
    /// Reload what is on screen, keeping the query, filters and scroll position.
    fn refresh_page(&mut self, mode: Mode) {
        if self.page == "Collection" {
            if let Some(kind) = self.context["kind"].as_str().map(str::to_owned) {
                self.open(kind, self.context.clone(), mode);
            }
        } else {
            self.load(self.page.clone(), self.query.clone(), mode);
        }
    }
    /// Imports and new downloads need a working YouTube Music session; say so where they are managed.
    fn check_connection(&self) {
        let core = self.core.clone();
        let tx = self.input.clone();
        std::thread::spawn(move || {
            if let Ok(text) = core.management_status() {
                tx.emit(Msg::ConnectionWarning(ui::youtube_attention(
                    &parse(&text)["youtube"],
                )));
            }
        });
    }
    /// Failed downloads, unsent requests and failed server jobs, shown as the Jobs badge.
    fn refresh_attention(&mut self) {
        let total = self
            .core
            .attention_summary(false)
            .map(|t| parse(&t)["total"].as_u64().unwrap_or(0) as usize)
            .unwrap_or(0);
        if total != self.attention {
            self.attention = total;
            self.jobs_badge.set_label(&total.to_string());
            self.jobs_badge.set_visible(total > 0);
            self.jobs_badge
                .set_tooltip_text(Some(&format!("{total} need attention")));
        }
    }
    /// Re-reads failed server jobs off the UI thread, so the badge never waits for Jobs to open.
    fn check_server_attention(&self) {
        let core = self.core.clone();
        let tx = self.input.clone();
        std::thread::spawn(move || {
            let _ = core.attention_summary(true);
            tx.emit(Msg::AttentionChecked);
        });
    }
    fn refresh_sidebar_playlists(&self) {
        let core = self.core.clone();
        let tx = self.input.clone();
        std::thread::spawn(move || {
            // The core orders Your Library playlists by recent play; Liked Songs is pinned elsewhere.
            let recent = core
                .browse(
                    "Library".into(),
                    json!({"filter":"playlists","sort":"recent","text":""}).to_string(),
                )
                .map(|text| {
                    songs(&parse(&text)["rows"])
                        .into_iter()
                        .filter(|r| r["browseKind"] == "playlists")
                        .collect::<Vec<_>>()
                });
            let rows = match recent {
                Ok(rows) => rows,
                Err(_) => match core.browse("Playlists".into(), String::new()) {
                    Ok(text) => songs(&parse(&text)["rows"]),
                    Err(_) => return,
                },
            };
            tx.emit(Msg::SidebarPlaylists(rows));
        });
    }
    fn build_sidebar(&self) {
        while let Some(child) = self.sidebar.first_child() {
            self.sidebar.remove(&child);
        }
        if let Some(parent) = self
            .jobs_badge
            .parent()
            .and_then(|p| p.downcast::<gtk::Box>().ok())
        {
            parent.remove(&self.jobs_badge);
        }
        let heading = |text: &str| {
            let row = gtk::ListBoxRow::new();
            row.set_selectable(false);
            row.set_activatable(false);
            row.set_child(Some(&ui::label(&text.to_uppercase(), &["sidebar-heading"])));
            row
        };
        for (page, icon) in NAVIGATION {
            let Some(icon) = icon else {
                self.sidebar.append(&heading(page));
                continue;
            };
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            line.append(&gtk::Image::from_icon_name(icon));
            line.append(&ui::label(&display_title(page), &[]));
            if *page == "Jobs" {
                line.append(&self.jobs_badge);
            }
            let row = gtk::ListBoxRow::new();
            row.set_widget_name(&format!("page:{page}"));
            row.set_child(Some(&line));
            self.sidebar.append(&row);
        }
        if !self.playlists.is_empty() {
            self.sidebar.append(&heading("Playlists"));
        }
        for (index, playlist) in self.playlists.iter().enumerate() {
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            line.append(&self.art.tile(playlist, Art::Playlist, 32, false));
            line.append(&ui::label(
                playlist["name"].as_str().unwrap_or("Playlist"),
                &[],
            ));
            let row = gtk::ListBoxRow::new();
            row.set_widget_name(&format!("playlist:{index}"));
            row.set_child(Some(&line));
            self.sidebar.append(&row);
        }
        self.select_sidebar();
    }
    fn select_sidebar(&self) {
        let target = if self.page == "Omnibar" {
            "Search"
        } else {
            &self.page
        };
        let mut child = self.sidebar.first_child();
        while let Some(widget) = child {
            if widget.widget_name() == format!("page:{target}") {
                if let Ok(row) = widget.clone().downcast::<gtk::ListBoxRow>() {
                    self.sidebar.select_row(Some(&row));
                    return;
                }
            }
            child = widget.next_sibling();
        }
        if self.page != "Collection" {
            self.sidebar.unselect_all();
        }
    }
    /// The playback clock, or the restored position until GStreamer has prerolled.
    fn clock(&self) -> f64 {
        if let Some(position) = self.pending_seek {
            return position;
        }
        self.player
            .query_position::<gst::ClockTime>()
            .map(|t| t.seconds_f64())
            .unwrap_or(self.position)
    }
    /// The core ignores reports from an older epoch or another song.
    fn save_position(&self, position: f64) {
        if self.current_id.is_empty() {
            return;
        }
        if let Ok(text) = self.core.queue_command("{}".into()) {
            let _ = self.core.save_playback_position(
                parse(&text)["playbackEpoch"].as_u64().unwrap_or(0),
                self.current_id.clone(),
                position,
            );
        }
    }
    fn seek_to(&mut self, seconds: f64) {
        let target = if self.duration > 0. {
            seconds.clamp(0., self.duration)
        } else {
            seconds.max(0.)
        };
        if self.pending_seek.is_some() {
            // Not prerolled yet: move the restore target instead.
            self.pending_seek = Some(target);
        } else if self
            .player
            .seek_simple(
                gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT,
                gst::ClockTime::from_nseconds((target * 1e9) as u64),
            )
            .is_err()
        {
            return;
        }
        self.position = target;
        self.save_position(target);
        mpris::seeked(target);
    }
    /// Sync the player bar with the shared queue and GStreamer clock.
    fn refresh_now(&mut self, force: bool) {
        let Ok(text) = self.core.queue_command("{}".into()) else {
            return;
        };
        let snapshot = parse(&text);
        let current = snapshot["current"].clone();
        self.shuffle = snapshot["state"]["shuffle"] == true;
        self.repeat = snapshot["state"]["repeat"].as_str().unwrap_or("off").into();
        let id = current["id"].as_str().unwrap_or("").to_owned();
        let new_song = force || id != self.current_id;
        if new_song || current != self.current {
            self.title = current["title"]
                .as_str()
                .unwrap_or("Nothing playing")
                .into();
            self.artist = current["artist"].as_str().unwrap_or("").into();
            self.starred = ui::present(&current["starred"]);
            self.preview = id.starts_with("yt:");
            self.current_id = id;
            self.current = current.clone();
            if new_song {
                mark_playing(
                    self.body.upcast_ref(),
                    &format!("track:{}", self.current_id),
                );
                while let Some(child) = self.now_art.first_child() {
                    self.now_art.remove(&child);
                }
                self.now_art
                    .append(&self.art.tile(&current, Art::Song, 56, false));
                if !current.is_null() {
                    // Desktop media controls read a cached artwork file; warm it off the UI thread.
                    let core = self.core.clone();
                    std::thread::spawn(move || {
                        let _ = core.artwork(current.to_string(), 300);
                    });
                }
            }
            if self.queue_dialog.is_some() {
                self.render_queue();
            }
        }
        let radio = snapshot["radioError"].as_str().unwrap_or("").to_owned();
        if radio != self.last_radio_error {
            if !radio.is_empty() {
                self.toast(&radio);
            }
            self.last_radio_error = radio;
        }
        self.position = self.clock();
        self.duration = self
            .player
            .query_duration::<gst::ClockTime>()
            .map(|t| t.seconds_f64())
            .or(self.current["duration"].as_f64())
            .unwrap_or(0.);
        let _ = self.core.clone().playback_progress(
            snapshot["playbackEpoch"].as_u64().unwrap_or(0),
            self.current_id.clone(),
            self.position,
            self.duration,
            self.playing && self.player.current_state() == gst::State::Playing,
        );
    }
    fn render(&mut self, keep_scroll: bool) {
        let adjustment = self.scroll.vadjustment();
        let value = adjustment.value();
        while let Some(child) = self.body.first_child() {
            self.body.remove(&child);
        }
        self.build_page_bar();
        self.fill_body();
        if keep_scroll && !self.loading {
            ui::restore_scroll(&adjustment, value);
        } else {
            adjustment.set_value(0.);
        }
    }
    /// Controls that stay put above the scrolling page: search filters and Library sorting.
    fn build_page_bar(&mut self) {
        let key = if self.is_search() && !self.query.trim().is_empty() && !self.resolved() {
            format!("search:{}", self.search_filter)
        } else if self.page == "Library" {
            "library".into()
        } else {
            String::new()
        };
        if key == self.page_bar_key {
            return;
        }
        self.page_bar_key = key.clone();
        while let Some(child) = self.page_bar.first_child() {
            self.page_bar.remove(&child);
        }
        self.page_bar.set_visible(!key.is_empty());
        let tx = self.input.clone();
        let pair = |a: &str, b: &str| (a.to_string(), b.to_string());
        if key.starts_with("search:") {
            let filters = ui::toggle_group(
                &[
                    pair("all", "All"),
                    pair("songs", "Songs"),
                    pair("albums", "Albums"),
                    pair("library", "Library"),
                ],
                &self.search_filter,
                move |f| tx.emit(Msg::SearchFilter(f)),
            );
            filters.update_property(&[gtk::accessible::Property::Label("Show")]);
            self.page_bar.append(&filters);
        } else if key == "library" {
            let t = tx.clone();
            let filters = ui::toggle_group(
                &[
                    pair("all", "All"),
                    pair("playlists", "Playlists"),
                    pair("albums", "Albums"),
                    pair("artists", "Artists"),
                ],
                self.library_options["filter"].as_str().unwrap_or("all"),
                move |f| t.emit(Msg::LibraryOptions("filter".into(), f)),
            );
            self.page_bar.append(&filters);
            let sorts = ["recent", "added", "name"];
            let sort =
                gtk::DropDown::from_strings(&["Recently played", "Recently added", "Alphabetical"]);
            sort.set_selected(
                sorts
                    .iter()
                    .position(|s| self.library_options["sort"] == *s)
                    .unwrap_or(0) as u32,
            );
            sort.set_tooltip_text(Some("Sort Your Library"));
            let t = tx.clone();
            sort.connect_selected_notify(move |d| {
                t.emit(Msg::LibraryOptions(
                    "sort".into(),
                    sorts[(d.selected() as usize).min(2)].into(),
                ))
            });
            self.page_bar.append(&sort);
            let text = gtk::SearchEntry::new();
            text.set_placeholder_text(Some("Search Your Library"));
            text.set_text(self.library_options["text"].as_str().unwrap_or(""));
            text.set_search_delay(250);
            text.set_hexpand(true);
            text.set_halign(gtk::Align::End);
            text.set_width_request(260);
            text.connect_search_changed(move |e| {
                tx.emit(Msg::LibraryOptions("text".into(), e.text().to_string()))
            });
            self.page_bar.append(&text);
        }
    }
    fn fill_body(&self) {
        if let Some(hero) = self.hero() {
            self.body.append(&hero);
        }
        if self.loading {
            let spinner = gtk::Spinner::new();
            spinner.set_spinning(true);
            spinner.set_size_request(32, 32);
            spinner.set_margin_top(48);
            self.body.append(&spinner);
            return;
        }
        if let Some(actions) = self.actions() {
            self.body.append(&actions);
        }
        if self.page == "Home" {
            self.shortcuts();
        }
        if self.is_search() {
            self.search_extras();
        }
        if MANAGEMENT_PAGES.contains(&self.page.as_str()) && !self.connection_warning.is_empty() {
            let banner = adw::Banner::builder()
                .title(self.connection_warning.as_str())
                .button_label("Open Settings")
                .revealed(true)
                .build();
            let tx = self.input.clone();
            banner.connect_button_clicked(move |_| tx.emit(Msg::Page("Settings".into())));
            self.body.append(&banner);
        }
        if self.kind == "settings" || self.page == "Settings" {
            if !self.status.is_empty() {
                self.body.append(&ui::label(
                    &format!("Server status unavailable: {}", self.status),
                    &["dim"],
                ));
            }
            self.body.append(&manage::settings(
                self.core.clone(),
                self.input.clone(),
                &self.extra,
                &self.window,
                self.shuffle,
                &self.repeat,
            ));
            return;
        }
        if self.kind == "jobs" && self.status.is_empty() {
            self.job_filters();
            let shown: Vec<Value> = self
                .rows
                .iter()
                .filter(|r| {
                    self.job_filter == "all"
                        || ui::job_bucket(r["status"].as_str().unwrap_or("")) == self.job_filter
                })
                .cloned()
                .collect();
            if shown.is_empty() {
                self.body.append(&ui::label(
                    if self.rows.is_empty() {
                        "No downloads or imports in progress."
                    } else {
                        "No jobs match this filter."
                    },
                    &["dim"],
                ));
                return;
            }
            self.body.append(&manage::rows(
                self.core.clone(),
                self.input.clone(),
                &self.kind,
                &shown,
                &self.window,
            ));
            return;
        }
        if ["imports", "youtubePlaylists"].contains(&self.kind.as_str()) && self.status.is_empty() {
            self.body.append(&manage::rows(
                self.core.clone(),
                self.input.clone(),
                &self.kind,
                &self.rows,
                &self.window,
            ));
            return;
        }
        if !self.status.is_empty() {
            let page = adw::StatusPage::builder()
                .icon_name("network-offline-symbolic")
                .title("Couldn't load this page")
                .description(self.status.as_str())
                .build();
            self.body.append(&page);
            return;
        }
        if self.rows.is_empty() {
            let quoted = format!("\u{201C}{}\u{201D}", self.query.trim());
            let filtered = format!(
                "No {} for {quoted}",
                match self.search_filter.as_str() {
                    "songs" => "songs",
                    "albums" => "albums",
                    _ => "library results",
                }
            );
            let (icon, title, description) = match self.page.as_str() {
                _ if self.is_search() && !self.query.trim().is_empty() => {
                    if self.search_filter != "all" && !self.source_rows.is_empty() {
                        (
                            "system-search-symbolic",
                            filtered.as_str(),
                            "Try another filter.",
                        )
                    } else {
                        (
                            "system-search-symbolic",
                            "No results",
                            "Check the spelling or try different words.",
                        )
                    }
                }
                "Search" | "Omnibar" => (
                    "system-search-symbolic",
                    "Search your music",
                    "Find songs, albums and artists, or paste a YouTube link.",
                ),
                "New music" => (
                    "star-new-symbolic",
                    "Find new music",
                    "Search YouTube Music, then add songs to your library.",
                ),
                "Library" => (
                    "view-app-grid-symbolic",
                    if self.library_options["text"]
                        .as_str()
                        .unwrap_or("")
                        .is_empty()
                    {
                        "Nothing saved yet"
                    } else {
                        "No matches in Your Library"
                    },
                    "Saved playlists, albums and artists appear here.",
                ),
                "Downloads" => (
                    "folder-download-symbolic",
                    "No downloads yet",
                    "Downloaded songs play without a connection.",
                ),
                "Queue" => (
                    "view-list-ordered-symbolic",
                    "Your queue is empty",
                    "Play a song or add one to the queue.",
                ),
                _ => ("folder-music-symbolic", "Nothing here yet", ""),
            };
            let page = adw::StatusPage::builder()
                .icon_name(icon)
                .title(title)
                .description(description)
                .build();
            self.body.append(&page);
            return;
        }
        if self.kind != "songs" {
            self.body.append(&self.collection_list());
            return;
        }
        let sections: Vec<(String, usize, usize)> = if self.page == "Queue" {
            let priority = self
                .rows
                .iter()
                .filter(|r| r["queueLane"] == "priority")
                .count();
            [
                ("Next in queue", 0, priority),
                ("Main queue", priority, self.rows.len() - priority),
            ]
            .into_iter()
            .filter(|s| s.2 > 0)
            .map(|(t, s, c)| (t.to_string(), s, c))
            .collect()
        } else {
            songs(&self.extra["sections"])
                .iter()
                .map(|s| {
                    (
                        s["title"].as_str().unwrap_or("").to_string(),
                        s["start"].as_u64().unwrap_or(0) as usize,
                        s["count"].as_u64().unwrap_or(0) as usize,
                    )
                })
                .filter(|s| s.1 + s.2 <= self.rows.len())
                .collect()
        };
        if sections.is_empty() {
            self.body.append(&self.track_list(0, self.rows.len()));
        }
        for (title, start, count) in sections {
            if !title.is_empty() {
                self.body.append(&ui::label(&title, &["section-title"]));
            }
            self.body.append(&self.track_list(start, count));
        }
    }
    /// Spotify-style Home shortcuts: up to eight recent collections, each with its own Play.
    fn shortcuts(&self) {
        let contexts: Vec<Value> = songs(&self.extra["contexts"]).into_iter().take(8).collect();
        if contexts.is_empty() {
            return;
        }
        let flow = gtk::FlowBox::new();
        flow.set_selection_mode(gtk::SelectionMode::None);
        flow.set_homogeneous(true);
        flow.set_max_children_per_line(4);
        flow.set_min_children_per_line(2);
        flow.set_column_spacing(8);
        flow.set_row_spacing(8);
        for item in contexts {
            let kind = item["kind"].as_str().unwrap_or("").to_owned();
            let name = item["name"]
                .as_str()
                .or(item["title"].as_str())
                .unwrap_or("Collection")
                .to_owned();
            let card = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            card.add_css_class("shortcut");
            card.set_overflow(gtk::Overflow::Hidden);
            let open = gtk::Button::new();
            open.add_css_class("flat");
            open.set_hexpand(true);
            open.set_tooltip_text(Some(&name));
            let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            let art = if kind == "liked" {
                self.art
                    .tile(&json!({"name":"Liked Songs"}), Art::Liked, 56, true)
            } else {
                self.art.tile(&item, Art::for_kind(&kind), 56, true)
            };
            content.append(&art);
            let label = ui::label(&name, &["track-title"]);
            label.set_wrap(true);
            label.set_lines(2);
            label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
            content.append(&label);
            open.set_child(Some(&content));
            let tx = self.input.clone();
            let target = item.clone();
            open.connect_clicked(move |_| tx.emit(Msg::Action(target.clone(), "contexts".into())));
            card.append(&open);
            let play = ui::icon_button("media-playback-start-symbolic", &format!("Play {name}"));
            play.remove_css_class("flat");
            play.add_css_class("shortcut-play");
            play.set_margin_start(6);
            play.set_margin_end(10);
            let tx = self.input.clone();
            play.connect_clicked(move |_| tx.emit(Msg::PlayContext(item.clone())));
            card.append(&play);
            flow.insert(&card, -1);
        }
        self.body.append(&flow);
    }
    /// Resolved-link selection, a separate "Did you mean", and suggestions only for empty results.
    fn search_extras(&self) {
        if self.resolved() {
            let tracks: Vec<Value> = self
                .rows
                .iter()
                .filter(|r| ui::is_song(r))
                .cloned()
                .collect();
            let header = gtk::Box::new(gtk::Orientation::Vertical, 6);
            let title = self.extra["title"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or(if self.extra["kind"] == "album" {
                    "YouTube album"
                } else {
                    "YouTube playlist"
                });
            header.append(&ui::label(title, &["heading"]));
            header.append(&ui::label(
                &format!("{} from YouTube", ui::count(tracks.len(), "track")),
                &["dim"],
            ));
            let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            buttons.set_margin_top(4);
            let tx = self.input.clone();
            if self.selecting {
                let all = self.selected.len() == tracks.len();
                let toggle =
                    gtk::Button::with_label(if all { "Select none" } else { "Select all" });
                let t = tx.clone();
                toggle.connect_clicked(move |_| t.emit(Msg::SelectAll));
                buttons.append(&toggle);
                let chosen: Vec<Value> = tracks
                    .iter()
                    .filter(|r| {
                        r["id"]
                            .as_str()
                            .is_some_and(|id| self.selected.contains(id))
                    })
                    .cloned()
                    .collect();
                let add = gtk::Button::with_label(&format!("Add {} to playlist", chosen.len()));
                add.add_css_class("suggested-action");
                add.set_sensitive(!chosen.is_empty());
                let t = tx.clone();
                add.connect_clicked(move |_| {
                    t.emit(Msg::PickPlaylist(Selection::Songs(chosen.clone())))
                });
                buttons.append(&add);
                let done = gtk::Button::with_label("Done");
                done.connect_clicked(move |_| tx.emit(Msg::Selecting(false)));
                buttons.append(&done);
            } else {
                let select = gtk::Button::new();
                select.set_child(Some(
                    &adw::ButtonContent::builder()
                        .icon_name("checkbox-checked-symbolic")
                        .label("Select songs")
                        .build(),
                ));
                let t = tx.clone();
                select.connect_clicked(move |_| t.emit(Msg::Selecting(true)));
                buttons.append(&select);
                let add = gtk::Button::new();
                add.set_child(Some(
                    &adw::ButtonContent::builder()
                        .icon_name("list-add-symbolic")
                        .label("Add all to playlist")
                        .build(),
                ));
                add.connect_clicked(move |_| {
                    tx.emit(Msg::PickPlaylist(Selection::Songs(tracks.clone())))
                });
                buttons.append(&add);
            }
            buttons.set_sensitive(self.rows.iter().any(ui::is_song));
            header.append(&buttons);
            self.body.append(&header);
            return;
        }
        let correction = match &self.extra["correction"] {
            Value::String(text) => text.trim().to_owned(),
            Value::Object(pair) => [pair.get("artist"), pair.get("title")]
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" "),
            _ => String::new(),
        };
        if !correction.is_empty() && correction.to_lowercase() != self.query.trim().to_lowercase() {
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 2);
            line.add_css_class("correction");
            line.append(&gtk::Label::new(Some("Did you mean")));
            let fix = gtk::Button::with_label(&correction);
            fix.add_css_class("flat");
            let tx = self.input.clone();
            fix.connect_clicked(move |_| {
                tx.emit(Msg::Action(
                    json!({"query":correction}),
                    "suggestion".into(),
                ))
            });
            line.append(&fix);
            line.append(&gtk::Label::new(Some("?")));
            self.body.append(&line);
        }
        if self.source_rows.is_empty() && !self.query.trim().is_empty() {
            let chips = gtk::FlowBox::new();
            chips.set_selection_mode(gtk::SelectionMode::None);
            for suggestion in songs(&self.extra["suggestions"])
                .into_iter()
                .filter_map(|s| s.as_str().filter(|s| !s.is_empty()).map(str::to_owned))
            {
                let b = gtk::Button::with_label(&suggestion);
                b.add_css_class("pill");
                let tx = self.input.clone();
                b.connect_clicked(move |_| {
                    tx.emit(Msg::Action(
                        json!({"query":suggestion}),
                        "suggestion".into(),
                    ))
                });
                chips.insert(&b, -1);
            }
            if chips.first_child().is_some() {
                self.body.append(&chips);
            }
        }
    }
    /// Job counts double as filters: choose one to show only those jobs, again for all.
    fn job_filters(&self) {
        if self.rows.is_empty() {
            return;
        }
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        for (id, title) in [
            ("active", "In progress"),
            ("done", "Completed"),
            ("attention", "Needs attention"),
        ] {
            let n = self
                .rows
                .iter()
                .filter(|r| ui::job_bucket(r["status"].as_str().unwrap_or("")) == id)
                .count();
            let chip = gtk::ToggleButton::with_label(&format!("{title} · {n}"));
            chip.add_css_class("pill");
            chip.set_active(self.job_filter == id);
            if id == "attention" && n > 0 {
                chip.add_css_class("warning-text");
            }
            let tx = self.input.clone();
            chip.connect_clicked(move |_| tx.emit(Msg::JobFilter(id.into())));
            line.append(&chip);
        }
        self.body.append(&line);
    }
    /// Spotify-style context header: cover, kind, large title and real metadata only.
    fn hero(&self) -> Option<gtk::Widget> {
        let context_kind = self.context["kind"].as_str();
        let (art, kind_label) = match (context_kind, self.page.as_str()) {
            (Some(kind), _) => (
                Some(Art::for_kind(kind)),
                match kind {
                    "playlists" => "Playlist",
                    "albums" => "Album",
                    "artists" => "Artist",
                    "genres" => "Genre",
                    "newAlbums" => "Album preview",
                    _ => "",
                },
            ),
            (None, "Liked Songs") => (Some(Art::Liked), "Playlist"),
            _ => (None, ""),
        };
        let title = if self.page == "Home" {
            greeting()
        } else {
            self.header.clone()
        };
        let mut meta = vec![];
        if !self.loading {
            for key in ["owner", "artist"] {
                if let Some(value) = self.context[key]
                    .as_str()
                    .filter(|v| !v.is_empty() && *v != title)
                {
                    meta.push(value.to_owned());
                }
            }
            if let Some(year) = self.context["year"].as_u64().filter(|y| *y > 0) {
                meta.push(year.to_string());
            }
            let noun = match self.kind.as_str() {
                "albums" => "album",
                "artists" => "artist",
                "playlists" => "playlist",
                "genres" => "genre",
                "jobs" => "job",
                "imports" => "import",
                _ => "song",
            };
            if self.status.is_empty() && self.kind != "settings" && self.page != "Home" {
                if (self.is_search() && !self.query.trim().is_empty() && !self.resolved())
                    || self.kind == "library"
                {
                    meta.push(ui::mixed_count(&self.rows));
                } else {
                    let mut total = ui::count(self.rows.len(), noun);
                    let duration = self.context["duration"].as_f64().unwrap_or(0.);
                    if self.kind == "songs" && duration > 0. {
                        total.push_str(&format!(", {}", ui::total_duration(duration)));
                    }
                    meta.push(total);
                }
            }
            if self.page == "Downloads" {
                meta.push(format!(
                    "{} stored",
                    glib::format_size(self.extra["readyBytes"].as_u64().unwrap_or(0))
                ));
            }
        }
        let text = gtk::Box::new(gtk::Orientation::Vertical, 6);
        text.set_valign(gtk::Align::End);
        text.set_hexpand(true);
        if !kind_label.is_empty() {
            text.append(&ui::label(kind_label, &["hero-kind"]));
        }
        let heading = ui::label(&title, &["hero-title"]);
        heading.set_wrap(true);
        heading.set_lines(2);
        heading.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        text.append(&heading);
        if !meta.is_empty() {
            text.append(&ui::label(&meta.join(" · "), &["hero-meta"]));
        }
        let hero = gtk::Box::new(gtk::Orientation::Horizontal, 24);
        hero.add_css_class("hero");
        if let Some(art) = art {
            let source = if art == Art::Liked {
                json!({"name":"Liked Songs"})
            } else {
                self.context.clone()
            };
            hero.append(&self.art.tile(&source, art, 192, true));
        } else {
            heading.remove_css_class("hero-title");
            heading.add_css_class("title-1");
        }
        hero.append(&text);
        Some(hero.upcast())
    }
    /// Play / shuffle / download / save, shown under the header like a music app.
    fn actions(&self) -> Option<gtk::Widget> {
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        bar.set_margin_top(4);
        let tx = self.input.clone();
        let context_kind = self.context["kind"].as_str().unwrap_or("").to_owned();
        let saved = ui::present(&self.context["starred"]);
        let songs_shown = self.kind == "songs"
            && self.rows.iter().any(ui::is_song)
            && self.page != "Queue"
            && !self.resolved();
        if songs_shown {
            let play = gtk::Button::from_icon_name("media-playback-start-symbolic");
            play.add_css_class("circular");
            play.add_css_class("play-hero");
            play.set_tooltip_text(Some("Play"));
            play.update_property(&[gtk::accessible::Property::Label("Play")]);
            let t = tx.clone();
            play.connect_clicked(move |_| t.emit(Msg::PlayCollection));
            bar.append(&play);
            let shuffle = ui::icon_button("media-playlist-shuffle-symbolic", "Shuffle play");
            let t = tx.clone();
            shuffle.connect_clicked(move |_| t.emit(Msg::ShuffleList("off".into())));
            bar.append(&shuffle);
            let t = tx.clone();
            let fresh = ui::action_menu(
                &[vec![
                    ("Shuffle with a few new songs".into(), "sparse".into()),
                    ("Shuffle with more new songs".into(), "dense".into()),
                ]],
                "fresh",
                move |density| t.emit(Msg::ShuffleList(density)),
            );
            fresh.set_icon_name("star-new-symbolic");
            fresh.set_tooltip_text(Some("Fresh shuffle"));
            bar.append(&fresh);
            if context_kind != "newAlbums" {
                let download = ui::icon_button("folder-download-symbolic", "Download all");
                download.set_sensitive(self.rows.iter().any(ui::is_library_song));
                let t = tx.clone();
                download.connect_clicked(move |_| t.emit(Msg::DownloadAll));
                bar.append(&download);
            }
        }
        let pill = |label: &str, tooltip: &str, op: &'static str| {
            let b = gtk::Button::with_label(label);
            b.add_css_class("pill");
            b.set_tooltip_text(Some(tooltip));
            let t = tx.clone();
            let item = self.context.clone();
            b.connect_clicked(move |_| t.emit(Msg::Action(item.clone(), op.into())));
            b
        };
        match context_kind.as_str() {
            "albums" => bar.append(&if saved {
                pill("Saved", "Remove from Your Library", "collectionUnsave")
            } else {
                pill(
                    "Save to Your Library",
                    "Save to Your Library",
                    "collectionSave",
                )
            }),
            "artists" => bar.append(&if saved {
                pill("Following", "Unfollow", "collectionUnsave")
            } else {
                pill(
                    "Follow",
                    "Adds this artist to Your Library",
                    "collectionSave",
                )
            }),
            "newAlbums" => {
                bar.append(&pill(
                    "Add to library",
                    "Add album to library",
                    "acquireAlbum",
                ));
                let add = ui::icon_button("list-add-symbolic", "Add album to playlist");
                let t = tx.clone();
                let item = self.context.clone();
                add.connect_clicked(move |_| {
                    t.emit(Msg::PickPlaylist(Selection::Album(item.clone())))
                });
                bar.append(&add);
            }
            _ => {}
        }
        if self.page == "Queue" {
            let switch = gtk::Switch::new();
            switch.set_active(self.autoplay_enabled);
            switch.set_valign(gtk::Align::Center);
            let t = tx.clone();
            switch.connect_active_notify(move |s| t.emit(Msg::Autoplay(s.is_active())));
            let label = ui::label("Autoplay similar songs when the queue ends", &["dim"]);
            label.set_mnemonic_widget(Some(&switch));
            switch.update_property(&[gtk::accessible::Property::Label("Autoplay")]);
            bar.append(&switch);
            bar.append(&label);
        }
        if self.page == "Playlists" {
            let create = gtk::Button::new();
            create.set_child(Some(
                &adw::ButtonContent::builder()
                    .icon_name("list-add-symbolic")
                    .label("New playlist")
                    .build(),
            ));
            create.add_css_class("pill");
            let t = tx.clone();
            create.connect_clicked(move |_| t.emit(Msg::PlaylistEdit(None, None)));
            bar.append(&create);
        }
        let mut more = vec![];
        if context_kind == "playlists" {
            more.push(vec![
                ("Rename playlist".to_string(), "rename".to_string()),
                ("Delete playlist".into(), "delete".into()),
            ]);
            more.push(vec![
                ("New playlist".into(), "new".into()),
                ("All playlists".into(), "all".into()),
            ]);
        }
        if self.page == "Downloads" && !self.rows.is_empty() {
            more.push(vec![("Remove all downloads".into(), "clear".into())]);
        }
        if !more.is_empty() {
            let t = tx.clone();
            let menu = ui::action_menu(&more, "page", move |op| {
                t.emit(match op.as_str() {
                    "rename" => Msg::PlaylistEdit(Some(Value::Null), None),
                    "delete" => Msg::PlaylistDelete(Value::Null),
                    "new" => Msg::PlaylistEdit(None, None),
                    "all" => Msg::Page("Playlists".into()),
                    _ => Msg::AskClearDownloads,
                })
            });
            menu.set_tooltip_text(Some("More options"));
            bar.append(&menu);
        }
        (bar.first_child().is_some()).then(|| bar.upcast())
    }
    fn track_list(&self, start: usize, count: usize) -> gtk::ListBox {
        let list = gtk::ListBox::new();
        list.add_css_class("track-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        let rows: Vec<Value> = self.rows[start..start + count].to_vec();
        let album = self.context["kind"] == "albums";
        for (offset, row) in rows.iter().enumerate() {
            list.append(&self.track_row(row, offset, album));
        }
        let tx = self.input.clone();
        let kind = self.kind.clone();
        let selecting = self.selecting && self.resolved();
        list.connect_row_activated(move |_, row| {
            if let Some(song) = rows.get(row.index() as usize) {
                tx.emit(if selecting && ui::is_song(song) {
                    Msg::ToggleSelected(song["id"].as_str().unwrap_or("").into())
                } else if let Some(kind) = song["browseKind"].as_str().filter(|k| *k != "songs") {
                    Msg::Action(song.clone(), kind.into())
                } else if song["queueLane"].is_string() {
                    Msg::Action(song.clone(), "queueJump".into())
                } else if kind != "songs" {
                    Msg::Action(song.clone(), kind.clone())
                } else {
                    Msg::PlayAt(start + row.index() as usize)
                });
            }
        });
        list
    }
    /// Secondary text for a mixed row, as Spotify lists it.
    fn subtitle(row: &Value, kind: &str) -> String {
        let count = row["songCount"]
            .as_u64()
            .map(|n| ui::count(n as usize, "song"));
        let parts: Vec<String> = match kind {
            "albums" | "newAlbums" => vec![
                "Album".into(),
                row["artist"].as_str().unwrap_or("").into(),
                row["year"]
                    .as_u64()
                    .filter(|y| *y > 0)
                    .map(|y| y.to_string())
                    .unwrap_or_default(),
            ],
            "artists" => vec![
                "Artist".into(),
                row["albumCount"]
                    .as_u64()
                    .filter(|n| *n > 0)
                    .map(|n| ui::count(n as usize, "album"))
                    .unwrap_or_default(),
            ],
            "playlists" | "liked" => vec!["Playlist".into(), count.unwrap_or_default()],
            "genres" => vec![count.unwrap_or_else(|| "Genre".into())],
            _ => ["status", "state", "artist"]
                .iter()
                .filter_map(|k| row[*k].as_str().map(str::to_owned))
                .collect(),
        };
        parts
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" · ")
    }
    /// Menus use the item's real saved state: Save/Remove for albums, Follow/Unfollow for artists.
    fn collection_menu(&self, row: &Value, kind: &str) -> Vec<Vec<(String, String)>> {
        let pair = |a: &str, b: &str| (a.to_string(), b.to_string());
        let listed_saved = self.page == "Library"
            || (kind == "albums" && self.page == "Albums")
            || (kind == "artists" && self.page == "Artists");
        let saved = ui::present(&row["starred"]) || listed_saved;
        let toggle = if saved {
            "collectionUnsave"
        } else {
            "collectionSave"
        };
        match kind {
            "albums" => vec![
                vec![pair(
                    if saved {
                        "Remove from Your Library"
                    } else {
                        "Save to Your Library"
                    },
                    toggle,
                )],
                vec![pair("Download album", "downloadCollection:albums")],
            ],
            "artists" => vec![vec![pair(
                if saved { "Unfollow" } else { "Follow" },
                toggle,
            )]],
            "newAlbums" => vec![vec![
                pair("Add album to library", "acquireAlbum"),
                pair("Add album to playlist…", "albumPlaylist"),
            ]],
            "playlists" => {
                let mut sections = vec![vec![pair(
                    "Download playlist",
                    "downloadCollection:playlists",
                )]];
                if ["playlists", "library"].contains(&self.kind.as_str()) {
                    sections.push(vec![
                        pair("Rename playlist", "renamePlaylist"),
                        pair("Delete playlist", "deletePlaylist"),
                    ]);
                }
                sections
            }
            _ => vec![],
        }
    }
    fn song_menu(&self, row: &Value) -> Vec<Vec<(String, String)>> {
        let pair = |a: &str, b: &str| (a.to_string(), b.to_string());
        let id = row["id"].as_str().unwrap_or("");
        let preview = id.starts_with("yt:");
        let library = ui::is_library_song(row);
        let mut queue = vec![];
        if self.kind == "songs" {
            queue = vec![
                pair("Play next", "playNext"),
                pair("Play last in priority queue", "playLast"),
                pair("Add to queue", "append"),
            ];
        }
        let mut navigate = vec![];
        if ui::album_link(row).is_some() {
            navigate.push(pair("Go to album", "goAlbum"));
        }
        if ui::artist_link(row).is_some() {
            navigate.push(pair("Go to artist", "goArtist"));
        }
        let mut saving = vec![pair("Add to playlist", "pickPlaylist")];
        if preview {
            saving.push(pair("Add to library", "acquire"));
        } else if library {
            let liked = ui::present(&row["starred"]) || self.page == "Liked Songs";
            saving.push(if liked {
                pair("Remove from Liked Songs", "unlike")
            } else {
                pair("Add to Liked Songs", "like")
            });
            saving.push(pair("Edit tags…", "editTags"));
        }
        let mut offline = vec![];
        if library {
            if row["download"].is_object() {
                let state = row["download"]["state"].as_str().unwrap_or("");
                if state == "queued" || state == "downloading" {
                    offline.push(pair("Cancel download", "cancelDownload"));
                }
                if state == "failed" || state == "cancelled" {
                    offline.push(pair("Retry download", "download"));
                }
                offline.push(pair("Remove download", "removeDownload"));
            } else {
                offline.push(pair("Download", "download"));
            }
        }
        let mut playlist = vec![];
        if row["playlistId"].is_string() {
            playlist = vec![
                pair("Move up in playlist", "playlistUp"),
                pair("Move down in playlist", "playlistDown"),
                pair("Remove from playlist", "playlistRemove"),
            ];
        }
        let mut danger = vec![];
        if library {
            danger.push(pair("Delete from server…", "deleteSong"));
        }
        vec![queue, navigate, saving, offline, playlist, danger]
    }
    fn track_row(&self, row: &Value, offset: usize, album: bool) -> gtk::ListBoxRow {
        if let Some(kind) = row["browseKind"].as_str().filter(|k| *k != "songs") {
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            line.set_margin_top(6);
            line.set_margin_bottom(6);
            line.set_margin_start(8);
            line.set_margin_end(4);
            line.append(&self.art.tile(row, Art::for_kind(kind), 40, true));
            let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
            text.set_valign(gtk::Align::Center);
            text.set_hexpand(true);
            let name = row["title"]
                .as_str()
                .or(row["name"].as_str())
                .unwrap_or("Collection");
            text.append(&ui::label(name, &["track-title"]));
            let subtitle = Self::subtitle(row, kind);
            if !subtitle.is_empty() {
                text.append(&ui::label(&subtitle, &["dim", "caption"]));
            }
            line.append(&text);
            let sections = self.collection_menu(row, kind);
            if !sections.is_empty() {
                let tx = self.input.clone();
                let item = row.clone();
                let menu = ui::action_menu(&sections, "row", move |op| {
                    tx.emit(Msg::Action(item.clone(), op))
                });
                menu.set_tooltip_text(Some(&format!("More options for {name}")));
                line.append(&menu);
            }
            let item = gtk::ListBoxRow::new();
            item.set_child(Some(&line));
            return item;
        }
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        line.set_margin_top(6);
        line.set_margin_bottom(6);
        line.set_margin_start(8);
        line.set_margin_end(4);
        let id = row["id"].as_str().unwrap_or("");
        if self.selecting && self.resolved() {
            let check = gtk::CheckButton::new();
            check.set_active(self.selected.contains(id));
            check.set_valign(gtk::Align::Center);
            let tx = self.input.clone();
            let key = id.to_owned();
            check.connect_toggled(move |_| tx.emit(Msg::ToggleSelected(key.clone())));
            line.append(&check);
        }
        let number = if album {
            row["track"]
                .as_u64()
                .map(|t| t as usize)
                .unwrap_or(offset + 1)
        } else {
            offset + 1
        };
        let index = ui::label(&number.to_string(), &["dim", "numeric"]);
        index.set_width_chars(3);
        index.set_xalign(1.);
        line.append(&index);
        if !album {
            line.append(&self.art.tile(row, Art::Song, 40, true));
        }
        let title = row["title"]
            .as_str()
            .or(row["name"].as_str())
            .unwrap_or("Untitled");
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_valign(gtk::Align::Center);
        text.set_hexpand(true);
        let heading = ui::label(title, &["track-title"]);
        if !id.is_empty() {
            heading.set_widget_name(&format!("track:{id}"));
        }
        if !self.current_id.is_empty() && id == self.current_id {
            heading.add_css_class("now-playing");
        }
        text.append(&heading);
        let byline = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        if id.starts_with("yt:") {
            let badge = ui::label("Preview", &["badge"]);
            badge.set_tooltip_text(Some("Streams from YouTube until added to your library"));
            byline.append(&badge);
        }
        byline.append(&ui::label(
            row["artist"].as_str().unwrap_or(""),
            &["dim", "caption"],
        ));
        text.append(&byline);
        line.append(&text);
        if !album {
            let album_label = ui::label(row["album"].as_str().unwrap_or(""), &["dim", "caption"]);
            album_label.set_width_chars(22);
            album_label.set_max_width_chars(22);
            line.append(&album_label);
        }
        if row["download"].is_object() {
            let state = row["download"]["state"].as_str().unwrap_or("");
            let progress = row["download"]["progress"].as_f64().unwrap_or(0.);
            match state {
                "ready" => line.append(&ui::label("Downloaded", &["badge", "ready"])),
                "queued" | "downloading" => {
                    let bar = gtk::ProgressBar::new();
                    bar.set_fraction(progress);
                    bar.set_valign(gtk::Align::Center);
                    bar.set_width_request(80);
                    bar.set_tooltip_text(Some(&format!("{state} · {:.0}%", progress * 100.)));
                    line.append(&bar);
                }
                _ => {
                    let badge = ui::label(
                        if state == "cancelled" {
                            "Cancelled"
                        } else {
                            "Failed"
                        },
                        &["badge", "failed"],
                    );
                    if let Some(error) = row["download"]["error"].as_str() {
                        badge.set_tooltip_text(Some(error));
                    }
                    line.append(&badge);
                }
            }
        }
        if let Some(duration) = row["duration"].as_f64().filter(|d| *d > 0.) {
            let clock = ui::label(&ui::clock(duration), &["dim", "numeric", "caption"]);
            clock.set_width_chars(5);
            clock.set_xalign(1.);
            line.append(&clock);
        }
        if row["queueLane"].is_string() {
            for (icon, tooltip, op) in [
                ("go-up-symbolic", "Move up", "queueUp"),
                ("go-down-symbolic", "Move down", "queueDown"),
                ("list-remove-symbolic", "Remove from queue", "queueRemove"),
            ] {
                let button = ui::icon_button(icon, tooltip);
                let tx = self.input.clone();
                let song = row.clone();
                button.connect_clicked(move |_| tx.emit(Msg::Action(song.clone(), op.into())));
                line.append(&button);
            }
        }
        let tx = self.input.clone();
        let song = row.clone();
        let menu = ui::action_menu(&self.song_menu(row), "row", move |op| {
            tx.emit(Msg::Action(song.clone(), op))
        });
        menu.set_tooltip_text(Some(&format!("More options for {title}")));
        line.append(&menu);
        let item = gtk::ListBoxRow::new();
        item.set_child(Some(&line));
        item
    }
    /// Albums, artists, playlists, Your Library and other browse rows: artwork, name and real counts.
    fn collection_list(&self) -> gtk::ListBox {
        let list = gtk::ListBox::new();
        list.add_css_class("track-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        for row in &self.rows {
            let kind = row["browseKind"]
                .as_str()
                .or(row["kind"].as_str().filter(|_| self.kind == "contexts"))
                .unwrap_or(&self.kind)
                .to_owned();
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 14);
            line.set_margin_top(6);
            line.set_margin_bottom(6);
            line.set_margin_start(8);
            line.set_margin_end(4);
            let art = if kind == "liked" {
                Art::Liked
            } else {
                Art::for_kind(&kind)
            };
            if art == Art::Liked {
                line.append(&self.art.tile(&json!({"name":"Liked Songs"}), art, 52, true));
            } else if art != Art::Other {
                line.append(&self.art.tile(row, art, 52, true));
            }
            let name = row["name"]
                .as_str()
                .or(row["title"].as_str())
                .or(row["value"].as_str())
                .unwrap_or("Untitled");
            let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
            text.set_valign(gtk::Align::Center);
            text.set_hexpand(true);
            text.append(&ui::label(name, &["track-title"]));
            let details = Self::subtitle(row, &kind);
            if !details.is_empty() {
                text.append(&ui::label(&details, &["dim", "caption"]));
            }
            line.append(&text);
            let sections = self.collection_menu(row, &kind);
            if !sections.is_empty() {
                let tx = self.input.clone();
                let item = row.clone();
                let menu = ui::action_menu(&sections, "row", move |op| {
                    tx.emit(Msg::Action(item.clone(), op))
                });
                menu.set_tooltip_text(Some(&format!("More options for {name}")));
                line.append(&menu);
            }
            let item = gtk::ListBoxRow::new();
            item.set_child(Some(&line));
            list.append(&item);
        }
        let rows = self.rows.clone();
        let tx = self.input.clone();
        let kind = self.kind.clone();
        list.connect_row_activated(move |_, row| {
            if let Some(item) = rows.get(row.index() as usize) {
                let target = item["browseKind"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| kind.clone());
                tx.emit(Msg::Action(item.clone(), target));
            }
        });
        list
    }
    fn playlist_picker(&self, selection: Selection, choices: Vec<Value>) {
        let dialog = adw::Dialog::new();
        dialog.set_title("Add to playlist");
        dialog.set_content_width(420);
        dialog.set_content_height(480);
        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        let create = adw::ActionRow::builder()
            .title("New playlist")
            .activatable(true)
            .build();
        create.add_prefix(&gtk::Image::from_icon_name("list-add-symbolic"));
        list.append(&create);
        for playlist in &choices {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(
                    playlist["name"].as_str().unwrap_or("Playlist"),
                ))
                .activatable(true)
                .build();
            if let Some(n) = playlist["songCount"].as_u64() {
                row.set_subtitle(&ui::count(n as usize, "song"));
            }
            row.add_prefix(&self.art.tile(playlist, Art::Playlist, 40, false));
            list.append(&row);
        }
        let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
        column.set_margin_all(16);
        let heading = match &selection {
            Selection::Album(album) => album["title"]
                .as_str()
                .or(album["name"].as_str())
                .unwrap_or("Album")
                .to_owned(),
            Selection::Songs(songs) if songs.len() == 1 => songs[0]["title"]
                .as_str()
                .unwrap_or("Selected song")
                .to_owned(),
            Selection::Songs(songs) => ui::count(songs.len(), "song"),
        };
        column.append(&ui::label(&heading, &["heading"]));
        if matches!(selection, Selection::Album(_)) {
            let note = ui::label(
                "The album is added to your library, then its songs join the playlist as they finish downloading.",
                &["dim"],
            );
            note.set_wrap(true);
            note.set_ellipsize(gtk::pango::EllipsizeMode::None);
            column.append(&note);
        }
        let tx = self.input.clone();
        let window = dialog.clone();
        list.connect_row_activated(move |_, row| {
            window.close();
            let index = row.index() as usize;
            if index == 0 {
                tx.emit(Msg::PlaylistEdit(None, Some(selection.clone())));
                return;
            }
            let id = choices[index - 1]["id"].clone();
            tx.emit(match &selection {
                Selection::Album(album) => Msg::LibraryCommand(
                    json!({"op":"acquireAlbum","kind":"newAlbums","item":album,"playlistId":id}),
                ),
                Selection::Songs(songs) => {
                    Msg::PlaylistCommand(json!({"op":"add","id":id,"songs":songs}))
                }
            });
        });
        column.append(&list);
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&adw::HeaderBar::new());
        toolbar.set_content(Some(
            &gtk::ScrolledWindow::builder()
                .vexpand(true)
                .hscrollbar_policy(gtk::PolicyType::Never)
                .child(&column)
                .build(),
        ));
        dialog.set_child(Some(&toolbar));
        dialog.present(Some(&self.window));
    }
    fn action(&mut self, song: Value, op: String, sender: &ComponentSender<Self>) {
        let library = |op: &str, kind: &str, item: Value| {
            Msg::LibraryCommand(json!({"op":op,"kind":kind,"item":item}))
        };
        match op.as_str() {
            "contexts" | "liked" => {
                self.push_trail();
                let kind = if op == "liked" {
                    "liked"
                } else {
                    song["kind"].as_str().unwrap_or("albums")
                };
                if kind == "liked" {
                    self.load("Liked Songs".into(), String::new(), Mode::Full);
                } else {
                    self.open(kind.into(), song.clone(), Mode::Full);
                }
            }
            "like" | "unlike" => sender.input(library(
                if op == "like" { "save" } else { "unsave" },
                "songs",
                song,
            )),
            "collectionSave" | "collectionUnsave" => {
                let kind = song["browseKind"]
                    .as_str()
                    .or(song["kind"].as_str())
                    .unwrap_or(&self.kind)
                    .to_owned();
                sender.input(library(
                    if op == "collectionSave" {
                        "save"
                    } else {
                        "unsave"
                    },
                    &kind,
                    song,
                ));
            }
            "acquireAlbum" => sender.input(library("acquireAlbum", "newAlbums", song)),
            "albumPlaylist" => sender.input(Msg::PickPlaylist(Selection::Album(song))),
            "editTags" => tags::show(self.core.clone(), self.input.clone(), song, &self.window),
            "goAlbum" | "goArtist" => {
                let link = if op == "goAlbum" {
                    ui::album_link(&song)
                } else {
                    ui::artist_link(&song)
                };
                if let Some(link) = link {
                    self.push_trail();
                    self.open(
                        if op == "goAlbum" { "albums" } else { "artists" }.into(),
                        link,
                        Mode::Full,
                    );
                }
            }
            "deleteSong" => {
                let dialog = adw::AlertDialog::new(
                    Some("Delete song from server?"),
                    Some("This removes the library file. Offline copies are managed separately."),
                );
                dialog.add_responses(&[("cancel", "Cancel"), ("delete", "Delete")]);
                dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
                dialog.set_close_response("cancel");
                let tx = self.input.clone();
                dialog.connect_response(None, move |_, r| {
                    if r == "delete" {
                        tx.emit(library("deleteSong", "songs", song.clone()));
                    }
                });
                dialog.present(Some(&self.window));
            }
            "suggestion" => {
                let query = song["query"].as_str().unwrap_or("").to_owned();
                self.search.set_text(&query);
                self.search_filter = "all".into();
                self.load("Omnibar".into(), query, Mode::Full);
            }
            "renamePlaylist" => sender.input(Msg::PlaylistEdit(Some(song), None)),
            "deletePlaylist" => sender.input(Msg::PlaylistDelete(song)),
            "pickPlaylist" => sender.input(Msg::PickPlaylist(Selection::Songs(vec![song]))),
            "playlistUp" | "playlistDown" | "playlistRemove" => {
                let index = song["playlistIndex"].as_u64().unwrap_or(0);
                let mut command = json!({"op":if op=="playlistRemove"{"remove"}else{"move"},"id":song["playlistId"],"songId":song["id"],"index":index,"from":index});
                command["to"] = json!(if op == "playlistUp" {
                    index.saturating_sub(1)
                } else {
                    (index + 1).min(self.rows.len().saturating_sub(1) as u64)
                });
                sender.input(Msg::PlaylistCommand(command));
            }
            "cancelDownload" | "removeDownload" => {
                let id = song["id"].as_str().unwrap_or("").to_owned();
                let result = if op == "cancelDownload" {
                    self.core.cancel_download(id)
                } else {
                    self.core.delete_download(id)
                };
                if let Err(error) = result {
                    self.toast(&error.to_string());
                }
                self.refresh_page(Mode::Refresh);
            }
            _ if op.starts_with("downloadCollection:") => {
                let kind = op.trim_start_matches("downloadCollection:").to_owned();
                let core = self.core.clone();
                let tx = self.input.clone();
                let id = song["id"].as_str().unwrap_or("").to_owned();
                self.toast("Downloading…");
                std::thread::spawn(move || {
                    let result = core.open_collection(kind, id).and_then(|text| {
                        let library: Vec<Value> = songs(&parse(&text)["rows"])
                            .into_iter()
                            .filter(ui::is_library_song)
                            .collect();
                        core.download_many(json!(library).to_string())
                    });
                    if let Err(error) = result {
                        tx.emit(Msg::Error(error.to_string()));
                    }
                    tx.emit(Msg::DownloadsChanged);
                });
            }
            _ if op.starts_with("queue") => {
                let priority = song["queueLane"] == "priority";
                let index = song["queueIndex"].as_u64().unwrap_or(0);
                let mut command = json!({"index":index});
                command["op"] = json!(match (op.as_str(), priority) {
                    ("queueJump", true) => "jumpSecondary",
                    ("queueJump", false) => "jump",
                    ("queueRemove", true) => "removeSecondary",
                    ("queueRemove", false) => "remove",
                    (_, true) => "moveSecondary",
                    (_, false) => "moveUpcoming",
                });
                if op == "queueUp" || op == "queueDown" {
                    command["from"] = json!(index);
                    command["direction"] = json!(if op == "queueUp" { -1 } else { 1 });
                    command["to"] = json!(if op == "queueUp" {
                        index.saturating_sub(1)
                    } else {
                        index + 1
                    });
                }
                let before = self.current_id.clone();
                if let Ok(result) = self.core.queue_command(command.to_string()) {
                    if op == "queueJump"
                        || parse(&result)["current"]["id"].as_str() != Some(&before)
                    {
                        self.start(op == "queueJump" || self.wants_playback);
                    }
                    if self.queue_dialog.is_some() {
                        self.render_queue();
                    }
                    if self.page == "Queue" {
                        self.load("Queue".into(), String::new(), Mode::Refresh);
                    }
                }
            }
            "albums" | "artists" | "playlists" | "genres" | "newAlbums" => {
                self.push_trail();
                self.open(op, song, Mode::Full);
            }
            _ => {
                if let Some(message) = match op.as_str() {
                    "playNext" => Some("Playing next"),
                    "playLast" => Some("Added to the end of Next in queue"),
                    "append" => Some("Added to queue"),
                    "download" => Some("Downloading…"),
                    "acquire" => Some("Adding to your library…"),
                    _ => None,
                } {
                    self.toast(message);
                }
                let core = self.core.clone();
                let tx = self.input.clone();
                std::thread::spawn(move || {
                    let result = if op == "acquire" {
                        core.acquire_song(song.to_string())
                    } else if op == "download" {
                        core.download(song.to_string()).map(|_| String::new())
                    } else {
                        core.queue_command(json!({"op":op,"songs":[song]}).to_string())
                    };
                    if result.is_err() {
                        tx.emit(Msg::Error(match op.as_str() {
                            "acquire" => "Couldn't add the song. Open Jobs to retry.".into(),
                            "download" => {
                                "Download couldn't start. Check the connection and retry.".into()
                            }
                            _ => "Queue could not be updated.".into(),
                        }));
                    }
                    tx.emit(Msg::DownloadsChanged);
                });
            }
        }
    }
    fn request_radio(&mut self) {
        if self.radio_inflight {
            return;
        }
        self.radio_inflight = true;
        let core = self.core.clone();
        let tx = self.input.clone();
        std::thread::spawn(move || {
            let extended = core.ensure_radio().unwrap_or(false);
            let _ = core.warm_upcoming();
            tx.emit(Msg::RadioDone(extended));
        });
    }
    fn start(&mut self, playing: bool) {
        self.wants_playback = playing;
        self.resume_after_radio = false;
        let Ok(text) = self.core.queue_command("{}".into()) else {
            return;
        };
        let snapshot = parse(&text);
        let song = snapshot["current"].clone();
        if song.is_null() {
            let _ = self.player.set_state(gst::State::Null);
            self.playing = false;
            self.wants_playback = false;
            self.pending_seek = None;
            self.refresh_now(true);
            return;
        }
        let preview = song["id"].as_str().is_some_and(|id| id.starts_with("yt:"));
        match self.core.media_url(song.to_string(), false) {
            Ok(url) => {
                let _ = self.player.set_state(gst::State::Null);
                self.pending.lock().unwrap().clear();
                self.stream_started.store(false, Ordering::SeqCst);
                self.player.set_property("uri", url);
                self.player
                    .set_property("volume", snapshot["gain"].as_f64().unwrap_or(1.));
                // The core keeps a saved position only for the song it belongs to.
                let resume = snapshot["resumePosition"].as_f64().unwrap_or(0.);
                self.pending_seek = (resume > 0.).then_some(resume);
                self.failed = false;
                let _ = self.player.set_state(if playing {
                    gst::State::Playing
                } else {
                    gst::State::Paused
                });
                self.playing = playing;
                if playing {
                    self.request_radio();
                }
                let core = self.core.clone();
                std::thread::spawn(move || {
                    let _ = core.warm_upcoming();
                });
                self.refresh_now(false);
            }
            Err(_) => {
                self.failed = true;
                self.playing = false;
                self.wants_playback = false;
                if playing {
                    self.toast(if preview {
                        "This preview can't be played right now."
                    } else {
                        "This song can't be played right now. Check the connection and retry."
                    });
                }
                self.refresh_now(true);
            }
        }
    }
}
/// Move the now-playing highlight without rebuilding the page.
fn mark_playing(widget: &gtk::Widget, name: &str) {
    let mut child = widget.first_child();
    while let Some(current) = child {
        if current.widget_name().starts_with("track:") {
            if current.widget_name() == name {
                current.add_css_class("now-playing");
            } else {
                current.remove_css_class("now-playing");
            }
        } else {
            mark_playing(&current, name);
        }
        child = current.next_sibling();
    }
}
fn greeting() -> String {
    let hour = glib::DateTime::now_local().map(|t| t.hour()).unwrap_or(12);
    match hour {
        5..=11 => "Good morning",
        12..=17 => "Good afternoon",
        _ => "Good evening",
    }
    .into()
}

// Test-only geometry of native controls; exclude text editors and session/status labels.
fn collect_controls(
    widget: &gtk::Widget,
    root: &gtk::Widget,
    in_menu: bool,
    controls: &mut Vec<Value>,
) {
    let in_menu = in_menu || widget.is::<gtk::Popover>();
    let label = if let Some(button) = widget.downcast_ref::<gtk::Button>() {
        button
            .label()
            .or_else(|| button.tooltip_text())
            .map(|s| s.to_string())
    } else if let Some(button) = widget.downcast_ref::<gtk::MenuButton>() {
        button.tooltip_text().map(|s| s.to_string())
    } else if in_menu {
        widget
            .downcast_ref::<gtk::Label>()
            .map(|l| l.text().to_string())
    } else {
        None
    };
    if widget.is_mapped() {
        if let (Some(label), Some(bounds)) = (label, widget.compute_bounds(root)) {
            if !label.is_empty() && bounds.width() > 0. && bounds.height() > 0. {
                controls.push(json!({"label":label,"x":bounds.x()+bounds.width()/2.,"y":bounds.y()+bounds.height()/2.}));
            }
        }
    }
    let mut child = widget.first_child();
    while let Some(item) = child {
        collect_controls(&item, root, in_menu, controls);
        child = item.next_sibling();
    }
}

fn main() {
    let override_server = std::env::var("ZAY_SERVER").ok();
    let server = override_server
        .clone()
        .unwrap_or("https://fedora-server.tailfc5dc.ts.net".into());
    let data = std::env::var("XDG_DATA_HOME")
        .unwrap_or_else(|_| format!("{}/.local/share", std::env::var("HOME").unwrap()));
    let core =
        NativeCore::new(server, format!("{data}/zaymusic")).expect("Initialize ZayMusic catalog");
    let saved = core.setting("server".into(), None).unwrap_or_default();
    let core = if override_server.is_none() && !saved.is_empty() && saved != "null" {
        NativeCore::new(saved, format!("{data}/zaymusic")).unwrap_or(core)
    } else {
        core
    };
    let app = RelmApp::new("io.zaymusic.Native");
    relm4::set_global_css(ui::CSS);
    app.run::<App>(core);
}

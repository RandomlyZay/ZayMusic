//! Stock Adwaita forms for shared connection, storage and import commands.
use super::{ui, Msg};
use relm4::{
    adw::{self, prelude::*},
    gtk::{self, gio, glib},
    Sender,
};
use serde_json::{json, Value};
use std::sync::Arc;
use zay_core::NativeCore;

pub fn command(core: Arc<NativeCore>, tx: Sender<Msg>, value: Value, success: &'static str) {
    // Input may contain pasted cookies. It never enters a Debug-printable UI message.
    std::thread::spawn(move || match core.management_command(value.to_string()) {
        Ok(_) => tx.emit(Msg::ManagementDone(success.into())),
        Err(_) => tx.emit(Msg::Error(
            "Action failed. Check the connection and retry.".into(),
        )),
    });
}
fn button(
    text: &str,
    core: &Arc<NativeCore>,
    tx: &Sender<Msg>,
    value: Value,
    success: &'static str,
) -> gtk::Button {
    let b = gtk::Button::with_label(text);
    let core = core.clone();
    let tx = tx.clone();
    b.connect_clicked(move |_| command(core.clone(), tx.clone(), value.clone(), success));
    b
}
/// A destructive command behind a stock confirmation dialog.
#[allow(clippy::too_many_arguments)] // A shared confirmation binds copy, command and native parent.
fn confirmed(
    text: &str,
    question: &'static str,
    detail: Option<&'static str>,
    response: &'static str,
    core: &Arc<NativeCore>,
    tx: &Sender<Msg>,
    value: Value,
    success: &'static str,
    window: &adw::ApplicationWindow,
) -> gtk::Button {
    let b = gtk::Button::with_label(text);
    let core = core.clone();
    let tx = tx.clone();
    let window = window.clone();
    b.connect_clicked(move |_| {
        let dialog = adw::AlertDialog::new(Some(question), detail);
        dialog.add_responses(&[("cancel", "Cancel"), ("confirm", response)]);
        dialog.set_response_appearance("confirm", adw::ResponseAppearance::Destructive);
        dialog.set_close_response("cancel");
        let core = core.clone();
        let tx = tx.clone();
        let value = value.clone();
        dialog.connect_response(None, move |_, r| {
            if r == "confirm" {
                command(core.clone(), tx.clone(), value.clone(), success);
            }
        });
        dialog.present(Some(&window));
    });
    b
}
fn group(title: &str) -> adw::PreferencesGroup {
    adw::PreferencesGroup::builder().title(title).build()
}
fn row(title: &str, value: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(glib::markup_escape_text(title))
        .build();
    let label = gtk::Label::new(Some(value));
    label.add_css_class("dim");
    label.set_selectable(true);
    row.add_suffix(&label);
    row
}
fn note(text: &str, classes: &[&str]) -> gtk::Label {
    let label = ui::label(text, classes);
    label.set_wrap(true);
    label.set_ellipsize(gtk::pango::EllipsizeMode::None);
    label.set_margin_top(6);
    label.set_margin_start(4);
    label
}
fn actions() -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    b.set_margin_top(10);
    b
}
/// The YouTube Music session in plain words.
pub fn session_state(youtube: &Value) -> &'static str {
    if youtube["configured"] != true {
        "Not connected"
    } else if youtube["verified"] != true {
        "Needs attention"
    } else if youtube["premium"] == true {
        "Connected · Premium"
    } else {
        "Connected"
    }
}
/// The server's managed-browser states; unknown values pass through.
fn browser_state(youtube: &Value) -> String {
    match youtube["browser"].as_str().unwrap_or("") {
        "" => "Not set up".into(),
        "connected" => "Signed in".into(),
        "signed_out" => "Signed out".into(),
        "disconnected" => "Offline".into(),
        other => other.into(),
    }
}
fn browser_help(youtube: &Value) -> &'static str {
    match youtube["browser"].as_str().unwrap_or("") {
        "signed_out" => "Open the remote browser and sign in to YouTube Music once.",
        "disconnected" => "The server's managed browser isn't running. Check the server.",
        _ => "",
    }
}
fn relogin_state(youtube: &Value) -> Option<String> {
    let state = youtube["lastReloginState"]
        .as_str()
        .filter(|s| !s.is_empty())?;
    Some(match state {
        "reconnected" => "Reconnected automatically".into(),
        "needs_human" => "Needs you to sign in".into(),
        "busy" => "In progress".into(),
        "failed" => "Failed".into(),
        "disabled" => "Off".into(),
        other => {
            let words = other.replace('_', " ");
            let mut chars = words.chars();
            chars
                .next()
                .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        }
    })
}
fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}
#[allow(clippy::too_many_arguments)]
pub fn settings(
    core: Arc<NativeCore>,
    tx: Sender<Msg>,
    extra: &Value,
    window: &adw::ApplicationWindow,
    shuffle: bool,
    repeat: &str,
) -> gtk::Widget {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 24);
    let prefs = core
        .preferences()
        .ok()
        .map(|s| super::parse(&s))
        .unwrap_or(Value::Null);
    let server = group("Server");
    server.set_description(Some(
        "Use the server's Tailscale HTTPS address. Restart ZayMusic after changing servers.",
    ));
    let address = adw::EntryRow::builder()
        .title("Server address")
        .text(prefs["server"].as_str().unwrap_or(""))
        .show_apply_button(true)
        .build();
    {
        let core = core.clone();
        let input = tx.clone();
        address.connect_apply(move |r| {
            input.emit(Msg::Error(
                match core.set_preference("server".into(), r.text().trim().into()) {
                    Ok(_) => "Saved. Restart ZayMusic to connect to the new server.",
                    Err(_) => "Use an HTTP(S) URL without credentials.",
                }
                .into(),
            ))
        });
    }
    server.add(&address);
    column.append(&server);

    let playback = group("Playback");
    let autoplay = adw::SwitchRow::builder()
        .title("Autoplay")
        .subtitle("Play similar songs when the queue ends")
        .active(prefs["autoplay"] == true)
        .build();
    let input = tx.clone();
    autoplay.connect_active_notify(move |r| input.emit(Msg::Autoplay(r.is_active())));
    playback.add(&autoplay);
    let volume = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0., 1., 0.01);
    volume.set_value(core.volume());
    volume.set_hexpand(true);
    volume.set_width_request(180);
    volume.set_draw_value(false);
    volume.update_property(&[gtk::accessible::Property::Label("Volume")]);
    let input = tx.clone();
    volume.connect_change_value(move |_, _, v| {
        input.emit(Msg::Volume(v));
        glib::Propagation::Proceed
    });
    let volume_row = adw::ActionRow::builder().title("Volume").build();
    volume_row.add_suffix(&volume);
    playback.add(&volume_row);
    let shuffle_row = adw::SwitchRow::builder()
        .title("Shuffle")
        .active(shuffle)
        .build();
    let input = tx.clone();
    shuffle_row.connect_active_notify(move |r| input.emit(Msg::SetShuffle(r.is_active())));
    playback.add(&shuffle_row);
    let repeat_row = adw::ComboRow::builder()
        .title("Repeat")
        .model(&gtk::StringList::new(&["Off", "All songs", "One song"]))
        .selected(match repeat {
            "all" => 1,
            "one" => 2,
            _ => 0,
        })
        .build();
    let input = tx.clone();
    repeat_row.connect_selected_notify(move |r| {
        input.emit(Msg::SetLoop(
            ["off", "all", "one"][r.selected().min(2) as usize].into(),
        ))
    });
    playback.add(&repeat_row);
    column.append(&playback);

    let youtube = &extra["youtube"];
    let music = group("YouTube Music");
    music.set_description(Some(
        "Sign in once in the server's managed browser. ZayMusic never stores YouTube credentials.",
    ));
    if !youtube.is_object() {
        music.add(&row("Connection status", "Unavailable · check Tailscale"));
    } else {
        music.add(&row("Managed browser", &browser_state(youtube)));
        let help = browser_help(youtube);
        if !help.is_empty() {
            music.add(&note(help, &["dim", "caption"]));
        }
        music.add(&row("Session", session_state(youtube)));
        if youtube["selfRelogin"] == true {
            music.add(&row("Automatic reconnection", "On"));
        }
        if let Some(state) = relogin_state(youtube) {
            music.add(&row("Last reconnect", &state));
        }
        if let Some(days) = youtube["expiresInDays"].as_i64() {
            music.add(&row(
                "Cookies expire",
                &if days <= 0 {
                    "Expired".into()
                } else {
                    format!("In {}", ui::count(days as usize, "day"))
                },
            ));
        }
        if let Some(message) = youtube["message"].as_str().filter(|s| !s.is_empty()) {
            music.add(&note(message, &["caption"]));
        }
    }
    let remote = adw::EntryRow::builder()
        .title("Remote browser URL")
        .text(prefs["remoteBrowserUrl"].as_str().unwrap_or(""))
        .show_apply_button(true)
        .build();
    {
        let core = core.clone();
        let input = tx.clone();
        remote.connect_apply(move |r| {
            match core.set_preference("remoteBrowserUrl".into(), r.text().trim().into()) {
                Ok(_) => input.emit(Msg::ManagementDone("Browser address saved.".into())),
                Err(_) => input.emit(Msg::Error(
                    "Use an HTTP(S) URL without credentials, query or fragment.".into(),
                )),
            }
        });
    }
    music.add(&remote);
    let buttons = actions();
    if let Some(url) = prefs["remoteBrowserUrl"]
        .as_str()
        .filter(|u| u.starts_with("http://") || u.starts_with("https://"))
    {
        let link = gtk::LinkButton::with_label(url, "Open remote browser");
        buttons.append(&link);
    }
    buttons.append(&button(
        "Verify connection",
        &core,
        &tx,
        json!({"op":"verifyConnection"}),
        "Connection checked.",
    ));
    if youtube["configured"] == true {
        let disconnect = confirmed(
            "Disconnect cookies",
            "Disconnect YouTube cookies?",
            Some("Acquisitions that need YouTube Music sign-in stop until you reconnect."),
            "Disconnect",
            &core,
            &tx,
            json!({"op":"disconnectCookies"}),
            "Cookies disconnected.",
            window,
        );
        disconnect.add_css_class("destructive-action");
        buttons.append(&disconnect);
    }
    music.add(&buttons);
    column.append(&music);

    let oauth = group("Google API (OAuth)");
    oauth.set_description(Some(
        "Sign-in codes stay in memory and expire automatically.",
    ));
    oauth.add(&row(
        "API connection",
        if youtube["oauth"]["connected"] == true {
            "Connected"
        } else {
            "Disconnected"
        },
    ));
    let buttons = actions();
    if let Some(code) = extra["oauthFlow"]["user_code"]
        .as_str()
        .filter(|s| !s.is_empty())
    {
        let label = gtk::Label::new(Some(code));
        label.set_selectable(true);
        label.add_css_class("title-2");
        label.add_css_class("numeric");
        label.set_margin_top(12);
        label.update_property(&[gtk::accessible::Property::Label(&format!(
            "Authorization code {code}"
        ))]);
        oauth.add(&label);
        if let Some(url) = extra["oauthFlow"]["verification_url"].as_str() {
            buttons.append(&gtk::LinkButton::with_label(url, "Authorize in browser"));
        }
        let copy = gtk::Button::with_label("Copy code");
        let code = code.to_owned();
        let input = tx.clone();
        copy.connect_clicked(move |b| {
            b.clipboard().set_text(&code);
            input.emit(Msg::Error("Code copied.".into()));
        });
        buttons.append(&copy);
        buttons.append(&button(
            "Get a new code",
            &core,
            &tx,
            json!({"op":"oauthStart"}),
            "New code ready.",
        ));
        // Cancelling forgets the pending code only; an existing connection is kept.
        buttons.append(&button(
            "Cancel sign-in",
            &core,
            &tx,
            json!({"op":"oauthCancel"}),
            "Sign-in cancelled.",
        ));
        let waiting = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        waiting.set_margin_top(8);
        let spinner = gtk::Spinner::new();
        spinner.set_spinning(true);
        waiting.append(&spinner);
        waiting.append(&ui::label("Waiting for authorization…", &["dim"]));
        oauth.add(&waiting);
    } else if youtube["oauth"]["connected"] == true {
        let disconnect = button(
            "Disconnect Google API",
            &core,
            &tx,
            json!({"op":"oauthDisconnect"}),
            "Google API disconnected.",
        );
        disconnect.add_css_class("destructive-action");
        buttons.append(&disconnect);
    } else {
        let connect = button(
            "Connect Google API",
            &core,
            &tx,
            json!({"op":"oauthStart"}),
            "Enter the code to authorize ZayMusic.",
        );
        let configured = youtube["oauth"]["configured"] == true;
        connect.set_sensitive(configured);
        buttons.append(&connect);
        if !configured {
            oauth.add(&note(
                "Configure the OAuth client on the server first.",
                &["dim", "caption"],
            ));
        }
    }
    oauth.add(&buttons);
    if let Some(error) = extra["oauthError"].as_str().filter(|s| !s.is_empty()) {
        oauth.add(&note(error, &["warning-text"]));
    }
    column.append(&oauth);

    let advanced = group("Advanced — manual cookies");
    advanced.set_description(Some(
        "Managed sign-in is preferred. Cookies are sent to the server once and never saved in this app.",
    ));
    let paste = gtk::Button::with_label("Connect with cookies.txt…");
    paste.set_halign(gtk::Align::Start);
    let parent = window.clone();
    let c = core.clone();
    let input = tx.clone();
    paste.connect_clicked(move |_| cookie_dialog(c.clone(), input.clone(), &parent));
    advanced.add(&paste);
    column.append(&advanced);

    let storage = group("Offline storage");
    let stats = core
        .storage_stats()
        .map(|s| super::parse(&s))
        .unwrap_or(Value::Null);
    storage.add(&row(
        "Downloaded audio",
        &glib::format_size(stats["readyBytes"].as_u64().unwrap_or(0)),
    ));
    let b = gtk::Button::with_label("Manage downloads");
    b.set_halign(gtk::Align::Start);
    b.set_margin_top(10);
    b.connect_clicked(move |_| tx.emit(Msg::Page("Downloads".into())));
    storage.add(&b);
    column.append(&storage);
    let clamp = adw::Clamp::new();
    clamp.set_maximum_size(720);
    clamp.set_child(Some(&column));
    clamp.upcast()
}

/// Pasted or chosen cookie text lives only in this dialog's buffer: kept for a retry after
/// a rejected attempt, cleared on success or close, never logged, persisted or messaged.
fn cookie_dialog(core: Arc<NativeCore>, tx: Sender<Msg>, window: &adw::ApplicationWindow) {
    let dialog = adw::Dialog::builder()
        .title("Connect YouTube Music")
        .content_width(560)
        .content_height(420)
        .build();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
    column.set_margin_start(18);
    column.set_margin_end(18);
    column.set_margin_top(6);
    column.set_margin_bottom(18);
    column.append(&note(
        "Paste the contents of a cookies.txt file, or choose the file. It is sent to the server once and never stored in this app.",
        &["dim"],
    ));
    let entry = gtk::TextView::new();
    entry.set_monospace(true);
    entry.set_wrap_mode(gtk::WrapMode::Char);
    entry.set_input_hints(gtk::InputHints::NO_SPELLCHECK | gtk::InputHints::NO_EMOJI);
    entry.update_property(&[gtk::accessible::Property::Label("Cookies.txt contents")]);
    let frame = gtk::ScrolledWindow::builder()
        .child(&entry)
        .vexpand(true)
        .min_content_height(160)
        .build();
    frame.add_css_class("card");
    column.append(&frame);
    let error = note("", &["warning-text"]);
    error.set_visible(false);
    column.append(&error);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let choose = gtk::Button::with_label("Choose file…");
    let clear = gtk::Button::with_label("Clear");
    let connect = gtk::Button::with_label("Connect");
    connect.add_css_class("suggested-action");
    connect.set_hexpand(true);
    connect.set_halign(gtk::Align::End);
    row.append(&choose);
    row.append(&clear);
    row.append(&connect);
    column.append(&row);
    toolbar.set_content(Some(&column));
    dialog.set_child(Some(&toolbar));
    let buffer = entry.buffer();
    connect.set_sensitive(false);
    buffer.connect_changed({
        let connect = connect.clone();
        move |b| connect.set_sensitive(b.char_count() > 0)
    });
    clear.connect_clicked({
        let buffer = buffer.clone();
        move |_| buffer.set_text("")
    });
    choose.connect_clicked({
        let buffer = buffer.clone();
        let error = error.clone();
        let window = window.clone();
        move |_| {
            let filter = gtk::FileFilter::new();
            filter.set_name(Some("Cookie files"));
            filter.add_mime_type("text/plain");
            filter.add_suffix("txt");
            let filters = gio::ListStore::new::<gtk::FileFilter>();
            filters.append(&filter);
            let chooser = gtk::FileDialog::builder()
                .title("Choose cookies.txt")
                .modal(true)
                .filters(&filters)
                .build();
            let buffer = buffer.clone();
            let error = error.clone();
            let window = window.clone();
            glib::spawn_future_local(async move {
                let Ok(file) = chooser.open_future(Some(&window)).await else {
                    return;
                };
                let text = file
                    .load_contents_future()
                    .await
                    .ok()
                    .filter(|(bytes, _)| !bytes.is_empty() && bytes.len() <= 1_000_000)
                    .and_then(|(bytes, _)| String::from_utf8(bytes.to_vec()).ok());
                match text {
                    Some(text) => {
                        buffer.set_text(&text);
                        error.set_visible(false);
                    }
                    None => {
                        error.set_label("Choose a cookies.txt file exported from your browser.");
                        error.set_visible(true);
                    }
                }
            });
        }
    });
    connect.connect_clicked({
        let buffer = buffer.clone();
        let dialog = dialog.clone();
        let error = error.clone();
        move |button| {
            let cookies = buffer
                .text(&buffer.start_iter(), &buffer.end_iter(), false)
                .to_string();
            button.set_sensitive(false);
            error.set_visible(false);
            let core = core.clone();
            let tx = tx.clone();
            let buffer = buffer.clone();
            let dialog = dialog.clone();
            let error = error.clone();
            let button = button.clone();
            glib::spawn_future_local(async move {
                let ok = gio::spawn_blocking(move || {
                    core.management_command(
                        json!({"op":"saveCookies","cookies":cookies}).to_string(),
                    )
                    .is_ok()
                })
                .await
                .unwrap_or(false);
                if ok {
                    buffer.set_text("");
                    dialog.close();
                    tx.emit(Msg::ManagementDone(
                        "Cookies sent. Verify the connection to confirm.".into(),
                    ));
                } else {
                    button.set_sensitive(true);
                    error.set_label(
                        "The server didn't accept these cookies. Check the file and try again.",
                    );
                    error.set_visible(true);
                }
            });
        }
    });
    dialog.connect_closed(move |_| buffer.set_text(""));
    dialog.present(Some(window));
}

pub fn rows(
    core: Arc<NativeCore>,
    tx: Sender<Msg>,
    kind: &str,
    rows: &[Value],
    window: &adw::ApplicationWindow,
) -> gtk::Widget {
    let list = gtk::ListBox::new();
    list.add_css_class("boxed-list");
    list.set_selection_mode(gtk::SelectionMode::None);
    for item in rows {
        let box_ = gtk::Box::new(gtk::Orientation::Vertical, 8);
        box_.set_margin_start(12);
        box_.set_margin_end(12);
        box_.set_margin_top(12);
        box_.set_margin_bottom(12);
        let title = item["name"]
            .as_str()
            .or(item["title"].as_str())
            .unwrap_or("Download");
        box_.append(&ui::label(title, &["heading"]));
        let id = item["id"]
            .as_str()
            .or(item["playlistId"].as_str())
            .unwrap_or("");
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        if kind == "jobs" {
            let status = item["status"].as_str().unwrap_or("");
            let detail: Vec<String> = [
                capitalized(status),
                item["stage"].as_str().unwrap_or("").into(),
            ]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect();
            box_.append(&ui::label(&detail.join(" · "), &["dim"]));
            if ui::job_bucket(status) == "active" {
                let progress = gtk::ProgressBar::new();
                progress.set_fraction(item["progress"].as_f64().unwrap_or(0.).clamp(0., 1.));
                box_.append(&progress);
            }
            if let Some(error) = item["error"].as_str().filter(|s| !s.is_empty()) {
                box_.append(&ui::label(error, &["warning-text"]));
            }
            if ["failed", "cancelled"].contains(&status) {
                actions.append(&button(
                    "Retry",
                    &core,
                    &tx,
                    json!({"op":"retryJob","id":id}),
                    "Retrying.",
                ));
            }
            if ["queued", "running", "downloading"].contains(&status) {
                actions.append(&button(
                    "Cancel",
                    &core,
                    &tx,
                    json!({"op":"cancelJob","id":id}),
                    "Cancelled.",
                ));
            } else {
                actions.append(&confirmed(
                    "Dismiss",
                    "Dismiss this job?",
                    None,
                    "Dismiss job",
                    &core,
                    &tx,
                    json!({"op":"deleteJob","id":id}),
                    "Job dismissed.",
                    window,
                ));
            }
        } else if kind == "imports" {
            box_.append(&ui::label(
                &format!(
                    "{} / {} complete · {} failed",
                    item["done"].as_u64().unwrap_or(0),
                    item["total"].as_u64().unwrap_or(0),
                    item["failed"].as_u64().unwrap_or(0)
                ),
                &["dim"],
            ));
            actions.append(&button(
                "Pull new songs",
                &core,
                &tx,
                json!({"op":"pullImport","id":id}),
                "Pulling new songs.",
            ));
            actions.append(&button(
                "Refresh status",
                &core,
                &tx,
                json!({"op":"syncImport","id":id}),
                "Import refreshed.",
            ));
            let details = gtk::Button::with_label("Track details");
            let entry = item.clone();
            let parent = window.clone();
            details.connect_clicked(move |_| import_details(&entry, &parent));
            actions.append(&details);
            if let Some(playlist) = item["playlistId"].as_str().filter(|s| !s.is_empty()) {
                let open = gtk::Button::with_label("Open playlist");
                let target = json!({"id":playlist,"name":title});
                let tx = tx.clone();
                open.connect_clicked(move |_| {
                    tx.emit(Msg::Action(target.clone(), "playlists".into()))
                });
                actions.append(&open);
            }
            if item["failed"].as_u64().unwrap_or(0) > 0 {
                let failed = gtk::Button::with_label("Show failed jobs");
                let tx = tx.clone();
                failed.connect_clicked(move |_| tx.emit(Msg::ShowFailedJobs));
                actions.append(&failed);
            }
        } else {
            let count = item["count"].as_u64().unwrap_or(0) as usize;
            box_.append(&ui::label(&ui::count(count, "song"), &["dim"]));
            actions.append(&button(
                "Import playlist",
                &core,
                &tx,
                json!({"op":"importPlaylist","id":id}),
                "Import started. Track progress in Imports.",
            ));
        }
        box_.append(&actions);
        list.append(&box_);
    }
    list.upcast()
}

fn import_details(item: &Value, window: &adw::ApplicationWindow) {
    let title = item["name"].as_str().unwrap_or("Import tracks");
    let dialog = adw::Dialog::builder()
        .title(title)
        .content_width(520)
        .content_height(560)
        .build();
    let list = gtk::ListBox::new();
    list.add_css_class("boxed-list");
    list.set_selection_mode(gtk::SelectionMode::None);
    list.set_valign(gtk::Align::Start);
    for song in crate::songs(&item["items"]) {
        let detail: Vec<String> = [
            capitalized(song["status"].as_str().unwrap_or("")),
            song["artist"].as_str().unwrap_or("").into(),
        ]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect();
        let row = adw::ActionRow::builder()
            .title(glib::markup_escape_text(
                song["title"].as_str().unwrap_or("Track"),
            ))
            .subtitle(glib::markup_escape_text(&detail.join(" · ")))
            .build();
        if let Some(error) = song["error"].as_str().filter(|s| !s.is_empty()) {
            let label = ui::label(error, &["warning-text", "caption"]);
            label.set_max_width_chars(28);
            label.set_tooltip_text(Some(error));
            row.add_suffix(&label);
        }
        list.append(&row);
    }
    let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    column.set_margin_start(16);
    column.set_margin_end(16);
    column.set_margin_top(8);
    column.set_margin_bottom(16);
    column.append(&list);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(
        &gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&column)
            .build(),
    ));
    dialog.set_child(Some(&toolbar));
    dialog.present(Some(window));
}

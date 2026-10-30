//! Stock Adwaita form over the core's `editTags`, with candidates from `metadata_lookup`.
use super::{parse, songs, Msg};
use relm4::{
    adw::{self, prelude::*},
    gtk::{self, glib},
    Sender,
};
use serde_json::{json, Value};
use std::sync::Arc;
use zay_core::NativeCore;

fn number(value: &Value) -> String {
    value
        .as_u64()
        .filter(|n| *n > 0)
        .map(|n| n.to_string())
        .unwrap_or_default()
}

/// Blank or non-positive numbers clear the field; text is trimmed.
fn edits(fields: &[(&str, adw::EntryRow)]) -> Value {
    let mut edits = json!({});
    for (key, row) in fields {
        let text = row.text().trim().to_owned();
        edits[*key] = if ["track", "year"].contains(key) {
            text.parse::<u64>()
                .ok()
                .filter(|n| *n > 0)
                .map(Value::from)
                .unwrap_or(Value::Null)
        } else {
            json!(text)
        };
    }
    edits
}

pub fn show(core: Arc<NativeCore>, tx: Sender<Msg>, song: Value, window: &adw::ApplicationWindow) {
    let dialog = adw::Dialog::builder()
        .title("Edit tags")
        .content_width(520)
        .content_height(640)
        .build();
    let header = adw::HeaderBar::new();
    header.set_show_end_title_buttons(false);
    let cancel = gtk::Button::with_label("Cancel");
    let save = gtk::Button::with_label("Save");
    save.add_css_class("suggested-action");
    header.pack_start(&cancel);
    header.pack_end(&save);
    let page = adw::PreferencesPage::new();
    let details = adw::PreferencesGroup::builder().title("Song").build();
    let numbers = adw::PreferencesGroup::new();
    let fields: Vec<(&str, adw::EntryRow)> = [
        ("title", "Title"),
        ("artist", "Artist"),
        ("album", "Album"),
        ("genre", "Genre"),
        ("track", "Track number"),
        ("year", "Year"),
    ]
    .into_iter()
    .map(|(key, title)| {
        let text = if ["track", "year"].contains(&key) {
            number(&song[key])
        } else {
            song[key].as_str().unwrap_or("").to_owned()
        };
        let row = adw::EntryRow::builder().title(title).text(text).build();
        if ["track", "year"].contains(&key) {
            row.set_input_purpose(gtk::InputPurpose::Digits);
            numbers.add(&row);
        } else {
            details.add(&row);
        }
        (key, row)
    })
    .collect();
    page.add(&details);
    page.add(&numbers);
    let lookup_group = adw::PreferencesGroup::builder()
        .title("Find metadata")
        .description("Choose a match to fill the form, then review and save.")
        .build();
    let lookup = adw::EntryRow::builder()
        .title("Artist and title")
        .text(
            [song["artist"].as_str(), song["title"].as_str()]
                .into_iter()
                .flatten()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" "),
        )
        .build();
    let find = gtk::Button::from_icon_name("system-search-symbolic");
    find.set_tooltip_text(Some("Find"));
    find.set_valign(gtk::Align::Center);
    find.add_css_class("flat");
    lookup.add_suffix(&find);
    lookup_group.add(&lookup);
    let message = gtk::Label::new(None);
    message.add_css_class("dim");
    message.set_xalign(0.);
    message.set_wrap(true);
    message.set_visible(false);
    message.set_margin_top(8);
    lookup_group.add(&message);
    let results = gtk::ListBox::new();
    results.add_css_class("boxed-list");
    results.set_selection_mode(gtk::SelectionMode::None);
    results.set_margin_top(8);
    results.set_visible(false);
    lookup_group.add(&results);
    page.add(&lookup_group);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&page));
    dialog.set_child(Some(&toolbar));

    let candidates: std::rc::Rc<std::cell::RefCell<Vec<Value>>> = Default::default();
    {
        let fields = fields.clone();
        let candidates = candidates.clone();
        results.connect_row_activated(move |_, row| {
            let Some(candidate) = candidates.borrow().get(row.index() as usize).cloned() else {
                return;
            };
            for (key, entry) in &fields {
                if let Some(text) = candidate[*key].as_str().filter(|s| !s.is_empty()) {
                    entry.set_text(text);
                } else if let Some(n) = candidate[*key].as_u64().filter(|n| *n > 0) {
                    entry.set_text(&n.to_string());
                }
            }
        });
    }
    let search = {
        let lookup = lookup.clone();
        let results = results.clone();
        let message = message.clone();
        let find = find.clone();
        move || {
            let query = lookup.text().trim().to_owned();
            if query.is_empty() {
                return;
            }
            find.set_sensitive(false);
            message.set_visible(false);
            let core = core.clone();
            let results = results.clone();
            let message = message.clone();
            let find = find.clone();
            let candidates = candidates.clone();
            glib::spawn_future_local(async move {
                let found = gtk::gio::spawn_blocking(move || core.metadata_lookup(query))
                    .await
                    .ok()
                    .and_then(|r| r.ok())
                    .map(|text| {
                        let v = parse(&text);
                        if v.is_array() {
                            songs(&v)
                        } else {
                            songs(&v["candidates"])
                        }
                    });
                find.set_sensitive(true);
                while let Some(child) = results.first_child() {
                    results.remove(&child);
                }
                let list = found.clone().unwrap_or_default();
                for candidate in &list {
                    let details: Vec<String> = [
                        candidate["artist"].as_str().map(str::to_owned),
                        candidate["album"].as_str().map(str::to_owned),
                        candidate["year"].as_u64().map(|y| y.to_string()),
                    ]
                    .into_iter()
                    .flatten()
                    .filter(|s| !s.is_empty())
                    .collect();
                    let row = adw::ActionRow::builder()
                        .title(glib::markup_escape_text(
                            candidate["title"].as_str().unwrap_or("Untitled"),
                        ))
                        .subtitle(glib::markup_escape_text(&details.join(" · ")))
                        .activatable(true)
                        .build();
                    if let Some(source) = candidate["source"].as_str().filter(|s| !s.is_empty()) {
                        let label = gtk::Label::new(Some(source));
                        label.add_css_class("dim");
                        label.add_css_class("caption");
                        row.add_suffix(&label);
                    }
                    results.append(&row);
                }
                results.set_visible(!list.is_empty());
                *candidates.borrow_mut() = list;
                let text = match &found {
                    None => "Metadata search is unavailable. Check the connection and retry.",
                    Some(list) if list.is_empty() => "No matches. Try a shorter search.",
                    _ => "",
                };
                message.set_label(text);
                message.set_visible(!text.is_empty());
            });
        }
    };
    let run = search.clone();
    find.connect_clicked(move |_| run());
    lookup.connect_entry_activated(move |_| search());
    let close = dialog.clone();
    cancel.connect_clicked(move |_| {
        close.close();
    });
    let close = dialog.clone();
    let title = fields[0].1.clone();
    title.connect_changed({
        let save = save.clone();
        move |row| save.set_sensitive(!row.text().trim().is_empty())
    });
    save.connect_clicked(move |_| {
        tx.emit(Msg::LibraryCommand(
            json!({"op":"editTags","kind":"songs","item":song,"edits":edits(&fields)}),
        ));
        close.close();
    });
    dialog.present(Some(window));
}

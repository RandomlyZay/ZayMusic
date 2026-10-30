use super::{parse, songs, ui, App, Msg};
use relm4::{
    adw::{self, prelude::*},
    gtk,
};
impl App {
    pub(super) fn render_queue(&self) {
        while let Some(child) = self.queue_box.first_child() {
            self.queue_box.remove(&child);
        }
        self.queue_box.set_margin_start(16);
        self.queue_box.set_margin_end(16);
        self.queue_box.set_margin_top(8);
        self.queue_box.set_margin_bottom(16);
        let rows = self
            .core
            .browse("Queue".into(), String::new())
            .map(|s| songs(&parse(&s)["rows"]))
            .unwrap_or_default();
        self.queue_box.append(&ui::label(
            &format!("Now playing · {}", self.title),
            &["heading"],
        ));
        let autoplay = adw::SwitchRow::builder()
            .title("Autoplay")
            .subtitle("Play similar songs when the queue ends")
            .active(self.autoplay_enabled)
            .build();
        let tx = self.input.clone();
        autoplay.connect_active_notify(move |r| tx.emit(Msg::Autoplay(r.is_active())));
        let settings = gtk::ListBox::new();
        settings.add_css_class("boxed-list");
        settings.set_selection_mode(gtk::SelectionMode::None);
        settings.append(&autoplay);
        self.queue_box.append(&settings);
        if rows.is_empty() {
            self.queue_box
                .append(&ui::label("Nothing else is queued.", &["dim"]));
        }
        for lane in ["priority", "main"] {
            if !rows.iter().any(|r| r["queueLane"] == lane) {
                continue;
            }
            self.queue_box.append(&ui::label(
                if lane == "priority" {
                    "Next in queue"
                } else {
                    "Main queue"
                },
                &["heading"],
            ));
            let list = gtk::ListBox::new();
            list.add_css_class("boxed-list");
            list.set_selection_mode(gtk::SelectionMode::None);
            for row in rows.iter().filter(|r| r["queueLane"] == lane) {
                let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                line.set_margin_top(8);
                line.set_margin_bottom(8);
                line.set_margin_start(8);
                let play = gtk::Button::new();
                play.add_css_class("flat");
                play.set_hexpand(true);
                let title = row["title"].as_str().unwrap_or("Track");
                let label = ui::label(title, &[]);
                play.set_child(Some(&label));
                let tx = self.input.clone();
                let song = row.clone();
                play.connect_clicked(move |_| {
                    tx.emit(Msg::Action(song.clone(), "queueJump".into()))
                });
                line.append(&play);
                for (icon, tooltip, op) in [
                    ("go-up-symbolic", "Move up", "queueUp"),
                    ("go-down-symbolic", "Move down", "queueDown"),
                    ("list-remove-symbolic", "Remove from queue", "queueRemove"),
                ] {
                    let b = ui::icon_button(icon, tooltip);
                    b.set_tooltip_text(Some(&format!("{tooltip}: {title}")));
                    let tx = self.input.clone();
                    let song = row.clone();
                    b.connect_clicked(move |_| tx.emit(Msg::Action(song.clone(), op.into())));
                    line.append(&b);
                }
                list.append(&line);
            }
            self.queue_box.append(&list);
        }
    }
    pub(super) fn show_queue(&mut self) {
        if let Some(dialog) = self.queue_dialog.take() {
            dialog.close();
            return;
        }
        self.render_queue();
        let dialog = adw::Dialog::builder()
            .title("Play queue")
            .content_width(520)
            .content_height(600)
            .build();
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&adw::HeaderBar::new());
        let scroll = gtk::ScrolledWindow::builder()
            .child(&self.queue_box)
            .build();
        toolbar.set_content(Some(&scroll));
        dialog.set_child(Some(&toolbar));
        let tx = self.input.clone();
        dialog.connect_closed(move |_| tx.emit(Msg::QueueClosed));
        dialog.present(Some(&self.window));
        self.queue_dialog = Some(dialog);
    }
}

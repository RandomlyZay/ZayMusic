use super::Msg;
use glib::variant::ToVariant;
use gstreamer::{self as gst, prelude::*};
use relm4::gtk::{gio, glib};
use std::{
    cell::RefCell,
    collections::HashMap,
    hash::{Hash, Hasher},
    sync::Arc,
};
use zay_core::NativeCore;
thread_local! {static BUS:RefCell<Option<gio::DBusConnection>>=const {RefCell::new(None)};static LAST:RefCell<String>=const {RefCell::new(String::new())};}
const PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER: &str = "org.mpris.MediaPlayer2.Player";
/// The MPRIS object path that identifies a queue song.
pub fn track_path(id: &str) -> String {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut hash);
    format!("/io/zaymusic/track/{:x}", hash.finish())
}
fn snapshot(core: &NativeCore) -> serde_json::Value {
    core.queue_command("{}".into())
        .map(|s| super::parse(&s))
        .unwrap_or_default()
}
/// Desktop shells read artwork from disk; only an already-cached file is offered so
/// property reads never touch the network.
fn art_url(core: &NativeCore, song: &serde_json::Value) -> Option<String> {
    [300, 96]
        .into_iter()
        .find_map(|size| core.artwork_cache_url(song.to_string(), size).ok())
}
fn loop_status(repeat: &str) -> &'static str {
    match repeat {
        "one" => "Track",
        "all" => "Playlist",
        _ => "None",
    }
}
fn metadata(core: &NativeCore, song: &serde_json::Value) -> glib::Variant {
    let mut m = HashMap::<String, glib::Variant>::new();
    if let Some(id) = song["id"].as_str() {
        if let Ok(path) = glib::variant::ObjectPath::try_from(track_path(id)) {
            m.insert("mpris:trackid".into(), path.to_variant());
        }
        m.insert(
            "xesam:title".into(),
            song["title"].as_str().unwrap_or("").to_variant(),
        );
        m.insert(
            "xesam:artist".into(),
            vec![song["artist"].as_str().unwrap_or("")].to_variant(),
        );
        m.insert(
            "xesam:album".into(),
            song["album"].as_str().unwrap_or("").to_variant(),
        );
        m.insert(
            "mpris:length".into(),
            ((song["duration"].as_f64().unwrap_or(0.) * 1e6) as i64).to_variant(),
        );
        if let Some(url) = art_url(core, song) {
            m.insert("mpris:artUrl".into(), url.to_variant());
        }
    }
    m.to_variant()
}
fn status(player: &gst::Element, song: &serde_json::Value) -> &'static str {
    if song.is_null() {
        "Stopped"
    } else if player.current_state() == gst::State::Playing {
        "Playing"
    } else {
        "Paused"
    }
}
fn emit(name: &str, interface: &str, args: glib::Variant) {
    BUS.with(|bus| {
        if let Some(bus) = bus.borrow().as_ref() {
            let _ = bus.emit_signal(None, PATH, interface, name, Some(&args));
        }
    });
}
/// Seeked must follow every jump in the clock that is not normal playback.
pub fn seeked(seconds: f64) {
    emit(
        "Seeked",
        PLAYER,
        ((seconds.max(0.) * 1e6) as i64,).to_variant(),
    );
}
pub fn notify(core: &NativeCore, player: &gst::Element) {
    let v = snapshot(core);
    let song = &v["current"];
    let status = status(player, song);
    let shuffle = v["state"]["shuffle"] == true;
    let repeat = loop_status(v["state"]["repeat"].as_str().unwrap_or("off"));
    let art = art_url(core, song).unwrap_or_default();
    let key = format!("{song}|{status}|{}|{shuffle}|{repeat}|{art}", core.volume());
    let changed = LAST.with(|last| {
        let mut last = last.borrow_mut();
        if *last == key {
            false
        } else {
            *last = key;
            true
        }
    });
    if !changed {
        return;
    }
    let mut values = HashMap::<String, glib::Variant>::new();
    values.insert("Metadata".into(), metadata(core, song));
    values.insert("PlaybackStatus".into(), status.to_variant());
    values.insert("Volume".into(), core.volume().to_variant());
    values.insert("Shuffle".into(), shuffle.to_variant());
    values.insert("LoopStatus".into(), repeat.to_variant());
    values.insert("CanPlay".into(), (!song.is_null()).to_variant());
    values.insert("CanPause".into(), (!song.is_null()).to_variant());
    values.insert("CanSeek".into(), (!song.is_null()).to_variant());
    emit(
        "PropertiesChanged",
        "org.freedesktop.DBus.Properties",
        (PLAYER, values, Vec::<String>::new()).to_variant(),
    );
}
pub fn register(core: Arc<NativeCore>, player: gst::Element, tx: relm4::Sender<Msg>) {
    let Ok(connection) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) else {
        return;
    };
    let xml = r#"<node>
 <interface name="org.mpris.MediaPlayer2">
 <method name="Raise"/><method name="Quit"/>
 <property name="CanQuit" type="b" access="read"/><property name="CanRaise" type="b" access="read"/>
 <property name="HasTrackList" type="b" access="read"/><property name="Identity" type="s" access="read"/>
 <property name="DesktopEntry" type="s" access="read"/><property name="SupportedUriSchemes" type="as" access="read"/><property name="SupportedMimeTypes" type="as" access="read"/>
 </interface>
 <interface name="org.mpris.MediaPlayer2.Player">
 <method name="Play"/><method name="Pause"/><method name="PlayPause"/><method name="Stop"/><method name="Next"/><method name="Previous"/>
 <method name="Seek"><arg name="Offset" direction="in" type="x"/></method>
 <method name="SetPosition"><arg name="TrackId" direction="in" type="o"/><arg name="Position" direction="in" type="x"/></method>
 <signal name="Seeked"><arg name="Position" type="x"/></signal>
 <property name="PlaybackStatus" type="s" access="read"/><property name="Metadata" type="a{sv}" access="read"/>
 <property name="LoopStatus" type="s" access="readwrite"/><property name="Shuffle" type="b" access="readwrite"/>
 <property name="Position" type="x" access="read"/><property name="Rate" type="d" access="read"/>
 <property name="MinimumRate" type="d" access="read"/><property name="MaximumRate" type="d" access="read"/>
 <property name="Volume" type="d" access="readwrite"/>
 <property name="CanGoNext" type="b" access="read"/><property name="CanGoPrevious" type="b" access="read"/>
 <property name="CanPlay" type="b" access="read"/><property name="CanPause" type="b" access="read"/>
 <property name="CanSeek" type="b" access="read"/><property name="CanControl" type="b" access="read"/>
 </interface></node>"#;
    let Ok(node) = gio::DBusNodeInfo::for_xml(xml) else {
        return;
    };
    BUS.with(|bus| *bus.borrow_mut() = Some(connection.clone()));
    for name in ["org.mpris.MediaPlayer2", PLAYER] {
        let Some(info) = node.lookup_interface(name) else {
            continue;
        };
        let tx = tx.clone();
        let p = player.clone();
        let props = player.clone();
        let core = core.clone();
        let set_tx = tx.clone();
        let _ = connection
            .register_object(PATH, &info)
            .method_call(move |_, _, _, _, method, args, invocation| {
                match method {
                    "Next" => tx.emit(Msg::Next),
                    "Previous" => tx.emit(Msg::Previous),
                    "PlayPause" => tx.emit(Msg::Toggle),
                    "Play" => {
                        if p.current_state() != gst::State::Playing {
                            tx.emit(Msg::Toggle);
                        }
                    }
                    "Pause" | "Stop" => {
                        if p.current_state() == gst::State::Playing {
                            tx.emit(Msg::Toggle);
                        }
                    }
                    "Seek" => {
                        if let Some((offset,)) = args.get::<(i64,)>() {
                            tx.emit(Msg::SeekBy(offset));
                        }
                    }
                    "SetPosition" => {
                        if let Some((track, position)) =
                            args.get::<(glib::variant::ObjectPath, i64)>()
                        {
                            tx.emit(Msg::SetPosition(track.as_str().to_owned(), position));
                        }
                    }
                    "Raise" | "Quit" => {}
                    _ => {
                        invocation.return_dbus_error(
                            "org.freedesktop.DBus.Error.NotSupported",
                            "Unsupported command",
                        );
                        return;
                    }
                }
                invocation.return_value(None);
            })
            .property(move |_, _, _, _, property| {
                let v = snapshot(&core);
                match property {
                    "Identity" => "ZayMusic".to_variant(),
                    "DesktopEntry" => "io.zaymusic.Native".to_variant(),
                    "SupportedUriSchemes" => vec!["https", "http", "file"].to_variant(),
                    "SupportedMimeTypes" => {
                        vec!["audio/ogg", "audio/mp4", "audio/mpeg", "audio/flac"].to_variant()
                    }
                    "PlaybackStatus" => status(&props, &v["current"]).to_variant(),
                    "Metadata" => metadata(&core, &v["current"]),
                    "LoopStatus" => {
                        loop_status(v["state"]["repeat"].as_str().unwrap_or("off")).to_variant()
                    }
                    "Shuffle" => (v["state"]["shuffle"] == true).to_variant(),
                    "Position" => (props
                        .query_position::<gst::ClockTime>()
                        .map(|t| t.useconds() as i64)
                        .unwrap_or(0))
                    .to_variant(),
                    "Rate" | "MinimumRate" | "MaximumRate" => 1_f64.to_variant(),
                    "Volume" => core.volume().to_variant(),
                    "CanQuit" | "CanRaise" | "HasTrackList" => false.to_variant(),
                    "CanPlay" | "CanPause" | "CanSeek" => (!v["current"].is_null()).to_variant(),
                    _ => true.to_variant(),
                }
            })
            .set_property(move |_, _, _, _, property, value| match property {
                "Volume" => match value.get::<f64>() {
                    Some(volume) if volume.is_finite() => {
                        set_tx.emit(Msg::Volume(volume));
                        true
                    }
                    _ => false,
                },
                "LoopStatus" => match value.get::<String>() {
                    Some(status) if ["None", "Track", "Playlist"].contains(&status.as_str()) => {
                        set_tx.emit(Msg::SetLoop(
                            match status.as_str() {
                                "Track" => "one",
                                "Playlist" => "all",
                                _ => "off",
                            }
                            .into(),
                        ));
                        true
                    }
                    _ => false,
                },
                "Shuffle" => match value.get::<bool>() {
                    Some(shuffle) => {
                        set_tx.emit(Msg::SetShuffle(shuffle));
                        true
                    }
                    None => false,
                },
                _ => false,
            })
            .build();
    }
    gio::bus_own_name_on_connection(
        &connection,
        "org.mpris.MediaPlayer2.zaymusic",
        gio::BusNameOwnerFlags::NONE,
        |_, _| {},
        |_, _| {},
    );
}

//! Media keys from the phone: play, pause, next and previous go to the MPRIS player on the
//! session bus (the one that is playing, else the first), volume goes to PipeWire/PulseAudio.

use std::process::Command;

use gio::glib::{self, prelude::*};
use gio::prelude::*;

use super::proto::Media;

const MPRIS: &str = "org.mpris.MediaPlayer2.";
const SEEK_US: i64 = 10_000_000;

/// The MPRIS method for a media action, if it's one a player answers.
pub fn mpris_method(m: Media) -> Option<&'static str> {
    match m {
        Media::PlayPause => Some("PlayPause"),
        Media::Next => Some("Next"),
        Media::Prev => Some("Previous"),
        Media::Stop => Some("Stop"),
        Media::SeekBack | Media::SeekFwd => Some("Seek"),
        _ => None,
    }
}

/// The `wpctl`/`pactl` arguments for a volume action.
pub fn volume_args(m: Media) -> Option<(&'static [&'static str], &'static [&'static str])> {
    match m {
        Media::VolUp => Some((&["set-volume", "@DEFAULT_AUDIO_SINK@", "5%+"], &["set-sink-volume", "@DEFAULT_SINK@", "+5%"])),
        Media::VolDown => Some((&["set-volume", "@DEFAULT_AUDIO_SINK@", "5%-"], &["set-sink-volume", "@DEFAULT_SINK@", "-5%"])),
        Media::Mute => Some((&["set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"], &["set-sink-mute", "@DEFAULT_SINK@", "toggle"])),
        _ => None,
    }
}

pub fn control(m: Media) -> Result<(), String> {
    if let Some((wpctl, pactl)) = volume_args(m) {
        let ok = Command::new("wpctl").args(wpctl).status().is_ok_and(|s| s.success());
        if ok || Command::new("pactl").args(pactl).status().is_ok_and(|s| s.success()) {
            return Ok(());
        }
        return Err("Couldn't change the volume.".into());
    }
    let method = mpris_method(m).ok_or("Unknown media action.")?;
    let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE).map_err(|e| e.to_string())?;
    let names = gio::DBusProxy::new_sync(&bus, gio::DBusProxyFlags::NONE, None, Some("org.freedesktop.DBus"), "/org/freedesktop/DBus", "org.freedesktop.DBus", gio::Cancellable::NONE)
        .and_then(|p| p.call_sync("ListNames", None, gio::DBusCallFlags::NONE, 2000, gio::Cancellable::NONE))
        .map_err(|e| e.to_string())?;
    let all: Vec<String> = names.child_value(0).get().unwrap_or_default();
    let players: Vec<String> = all.into_iter().filter(|n| n.starts_with(MPRIS)).collect();
    let proxy_for = |name: &str| {
        gio::DBusProxy::new_sync(&bus, gio::DBusProxyFlags::NONE, None, Some(name), "/org/mpris/MediaPlayer2", "org.mpris.MediaPlayer2.Player", gio::Cancellable::NONE).ok()
    };
    let proxies: Vec<gio::DBusProxy> = players.iter().filter_map(|n| proxy_for(n)).collect();
    let playing = |p: &gio::DBusProxy| p.cached_property("PlaybackStatus").and_then(|v| v.str().map(|s| s == "Playing")).unwrap_or(false);
    let Some(player) = proxies.iter().find(|p| playing(p)).or(proxies.first()) else {
        return Err("No media player is open on this PC.".into());
    };
    let args = match m {
        Media::SeekBack => Some((-SEEK_US,).to_variant()),
        Media::SeekFwd => Some((SEEK_US,).to_variant()),
        _ => None,
    };
    player.call_sync(method, args.as_ref(), gio::DBusCallFlags::NONE, 2000, gio::Cancellable::NONE).map_err(|e: glib::Error| e.to_string())?;
    Ok(())
}

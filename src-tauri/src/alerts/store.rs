//! The alert settings, kept as JSON in the app data folder (`alerts.json`).

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use serde::{Deserialize, Serialize};

pub const POSITIONS: &[&str] = &["bottom-right", "bottom-left", "top-right", "top-left"];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Which corner of the screen the alerts stack in.
    pub position: String,
    /// Seconds a message toast stays.
    pub toast_secs: u32,
    pub ringtone: bool,
    /// 0 to 100.
    pub volume: u32,
    /// Do not disturb: no message toasts, calls ring without a sound.
    pub dnd: bool,
    /// Show the sender's text in toasts.
    pub previews: bool,
    pub phone_calls: bool,
    pub sms: bool,
    pub whatsapp: bool,
    pub teams: bool,
    pub messenger: bool,
    /// Every other Messages account.
    pub other_messages: bool,
    /// Ctrl+Alt+A answers and Ctrl+Alt+D declines the ringing call.
    pub call_keys: bool,
    pub quick_replies: Vec<String>,
    /// The "sign in once in the call window" hint was shown.
    pub call_window_hint_shown: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            position: "bottom-right".into(),
            toast_secs: 8,
            ringtone: true,
            volume: 70,
            dnd: false,
            previews: true,
            phone_calls: true,
            sms: true,
            whatsapp: true,
            teams: true,
            messenger: true,
            other_messages: true,
            call_keys: true,
            quick_replies: vec!["Can't talk now, I'll call you back".into(), "On my way".into(), "Call you in 5 minutes".into()],
            call_window_hint_shown: false,
        }
    }
}

impl Settings {
    /// Whether alerts are on for a Messages service ("whatsapp", "teams", "messenger", anything else).
    pub fn source_on(&self, service: &str) -> bool {
        match service {
            "whatsapp" => self.whatsapp,
            "teams" => self.teams,
            "messenger" => self.messenger,
            _ => self.other_messages,
        }
    }

    /// Keeps values in range.
    pub fn cleaned(mut self) -> Settings {
        if !POSITIONS.contains(&self.position.as_str()) {
            self.position = "bottom-right".into();
        }
        self.toast_secs = self.toast_secs.clamp(2, 60);
        self.volume = self.volume.min(100);
        self.quick_replies = self.quick_replies.into_iter().map(|r| r.chars().filter(|c| !c.is_control()).take(160).collect::<String>().trim().to_string()).filter(|r| !r.is_empty()).take(6).collect();
        self
    }
}

static PATH: OnceLock<PathBuf> = OnceLock::new();
static SETTINGS: Mutex<Option<Settings>> = Mutex::new(None);

fn guard() -> MutexGuard<'static, Option<Settings>> {
    SETTINGS.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn init(path: PathBuf) {
    *guard() = Some(load(&path));
    let _ = PATH.set(path);
}

fn load(path: &Path) -> Settings {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice::<Settings>(&b).ok()).unwrap_or_default().cleaned()
}

pub fn get() -> Settings {
    guard().get_or_insert_with(Settings::default).clone()
}

/// Replaces the settings (cleaned) and saves them.
pub fn set(next: Settings) -> Result<Settings, String> {
    let next = next.cleaned();
    if let Some(path) = PATH.get() {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&next).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    }
    *guard() = Some(next.clone());
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn out_of_range_values_are_pulled_in() {
        let s = Settings { position: "middle".into(), toast_secs: 900, volume: 400, quick_replies: vec!["  ".into(), "ok".into()], ..Settings::default() }.cleaned();
        assert_eq!((s.position.as_str(), s.toast_secs, s.volume), ("bottom-right", 60, 100));
        assert_eq!(s.quick_replies, vec!["ok".to_string()]);
    }

    #[test]
    fn services_map_to_their_switch() {
        let s = Settings { teams: false, other_messages: false, ..Settings::default() };
        assert!(s.source_on("whatsapp") && !s.source_on("teams") && !s.source_on("telegram"));
    }

    #[test]
    fn a_partial_file_keeps_the_defaults() {
        let s: Settings = serde_json::from_str(r#"{"volume":10}"#).unwrap();
        assert_eq!((s.volume, s.toast_secs, s.sms), (10, 8, true));
    }
}

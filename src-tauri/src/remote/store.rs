//! Remote control's settings (Settings → Phone Link → Remote control), kept in `remote.json`.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// Whether phones may ask to see and control this PC at all.
    pub enabled: bool,
    /// Phones that may start a session without asking (their link ids).
    pub always: Vec<String>,
    /// Sessions never inject input: the phone only watches.
    pub view_only: bool,
    /// "x264" (constant bitrate) or "vaapi" (graphics chip, constant quality).
    pub encoder: String,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings { enabled: false, always: Vec::new(), view_only: false, encoder: "x264".into() }
    }
}

impl Settings {
    pub fn cleaned(mut self) -> Settings {
        if self.encoder != "vaapi" {
            self.encoder = "x264".into();
        }
        self.always.retain(|id| id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()));
        self.always.sort();
        self.always.dedup();
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

/// Adds a phone to the always-allowed list.
pub fn allow_always(phone_id: &str) {
    let mut s = get();
    if !s.always.iter().any(|p| p == phone_id) {
        s.always.push(phone_id.to_string());
        let _ = set(s);
    }
}

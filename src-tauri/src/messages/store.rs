//! The accounts and the section's settings, kept as JSON in the app data folder
//! (`messages.json`, beside the per-account data folders in `messages/`).

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use serde::{Deserialize, Serialize};

use super::services;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub service: String,
    pub url: String,
    pub name: String,
    pub order: i32,
    pub muted: bool,
    /// Minutes unused before the panel sleeps; None keeps it always on.
    pub sleep_after: Option<u32>,
    pub zoom: f64,
    /// Where it runs: "panel" (inside Zazabor) or "browser" (its own browser app window).
    #[serde(default = "panel_mode")]
    pub mode: String,
}

fn panel_mode() -> String {
    "panel".into()
}

pub fn valid_mode(mode: &str) -> bool {
    mode == "panel" || mode == "browser"
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Do not disturb: no desktop notifications from any account (unread counts still count).
    pub dnd: bool,
    /// Show the sender and text in notifications; off shows "New message" only.
    pub previews: bool,
    /// What a new account sleeps after, in minutes (WhatsApp and Teams start always on).
    pub default_sleep: u32,
    /// The "calls open in your browser" hint was shown once.
    pub calls_hint_shown: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { dnd: false, previews: true, default_sleep: 30, calls_hint_shown: false }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Store {
    pub accounts: Vec<Account>,
    pub settings: Settings,
}

/// What `msg_update` may change; anything left out stays.
#[derive(Deserialize, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Patch {
    pub name: Option<String>,
    pub muted: Option<bool>,
    /// Some(None) means always on; the field absent leaves it.
    #[serde(default, deserialize_with = "double_option")]
    pub sleep_after: Option<Option<u32>>,
    pub zoom: Option<f64>,
    pub mode: Option<String>,
}

fn double_option<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Option<u32>>, D::Error> {
    Option::<u32>::deserialize(d).map(Some)
}

pub const MIN_ZOOM: f64 = 0.5;
pub const MAX_ZOOM: f64 = 3.0;

pub fn clean_name(name: &str) -> String {
    name.chars().filter(|c| !c.is_control()).collect::<String>().trim().chars().take(40).collect()
}

impl Store {
    pub fn load(path: &Path) -> Store {
        std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let json = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        // Written beside it and renamed, so a crash never leaves half a file.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }

    pub fn get(&self, id: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.id == id)
    }

    fn next_id(&self, service: &str) -> String {
        for n in 1.. {
            let id = format!("{service}-{n}");
            if self.get(&id).is_none() {
                return id;
            }
        }
        unreachable!()
    }

    /// Adds an account at the end. The address must be allowed for the service.
    pub fn add(&mut self, service: &str, name: &str, url: Option<&str>, mode: &str) -> Result<Account, String> {
        if !valid_mode(mode) {
            return Err("unknown mode".into());
        }
        if !services::known(service) {
            return Err(format!("unknown service {service:?}"));
        }
        let url = match (services::find(service), url) {
            (Some(s), None) => s.urls[0].to_string(),
            (_, Some(u)) => u.to_string(),
            (None, None) => return Err("a custom account needs an address".into()),
        };
        if !services::url_ok(service, &url) {
            return Err("that address isn't allowed: it must be https".into());
        }
        let name = clean_name(name);
        if name.is_empty() {
            return Err("give the account a name".into());
        }
        let always = services::find(service).is_some_and(|s| s.always_on);
        let account = Account {
            id: self.next_id(service),
            service: service.into(),
            url,
            name,
            order: self.accounts.iter().map(|a| a.order).max().map_or(0, |o| o + 1),
            muted: false,
            sleep_after: if always { None } else { Some(self.settings.default_sleep.max(1)) },
            zoom: 1.0,
            mode: mode.into(),
        };
        self.accounts.push(account.clone());
        Ok(account)
    }

    pub fn update(&mut self, id: &str, patch: Patch) -> Result<Account, String> {
        let account = self.accounts.iter_mut().find(|a| a.id == id).ok_or("no such account")?;
        if let Some(name) = patch.name {
            let name = clean_name(&name);
            if name.is_empty() {
                return Err("the name can't be empty".into());
            }
            account.name = name;
        }
        if let Some(m) = patch.muted {
            account.muted = m;
        }
        if let Some(s) = patch.sleep_after {
            account.sleep_after = s.map(|m| m.clamp(1, 24 * 60));
        }
        if let Some(m) = patch.mode {
            if !valid_mode(&m) {
                return Err("unknown mode".into());
            }
            account.mode = m;
        }
        if let Some(z) = patch.zoom {
            account.zoom = z.clamp(MIN_ZOOM, MAX_ZOOM);
        }
        Ok(account.clone())
    }

    /// Puts the accounts in the order of `ids` (unknown ids are ignored, missing ones keep their place at the end).
    pub fn reorder(&mut self, ids: &[String]) {
        let mut next = 0;
        for id in ids {
            if let Some(a) = self.accounts.iter_mut().find(|a| &a.id == id) {
                a.order = next;
                next += 1;
            }
        }
        let mut rest: Vec<&mut Account> = self.accounts.iter_mut().filter(|a| !ids.contains(&a.id)).collect();
        rest.sort_by_key(|a| a.order);
        for a in rest {
            a.order = next;
            next += 1;
        }
        self.accounts.sort_by_key(|a| a.order);
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.accounts.len();
        self.accounts.retain(|a| a.id != id);
        self.accounts.len() != before
    }
}

static STORE: Mutex<Option<Store>> = Mutex::new(None);
static PATH: OnceLock<PathBuf> = OnceLock::new();

pub fn init(file: PathBuf) {
    *STORE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Store::load(&file));
    let _ = PATH.set(file);
}

/// The live store; the guard is held only for a short read or change.
pub fn lock() -> MutexGuard<'static, Option<Store>> {
    STORE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Reads the store.
pub fn read<T>(f: impl FnOnce(&Store) -> T) -> T {
    let mut guard = lock();
    f(guard.get_or_insert_with(Store::default))
}

/// Changes the store and saves it; the change stays in memory even if saving fails.
pub fn change<T>(f: impl FnOnce(&mut Store) -> Result<T, String>) -> Result<T, String> {
    let mut guard = lock();
    let store = guard.get_or_insert_with(Store::default);
    let out = f(store)?;
    if let Some(path) = PATH.get() {
        store.save(path)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zazabor-msg-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn round_trip() {
        let dir = scratch("store");
        let file = dir.join("messages.json");
        let mut store = Store::default();
        let a = store.add("whatsapp", "Mum", None, "panel").unwrap();
        let b = store.add("whatsapp", "Work", None, "panel").unwrap();
        let c = store.add("teams", "School", Some("https://teams.live.com/"), "panel").unwrap();
        store.update(&b.id, Patch { muted: Some(true), zoom: Some(1.2), ..Patch::default() }).unwrap();
        store.settings.dnd = true;
        store.save(&file).unwrap();
        let back = Store::load(&file);
        assert_eq!(back, store);
        assert_ne!(a.id, b.id);
        assert_eq!(c.url, "https://teams.live.com/");
        assert_eq!(back.get(&b.id).unwrap().zoom, 1.2);
        assert!(back.settings.dnd);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_or_broken_file_is_empty() {
        let dir = scratch("broken");
        assert_eq!(Store::load(&dir.join("nope.json")), Store::default());
        std::fs::write(dir.join("bad.json"), "{not json").unwrap();
        assert_eq!(Store::load(&dir.join("bad.json")), Store::default());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn defaults_and_validation() {
        let mut s = Store::default();
        assert_eq!(s.add("whatsapp", "A", None, "panel").unwrap().sleep_after, None);
        assert_eq!(s.add("telegram", "B", None, "panel").unwrap().sleep_after, Some(30));
        assert!(s.add("nonsense", "C", None, "panel").is_err());
        assert!(s.add("telegram", "C", None, "tab").is_err());
        let b = s.add("telegram", "Br", None, "browser").unwrap();
        assert_eq!(b.mode, "browser");
        let id = b.id.clone();
        s.update(&id, Patch { mode: Some("panel".into()), ..Patch::default() }).unwrap();
        assert_eq!(s.get(&id).unwrap().mode, "panel");
        assert!(s.update(&id, Patch { mode: Some("x".into()), ..Patch::default() }).is_err());
        assert!(s.add("custom", "C", None, "panel").is_err());
        assert!(s.add("custom", "C", Some("http://example.org/"), "panel").is_err());
        assert!(s.add("whatsapp", "C", Some("https://example.org/"), "panel").is_err());
        assert!(s.add("telegram", "   ", None, "panel").is_err());
        assert!(s.add("custom", "Mine", Some("https://example.org/chat"), "panel").is_ok());
        assert_eq!(s.add("whatsapp", &"x".repeat(100), None, "panel").unwrap().name.len(), 40);
    }

    #[test]
    fn update_and_reorder_and_remove() {
        let mut s = Store::default();
        let a = s.add("telegram", "A", None, "panel").unwrap().id;
        let b = s.add("telegram", "B", None, "panel").unwrap().id;
        let c = s.add("telegram", "C", None, "panel").unwrap().id;
        s.update(&a, Patch { name: Some("  Alpha ".into()), sleep_after: Some(None), zoom: Some(9.0), ..Patch::default() }).unwrap();
        let alpha = s.get(&a).unwrap();
        assert_eq!((alpha.name.as_str(), alpha.sleep_after, alpha.zoom), ("Alpha", None, MAX_ZOOM));
        assert!(s.update(&a, Patch { name: Some(" ".into()), ..Patch::default() }).is_err());
        s.reorder(&[c.clone(), a.clone()]);
        assert_eq!(s.accounts.iter().map(|x| x.id.clone()).collect::<Vec<_>>(), vec![c, a.clone(), b.clone()]);
        assert!(s.remove(&b));
        assert!(!s.remove(&b));
        assert_eq!(s.accounts.len(), 2);
    }

    #[test]
    fn patch_reads_sleep_forms() {
        let none: Patch = serde_json::from_str(r#"{"name":"x"}"#).unwrap();
        assert_eq!(none.sleep_after, None);
        let always: Patch = serde_json::from_str(r#"{"sleepAfter":null}"#).unwrap();
        assert_eq!(always.sleep_after, Some(None));
        let some: Patch = serde_json::from_str(r#"{"sleepAfter":15}"#).unwrap();
        assert_eq!(some.sleep_after, Some(Some(15)));
    }
}

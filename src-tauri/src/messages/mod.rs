//! Messages: the user's chat services (WhatsApp, Messenger, Teams, Telegram, ...) as their official
//! web apps in panels inside the main window (docs/plans/messages.md). Each account's panel is a
//! web view with its own data folder; the page reaches Zazabor only through the injected
//! script's reports (`events.rs`), never through Tauri's IPC. The page (src/modules/messages) draws
//! the sidebar and tells Rust which account to show and where.

pub mod events;
pub mod panels;
pub mod services;
pub mod store;

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use panels::{Rect, Spec};
use store::{Account, Patch, Settings};

#[derive(Default)]
struct Runtime {
    /// The account whose panel is on screen.
    shown: Option<String>,
    /// When each account was last on screen (or woken), for sleeping the unused ones.
    last_used: HashMap<String, Instant>,
    /// The last unread count each account reported.
    unread: HashMap<String, u32>,
}

static RT: Mutex<Option<Runtime>> = Mutex::new(None);

fn rt<T>(f: impl FnOnce(&mut Runtime) -> T) -> T {
    let mut guard = RT.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(Runtime::default))
}

/// Records an account's unread count; true when it changed.
pub fn set_unread(id: &str, count: u32) -> bool {
    rt(|r| r.unread.insert(id.to_string(), count) != Some(count))
}

/// Tells the page, once ever, that calls open in the browser.
pub fn hint(app: &AppHandle, text: &str) {
    let first = store::change(|s| Ok(!std::mem::replace(&mut s.settings.calls_hint_shown, true))).unwrap_or(false);
    if first {
        let _ = app.emit("msg-hint", serde_json::json!({ "text": text }));
    }
}

fn changed(app: &AppHandle) {
    let _ = app.emit("msg-changed", ());
}

/// One "minute" of the sleep timers (a test can shorten it).
fn minute() -> Duration {
    Duration::from_secs(std::env::var("ZAZABOR_MSG_MINUTE_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(60))
}

pub fn init(app: AppHandle, app_data: std::path::PathBuf) {
    panels::init(&app_data);
    store::init(app_data.join("messages.json"));
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(10));
        sleep_unused(&app);
    });
}

/// Sleeps the panels that were unused for their account's time.
fn sleep_unused(app: &AppHandle) {
    let due: Vec<Account> = store::read(|s| s.accounts.iter().filter(|a| a.sleep_after.is_some() && a.mode == "panel").cloned().collect());
    if due.is_empty() {
        return;
    }
    let alive = panels::alive(app);
    let now = Instant::now();
    let mut slept = false;
    for account in due.into_iter().filter(|a| alive.contains(&a.id)) {
        let (shown, last) = rt(|r| (r.shown.clone(), r.last_used.get(&account.id).copied()));
        let limit = minute() * account.sleep_after.unwrap_or(0);
        if shown.as_deref() != Some(&account.id) && last.is_some_and(|t| now.duration_since(t) >= limit) {
            eprintln!("[messages] {} sleeps (unused)", account.id);
            slept |= panels::destroy(app, &account.id, false).is_ok();
        }
    }
    if slept {
        changed(app);
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountState {
    #[serde(flatten)]
    account: Account,
    unread: u32,
    /// The panel is alive (not sleeping, not yet opened).
    awake: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    accounts: Vec<AccountState>,
    settings: Settings,
    shown: Option<String>,
}

#[tauri::command]
pub async fn msg_state(app: AppHandle) -> Result<State, String> {
    let alive = panels::alive(&app);
    let (accounts, settings) = store::read(|s| (s.accounts.clone(), s.settings.clone()));
    let (shown, unread) = rt(|r| (r.shown.clone(), r.unread.clone()));
    let accounts = accounts.into_iter().map(|a| AccountState { unread: unread.get(&a.id).copied().unwrap_or(0), awake: alive.contains(&a.id), account: a }).collect();
    Ok(State { accounts, settings, shown })
}

#[tauri::command]
pub async fn msg_add(app: AppHandle, service: String, name: String, url: Option<String>, mode: Option<String>) -> Result<Account, String> {
    let mode = mode.unwrap_or_else(|| "panel".into());
    let account = store::change(|s| s.add(&service, &name, url.as_deref(), &mode))?;
    changed(&app);
    Ok(account)
}

#[tauri::command]
pub async fn msg_update(app: AppHandle, id: String, patch: Patch) -> Result<Account, String> {
    let zoom = patch.zoom;
    let to_browser = patch.mode.as_deref() == Some("browser");
    let account = store::change(|s| s.update(&id, patch))?;
    if to_browser {
        // Its panel goes away; the login doesn't carry over to the browser.
        panels::destroy(&app, &id, false)?;
        rt(|r| {
            if r.shown.as_deref() == Some(&id) {
                r.shown = None;
            }
        });
    }
    if zoom.is_some() && panels::exists_on_main(&app, &id) {
        panels::zoom(&app, &id, account.zoom)?;
    }
    changed(&app);
    Ok(account)
}

#[tauri::command]
pub async fn msg_reorder(app: AppHandle, ids: Vec<String>) -> Result<(), String> {
    store::change(|s| {
        s.reorder(&ids);
        Ok(())
    })?;
    changed(&app);
    Ok(())
}

/// Removes an account; `wipe` also signs it out and deletes its data folder.
#[tauri::command]
pub async fn msg_remove(app: AppHandle, id: String, wipe: bool) -> Result<(), String> {
    panels::destroy(&app, &id, wipe)?;
    if wipe {
        panels::wipe_dir(&panels::browser_profile(&id)?)?;
    }
    store::change(|s| Ok(s.remove(&id)))?;
    rt(|r| {
        r.unread.remove(&id);
        r.last_used.remove(&id);
        if r.shown.as_deref() == Some(&id) {
            r.shown = None;
        }
    });
    changed(&app);
    Ok(())
}

/// Shows an account's panel at `rect` (making it first, or waking it) and hides the others.
#[tauri::command]
pub async fn msg_show(app: AppHandle, id: String, rect: Rect) -> Result<(), String> {
    let account = store::read(|s| s.get(&id).cloned()).ok_or("no such account")?;
    if account.mode == "browser" {
        return Err("this account runs in a browser window".into());
    }
    let woke = !panels::exists_on_main(&app, &id);
    if woke {
        panels::create(&app, Spec { id: id.clone(), service: account.service, url: account.url, zoom: account.zoom }, rect)?;
    }
    panels::set_rect(&app, &id, rect)?;
    panels::show_only(&app, Some(&id))?;
    rt(|r| {
        r.shown = Some(id.clone());
        r.last_used.insert(id, Instant::now());
    });
    if woke {
        changed(&app);
    }
    Ok(())
}

/// Hides the panel on screen (the user left the section, or a sheet covers it).
#[tauri::command]
pub async fn msg_hide(app: AppHandle) -> Result<(), String> {
    panels::show_only(&app, None)?;
    rt(|r| {
        if let Some(id) = r.shown.take() {
            r.last_used.insert(id, Instant::now());
        }
    });
    Ok(())
}

/// Moves the panel on screen (the page's content area changed).
#[tauri::command]
pub async fn msg_rect(app: AppHandle, rect: Rect) -> Result<(), String> {
    match rt(|r| r.shown.clone()) {
        Some(id) => panels::set_rect(&app, &id, rect),
        None => Ok(()),
    }
}

#[tauri::command]
pub async fn msg_reload(app: AppHandle, id: String) -> Result<(), String> {
    if panels::exists_on_main(&app, &id) {
        panels::reload(&app, &id)?;
        rt(|r| r.last_used.insert(id, Instant::now()));
    }
    Ok(())
}

/// Unloads an account's panel now (its memory comes back; the login stays on disk).
#[tauri::command]
pub async fn msg_hibernate(app: AppHandle, id: String) -> Result<(), String> {
    panels::destroy(&app, &id, false)?;
    rt(|r| {
        if r.shown.as_deref() == Some(&id) {
            r.shown = None;
        }
    });
    changed(&app);
    Ok(())
}

/// Opens (or brings up) the account's own browser window: a Chromium-family app window with a
/// profile of its own, never the user's browser profile. Without such a browser, the default one.
#[tauri::command]
pub async fn msg_open_browser_window(id: String) -> Result<(), String> {
    let account = store::read(|s| s.get(&id).cloned()).ok_or("no such account")?;
    let profile = panels::browser_profile(&id)?;
    crate::meet::browser::open(&account.url, &profile)
}

/// The browser an account's window would use, for the page's wording.
#[tauri::command]
pub async fn msg_browser_name() -> Option<String> {
    crate::meet::browser::label()
}

/// Opens the account's current page in the default browser.
#[tauri::command]
pub async fn msg_open_external(app: AppHandle, id: String) -> Result<(), String> {
    let account = store::read(|s| s.get(&id).cloned()).ok_or("no such account")?;
    let url = panels::current_uri(&app, &id).filter(|u| u.starts_with("https://") || u.starts_with("http://")).unwrap_or(account.url);
    crate::open_with_system(std::ffi::OsStr::new(&url))
}

/// A still picture of the panel, to show while a sheet covers it.
#[tauri::command]
pub async fn msg_snapshot(app: AppHandle, id: String) -> Result<String, String> {
    panels::snapshot(&app, &id)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPatch {
    dnd: Option<bool>,
    previews: Option<bool>,
    default_sleep: Option<u32>,
}

#[tauri::command]
pub async fn msg_settings(app: AppHandle, patch: SettingsPatch) -> Result<Settings, String> {
    let settings = store::change(|s| {
        if let Some(v) = patch.dnd {
            s.settings.dnd = v;
        }
        if let Some(v) = patch.previews {
            s.settings.previews = v;
        }
        if let Some(v) = patch.default_sleep {
            s.settings.default_sleep = v.clamp(1, 24 * 60);
        }
        Ok(s.settings.clone())
    })?;
    changed(&app);
    Ok(settings)
}

/// Bytes the accounts' data folders use.
#[tauri::command]
pub async fn msg_data_size() -> Result<u64, String> {
    Ok(panels::data_root().map(panels::dir_size).unwrap_or(0))
}

/// Closes every panel and deletes every data folder: signs out of all accounts (they stay listed).
#[tauri::command]
pub async fn msg_clear_data(app: AppHandle) -> Result<(), String> {
    for id in panels::alive(&app) {
        panels::destroy(&app, &id, false)?;
    }
    rt(|r| {
        r.shown = None;
        r.unread.clear();
    });
    if let Some(root) = panels::data_root() {
        panels::wipe_dir(root)?;
    }
    changed(&app);
    Ok(())
}

/// Starts the "always on" accounts' panels in the background, so their notifications arrive
/// without the section being opened (the page calls it when the app starts).
#[tauri::command]
pub async fn msg_start(app: AppHandle) -> Result<(), String> {
    let always: Vec<Account> = store::read(|s| s.accounts.iter().filter(|a| a.sleep_after.is_none() && a.mode == "panel").cloned().collect());
    let rect = Rect { x: 0.0, y: 0.0, w: 900.0, h: 600.0 };
    let mut any = false;
    for a in always {
        if !panels::exists_on_main(&app, &a.id) {
            panels::create(&app, Spec { id: a.id.clone(), service: a.service, url: a.url, zoom: a.zoom }, rect)?;
            rt(|r| r.last_used.insert(a.id, Instant::now()));
            any = true;
        }
    }
    if any {
        changed(&app);
    }
    Ok(())
}

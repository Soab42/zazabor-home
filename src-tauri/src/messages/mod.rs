//! Messages: the user's chat services (WhatsApp, Messenger, Teams, Telegram, ...) as their official
//! web apps in panels inside the main window (docs/plans/messages.md). Each account's panel is a
//! child web view with its own data folder; the page reaches Zazabor only through the injected
//! script's reports (`events.rs`), never through Tauri's IPC.

pub mod events;
pub mod panels;
mod spike;

use tauri::AppHandle;

use panels::{Rect, Spec};

pub fn init(app: AppHandle, app_data: std::path::PathBuf) {
    panels::init(&app_data);
    spike::init(app);
}

/// Makes an account's panel (hidden until `msg_show`).
#[tauri::command]
pub async fn msg_create(app: AppHandle, id: String, name: String, url: String, user_agent: Option<String>, rect: Rect) -> Result<(), String> {
    if !url.starts_with("https://") {
        return Err("a service address must be https".into());
    }
    panels::create(&app, Spec { id, name, url, user_agent }, rect, false)
}

/// Shows one account's panel at `rect` and hides the others.
#[tauri::command]
pub async fn msg_show(app: AppHandle, id: String, rect: Rect, others: Vec<String>) -> Result<(), String> {
    for other in others.iter().filter(|o| **o != id) {
        panels::set_visible(&app, other, false)?;
    }
    panels::set_rect(&app, &id, rect)?;
    panels::set_visible(&app, &id, true)
}

#[tauri::command]
pub async fn msg_hide(app: AppHandle, ids: Vec<String>) -> Result<(), String> {
    ids.iter().try_for_each(|id| panels::set_visible(&app, id, false))
}

#[tauri::command]
pub async fn msg_rect(app: AppHandle, id: String, rect: Rect) -> Result<(), String> {
    panels::set_rect(&app, &id, rect)
}

#[tauri::command]
pub async fn msg_reload(app: AppHandle, id: String) -> Result<(), String> {
    panels::reload(&app, &id)
}

#[tauri::command]
pub async fn msg_zoom(app: AppHandle, id: String, factor: f64) -> Result<(), String> {
    panels::zoom(&app, &id, factor)
}

/// Closes an account's panel; `wipe` also deletes its data folder (sign out and forget).
#[tauri::command]
pub async fn msg_destroy(app: AppHandle, id: String, wipe: bool) -> Result<(), String> {
    panels::destroy(&app, &id, wipe)
}

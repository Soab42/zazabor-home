//! Alerts: incoming calls and new messages as cards in the corner of the screen, whatever Zazabor
//! is doing (main window hidden, minimised, or `--background`): a call card rings until it is
//! answered, declined or over; a message toast fades after a few seconds and can reply and call.
//!
//! One small window ("alerts": frameless, transparent, always on top, off the taskbar, shown
//! without taking the keyboard) draws them; this module owns what it shows (`queue.rs`) and what
//! the buttons do. Sources: Phone Link (calls over Bluetooth or the Link app, texts), and the
//! Messages accounts (`messages/events.rs`: notifications, and the call dialogs inject.js sees).
//! Without a working window every source falls back to the desktop's own notification.

mod calls;
mod demo;
mod keys;
pub mod queue;
pub mod store;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::messages::{self, store as msg_store};
pub use calls::{detect, Detected};
use queue::{Alert, Kind, Queue, MAX_VISIBLE};
use store::Settings;

pub const LABEL: &str = "alerts";
const WIDTH: f64 = 408.0;
const MARGIN: f64 = 16.0;
/// The window gets this long to say it is up before alerts go to the desktop's notifications.
const STARTUP_GRACE: Duration = Duration::from_secs(20);

struct Inner {
    queue: Queue,
    version: u64,
    visible: bool,
    started: Instant,
}

static INNER: Mutex<Option<Inner>> = Mutex::new(None);
static APP: OnceLock<AppHandle> = OnceLock::new();
static WINDOW: AtomicBool = AtomicBool::new(false);
static READY: AtomicBool = AtomicBool::new(false);

fn inner() -> MutexGuard<'static, Option<Inner>> {
    INNER.lock().unwrap_or_else(|e| e.into_inner())
}

fn with<T>(f: impl FnOnce(&mut Inner) -> T) -> T {
    let mut guard = inner();
    f(guard.get_or_insert_with(|| Inner { queue: Queue::default(), version: 0, visible: false, started: Instant::now() }))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Whether alerts can be shown in their window (else callers use the desktop's notifications).
pub fn available() -> bool {
    WINDOW.load(Ordering::Relaxed) && (READY.load(Ordering::Relaxed) || with(|i| i.started.elapsed() < STARTUP_GRACE))
}

// ---- the window ---------------------------------------------------------------------------------

/// Makes the (hidden) alerts window and starts the clocks. Called once at startup, also in the
/// background mode.
pub fn init(app: AppHandle, data_dir: std::path::PathBuf) {
    store::init(data_dir.join("alerts.json"));
    let _ = APP.set(app.clone());
    with(|i| i.started = Instant::now());
    match build_window(&app) {
        Ok(()) => WINDOW.store(true, Ordering::Relaxed),
        Err(err) => eprintln!("Alerts: no window, using desktop notifications: {err}"),
    }
    let sweeper = app.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(500));
        let gone = with(|i| i.queue.sweep(Instant::now()));
        if !gone.is_empty() {
            publish(&sweeper);
        }
    });
    demo::start(&app, &data_dir);
}

fn build_window(app: &AppHandle) -> Result<(), String> {
    let start = json!({ "module": "alerts" });
    let window = tauri::WebviewWindowBuilder::new(app, LABEL, tauri::WebviewUrl::App("index.html".into()))
        .title("Alerts — Zazabor Home")
        .inner_size(WIDTH, 60.0)
        // Resizable (there is no frame to grab): a non-resizable GTK window can grow but not shrink.
        .resizable(true)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        .visible(false)
        .shadow(false)
        .transparent(true)
        .background_color(tauri::webview::Color(0, 0, 0, 0))
        .initialization_script(format!("window.__ZAZABOR_WINDOW__ = {start};"))
        .build()
        .map_err(|e| e.to_string())?;
    #[cfg(target_os = "linux")]
    if let Ok(gtk_window) = window.gtk_window() {
        use gtk::prelude::GtkWindowExt;
        // Appearing must not take the keyboard from what the user is typing in; a click does.
        gtk_window.set_focus_on_map(false);
    }
    #[cfg(not(target_os = "linux"))]
    let _ = window;
    Ok(())
}

/// Puts the window in its corner of the monitor with the pointer (else the main one), inside the
/// work area, at `height` CSS pixels, and shows it without focus.
fn layout(app: &AppHandle, height: f64) {
    let Some(window) = app.get_webview_window(LABEL) else { return };
    let height = height.clamp(40.0, 900.0).ceil();
    let cursor = app.cursor_position().ok();
    let monitor = cursor.and_then(|c| app.monitor_from_point(c.x, c.y).ok().flatten()).or_else(|| app.primary_monitor().ok().flatten());
    let (scale, left, top, right, bottom) = match &monitor {
        Some(m) => {
            let a = m.work_area();
            (m.scale_factor(), a.position.x as f64, a.position.y as f64, (a.position.x + a.size.width as i32) as f64, (a.position.y + a.size.height as i32) as f64)
        }
        None => (1.0, 0.0, 0.0, 1920.0, 1080.0),
    };
    let (w, h, m) = (WIDTH * scale, height * scale, MARGIN * scale);
    let position = store::get().position;
    let x = if position.ends_with("left") { left + m } else { right - w - m };
    let y = if position.starts_with("top") { top + m } else { bottom - h - m };
    let (iw, ih) = (WIDTH as i32, height as i32);
    let _ = window.with_webview(move |view| {
        use gtk::prelude::WidgetExt;
        view.inner().set_size_request(iw, ih);
    });
    let _ = window.set_size(tauri::LogicalSize::new(WIDTH, height));
    let _ = window.set_position(tauri::PhysicalPosition::new(x.round() as i32, y.round() as i32));
    if !with(|i| std::mem::replace(&mut i.visible, true)) {
        let _ = window.show();
    }
}

fn hide_window(app: &AppHandle) {
    if with(|i| std::mem::replace(&mut i.visible, false)) {
        if let Some(window) = app.get_webview_window(LABEL) {
            let _ = window.hide();
        }
    }
}

// ---- what the window shows ----------------------------------------------------------------------

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct StateOut {
    version: u64,
    alerts: Vec<Alert>,
    hidden: usize,
    settings: Settings,
}

/// The cards for the window. While Zazabor is locked, who and what stay private.
fn state_out() -> StateOut {
    let locked = crate::lock::is_locked();
    let settings = store::get();
    with(|i| {
        let view = i.queue.view(MAX_VISIBLE);
        let alerts = view
            .cards
            .iter()
            .map(|a| {
                let mut a = (*a).clone();
                if locked {
                    a.title = if a.kind == Kind::Call { "Incoming call".into() } else { "New message".into() };
                    a.body.clear();
                    a.number = None;
                    a.can_reply = false;
                }
                a
            })
            .collect();
        StateOut { version: i.version, alerts, hidden: view.hidden, settings }
    })
}

/// Tells the window what changed (it measures itself and answers with `alerts_resize`).
fn publish(app: &AppHandle) {
    let empty = with(|i| {
        i.version += 1;
        i.queue.is_empty()
    });
    if empty {
        hide_window(app);
    }
    let _ = app.emit_to(LABEL, "alerts-state", state_out());
    update_keys();
}

/// The call keys are held while a call rings.
fn update_keys() {
    let ringing = with(|i| i.queue.calls().next().is_some());
    keys::set_active(ringing && store::get().call_keys, |key| {
        hotkey(if key == keys::Key::Answer { "answer" } else { "decline" });
    });
}

fn push(alert: Alert) -> u64 {
    let Some(app) = APP.get() else { return 0 };
    let (id, changed) = with(|i| i.queue.push(alert, Instant::now()));
    if changed {
        publish(app);
    }
    id
}

fn remove(id: u64) {
    if let Some(app) = APP.get() {
        if with(|i| i.queue.remove(id)) {
            publish(app);
        }
    }
}

fn remove_key(key: &str) {
    if let Some(app) = APP.get() {
        if with(|i| i.queue.remove_key(key)) {
            publish(app);
        }
    }
}

fn set_note(id: u64, note: Option<String>) {
    if let Some(app) = APP.get() {
        with(|i| {
            if let Some(a) = i.queue.get_mut(id) {
                a.note = note;
            }
        });
        publish(app);
    }
}

fn toast_ttl(settings: &Settings) -> Duration {
    Duration::from_secs(settings.toast_secs as u64)
}

/// A card with nothing to press: a hint or a result.
fn info(service: &str, title: &str, body: &str) {
    let settings = store::get();
    let mut alert = Alert::new(Kind::Message, service, title, body);
    alert.at = now_ms();
    alert.ttl = toast_ttl(&settings) + Duration::from_secs(4);
    push(alert);
}

// ---- hooks: Phone Link --------------------------------------------------------------------------

fn phone_call_alert(phone_id: Option<String>, number: &str, name: Option<&str>, waiting: bool, via: &str) -> Alert {
    let settings = store::get();
    let shown = name.filter(|n| !n.is_empty()).unwrap_or(number);
    let mut a = Alert::new(Kind::Call, "phone", if shown.is_empty() { "Unknown caller" } else { shown }, if waiting { "Call waiting" } else { "Incoming call" });
    a.key = "call:phone".into();
    a.via = via.into();
    a.at = now_ms();
    a.number = (!number.is_empty()).then(|| number.to_string());
    a.silent = settings.dnd || !settings.ringtone;
    a.can_reply = phone_id.is_some() && !number.is_empty();
    a.phone_id = phone_id;
    a
}

/// The Bluetooth hands-free link's view: `caller` is the call ringing (number, name, waiting), or
/// None when nothing rings.
pub fn phone_ring(phone_id: Option<String>, caller: Option<(String, Option<String>, bool)>) {
    if APP.get().is_none() || !WINDOW.load(Ordering::Relaxed) || !store::get().phone_calls {
        return;
    }
    match caller {
        Some((number, name, waiting)) => {
            if !available() {
                return;
            }
            let phone_id = phone_id.or_else(crate::phonelink::connected_phone);
            push(phone_call_alert(phone_id, &number, name.as_deref(), waiting, "hfp"));
        }
        None => {
            if let Some(app) = APP.get() {
                if with(|i| i.queue.remove_key_via("call:phone", "hfp")) {
                    publish(app);
                }
            }
        }
    }
}

/// The Link app's `call.state` (with `phoneId`): ringing shows the card, anything else ends it.
pub fn phone_app_call(call: &Value) {
    if APP.get().is_none() || !WINDOW.load(Ordering::Relaxed) || !store::get().phone_calls {
        return;
    }
    let text = |k: &str| call.get(k).and_then(Value::as_str).map(str::to_string);
    if text("state").as_deref() == Some("ringing") {
        // Bluetooth already shows this call, with the sound on the PC.
        if !available() || with(|i| i.queue.find_key("call:phone").is_some_and(|a| a.via == "hfp")) {
            return;
        }
        push(phone_call_alert(text("phoneId"), &text("number").unwrap_or_default(), text("name").as_deref(), false, "app"));
    } else if let Some(app) = APP.get() {
        if with(|i| i.queue.remove_key_via("call:phone", "app")) {
            publish(app);
        }
    }
}

/// A text from a phone. True when it is handled here (shown, or deliberately silent).
pub fn sms(phone_id: &str, from: &str, number: &str, body: &str, thread: &Value) -> bool {
    let settings = store::get();
    if !available() || !settings.sms {
        return false;
    }
    if settings.dnd {
        return true;
    }
    let mut a = Alert::new(Kind::Message, "sms", from, if settings.previews { body } else { "New message" });
    a.at = now_ms();
    a.ttl = toast_ttl(&settings);
    a.phone_id = Some(phone_id.to_string());
    a.number = Some(number.to_string());
    a.addresses = vec![number.to_string()];
    a.thread_id = Some(thread.clone());
    a.can_reply = !number.is_empty();
    a.can_call = !number.is_empty();
    a.can_read = true;
    push(a);
    true
}

// ---- hooks: Messages ----------------------------------------------------------------------------

/// A notification from a Messages account. True when it is handled here.
pub fn web_message(account: &msg_store::Account, title: &str, body: &str) -> bool {
    let settings = store::get();
    if !available() || !settings.source_on(&account.service) {
        return false;
    }
    if settings.dnd {
        return true;
    }
    let previews = settings.previews && msg_store::read(|s| s.settings.previews);
    let mut a = Alert::new(Kind::Message, &account.service, if previews { title } else { "New message" }, if previews { body } else { "" });
    a.at = now_ms();
    a.ttl = toast_ttl(&settings);
    a.account_id = Some(account.id.clone());
    a.account_name = Some(account.name.clone());
    a.can_reply = true;
    a.can_call = true;
    push(a);
    true
}

/// A call ringing in a Messages account (`ringing`), or over. True when it is handled here.
pub fn web_call(account: &msg_store::Account, name: &str, video: bool, ringing: bool) -> bool {
    let settings = store::get();
    let key = format!("call:{}", account.id);
    if !ringing {
        remove_key(&key);
        return true;
    }
    if !available() || !settings.source_on(&account.service) {
        return false;
    }
    let messages_dnd = msg_store::read(|s| s.settings.dnd);
    let who = if name.trim().is_empty() { account.name.as_str() } else { name.trim() };
    let mut a = Alert::new(Kind::Call, &account.service, who, if video { "Incoming video call" } else { "Incoming call" });
    a.key = key;
    a.via = "web".into();
    a.video = video;
    a.at = now_ms();
    a.account_id = Some(account.id.clone());
    a.account_name = Some(account.name.clone());
    a.silent = settings.dnd || messages_dnd || account.muted || !settings.ringtone;
    a.can_reply = true;
    push(a);
    true
}

/// The page closed a notification titled `title`: when that is the ringing call's, the call is over.
pub fn web_call_closed(account: &msg_store::Account, title: &str) {
    let key = format!("call:{}", account.id);
    if with(|i| i.queue.find_key(&key).is_some_and(|a| a.title == title)) {
        remove_key(&key);
    }
}

/// A call that rang and wasn't answered: its card goes, a "missed" toast replaces it.
pub fn web_call_missed(account: &msg_store::Account, name: &str, video: bool) {
    web_call(account, "", false, false);
    let settings = store::get();
    if !available() || !settings.source_on(&account.service) || settings.dnd {
        return;
    }
    let mut a = Alert::new(Kind::Message, &account.service, if name.trim().is_empty() { &account.name } else { name.trim() }, if video { "Missed video call" } else { "Missed call" });
    a.at = now_ms();
    a.ttl = toast_ttl(&settings);
    a.account_id = Some(account.id.clone());
    a.account_name = Some(account.name.clone());
    a.can_reply = true;
    push(a);
}

// ---- what the buttons do ------------------------------------------------------------------------

fn alert_of(id: u64) -> Result<Alert, String> {
    with(|i| i.queue.get(id).cloned()).ok_or_else(|| "That alert is gone.".to_string())
}

/// Shows the main window (making it when it was closed) and, once its page is up, sends `event`.
fn open_in_main(app: &AppHandle, event: &str, payload: Value) {
    let had = app.get_webview_window("main").is_some();
    crate::show_main(app, None);
    let (app, event) = (app.clone(), event.to_string());
    std::thread::spawn(move || {
        if !had {
            std::thread::sleep(Duration::from_secs(3));
        }
        let _ = app.emit(&event, payload);
    });
}

fn open_conversation(app: &AppHandle, a: &Alert) {
    if let Some(id) = &a.account_id {
        open_in_main(app, "msg-open", json!({ "id": id }));
    } else if let Some(phone_id) = &a.phone_id {
        open_in_main(app, "phone-open", json!({ "phoneId": phone_id, "tab": "messages", "threadId": a.thread_id }));
    }
}

/// The account's own Chrome app window (its call window): where a web call is taken, since the
/// panels can't run calls (this WebKitGTK has no WebRTC). Its profile is the account's browser
/// profile, so one sign-in serves browser mode and calls alike.
fn open_call_window(account_id: &str) -> Result<(), String> {
    let account = msg_store::read(|s| s.get(account_id).cloned()).ok_or("That account is gone.")?;
    let profile = messages::panels::browser_profile(account_id)?;
    crate::meet::browser::open(&account.url, &profile)?;
    let settings = store::get();
    if account.mode == "panel" && !settings.call_window_hint_shown {
        let _ = store::set(Settings { call_window_hint_shown: true, ..settings });
        info(&account.service, "Call window", "Sign in once in the call window; after that calls open there directly.");
    }
    Ok(())
}

pub async fn answer(id: u64) -> Result<(), String> {
    let a = alert_of(id)?;
    let app = APP.get().ok_or("Alerts aren't running.")?.clone();
    if a.demo {
        eprintln!("[alerts] demo: answer {}", a.key);
        remove(id);
        return Ok(());
    }
    match a.via.as_str() {
        "web" => {
            let account = a.account_id.clone().ok_or("No account.")?;
            tauri::async_runtime::spawn_blocking(move || open_call_window(&account)).await.map_err(|e| e.to_string())??;
            remove(id);
        }
        _ => {
            if let Err(err) = crate::phonelink::hfp::phone_call_answer(a.phone_id.clone()).await {
                set_note(id, Some(err.clone()));
                return Err(err);
            }
            remove(id);
        }
    }
    let _ = app;
    Ok(())
}

pub async fn decline(id: u64) -> Result<(), String> {
    let a = alert_of(id)?;
    if a.demo {
        eprintln!("[alerts] demo: decline {}", a.key);
        remove(id);
        return Ok(());
    }
    if a.via == "web" {
        // Best effort: the page's own decline button, when its panel is up.
        if let (Some(app), Some(account)) = (APP.get(), &a.account_id) {
            let _ = messages::panels::eval(app, account, "window.__zmsg&&window.__zmsg.callAction&&window.__zmsg.callAction('decline')");
        }
    } else if let Err(err) = crate::phonelink::hfp::phone_call_decline(a.phone_id.clone()).await {
        set_note(id, Some(err.clone()));
        return Err(err);
    }
    remove(id);
    Ok(())
}

async fn send_text(a: &Alert, text: &str) -> Result<(), String> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("Write something first.".into());
    }
    if a.demo {
        eprintln!("[alerts] demo: would send a text of {} characters to {} number(s)", text.chars().count(), a.addresses.len().max(a.number.iter().count()));
        return Ok(());
    }
    let phone_id = a.phone_id.clone().ok_or("No phone.")?;
    let to: Vec<String> = if a.addresses.is_empty() { a.number.iter().cloned().collect() } else { a.addresses.clone() };
    tauri::async_runtime::spawn_blocking(move || crate::phonelink::send_sms(&phone_id, to, &text)).await.map_err(|e| e.to_string())?
}

/// The inline reply of a text: sends it from the phone.
pub async fn reply(id: u64, text: String) -> Result<(), String> {
    let a = alert_of(id)?;
    if a.account_id.is_some() {
        return Err("A chat in Messages can't be answered from here.".into());
    }
    send_text(&a, &text).await?;
    if let (false, Some(phone_id), Some(thread)) = (a.demo, a.phone_id.clone(), a.thread_id.clone()) {
        let _ = tauri::async_runtime::spawn_blocking(move || crate::phonelink::mark_read(&phone_id, thread)).await;
    }
    Ok(())
}

/// "Message" on a call: a text to the caller, then the call is declined. A chat call has no
/// inline text: its chat opens.
pub async fn call_message(id: u64, text: String) -> Result<(), String> {
    let a = alert_of(id)?;
    if a.via == "web" {
        let app = APP.get().ok_or("Alerts aren't running.")?;
        open_conversation(app, &a);
        return decline(id).await;
    }
    send_text(&a, &text).await?;
    decline(id).await
}

/// The toast's Call button: dials a text's sender; for a chat, its chat opens (calls start there).
pub async fn call_back(id: u64) -> Result<(), String> {
    let a = alert_of(id)?;
    let app = APP.get().ok_or("Alerts aren't running.")?.clone();
    if a.demo {
        eprintln!("[alerts] demo: call {:?}", a.number);
        remove(id);
        return Ok(());
    }
    if a.account_id.is_some() {
        open_conversation(&app, &a);
        info(&a.service, "Start the call in the chat", "Open the call window from the chat, or call from your phone.");
    } else {
        let number = a.number.clone().ok_or("No number.")?;
        crate::phonelink::hfp::phone_call_dial(number, a.phone_id.clone()).await?;
    }
    remove(id);
    Ok(())
}

pub async fn mark_read(id: u64) -> Result<(), String> {
    let a = alert_of(id)?;
    if !a.demo {
        if let (Some(phone_id), Some(thread)) = (a.phone_id.clone(), a.thread_id.clone()) {
            tauri::async_runtime::spawn_blocking(move || crate::phonelink::mark_read(&phone_id, thread)).await.map_err(|e| e.to_string())??;
        }
    }
    remove(id);
    Ok(())
}

/// The body of a card was clicked: the conversation opens in Zazabor.
pub fn open(id: u64) -> Result<(), String> {
    let a = alert_of(id)?;
    let app = APP.get().ok_or("Alerts aren't running.")?;
    if !a.demo {
        open_conversation(app, &a);
    }
    if a.kind == Kind::Message {
        remove(id);
    }
    Ok(())
}

/// Ctrl+Alt+A / Ctrl+Alt+D: the first ringing call.
pub fn hotkey(action: &str) {
    let id = with(|i| i.queue.calls().next().map(|a| a.id));
    let Some(id) = id else { return };
    let action = action.to_string();
    tauri::async_runtime::spawn(async move {
        let result = if action == "answer" { answer(id).await } else { decline(id).await };
        if let Err(err) = result {
            eprintln!("[alerts] {action}: {err}");
        }
    });
}

// ---- commands -----------------------------------------------------------------------------------

#[tauri::command]
pub fn alerts_state() -> StateOut {
    state_out()
}

/// The window's page is up.
#[tauri::command]
pub fn alerts_ready() {
    READY.store(true, Ordering::Relaxed);
}

/// The window measured its cards: place it and show it (or hide it when nothing is left).
#[tauri::command]
pub fn alerts_resize(app: AppHandle, height: f64) {
    if with(|i| i.queue.is_empty()) {
        hide_window(&app);
    } else {
        layout(&app, height);
    }
}

#[tauri::command]
pub fn alerts_hover(id: u64, on: bool) {
    with(|i| i.queue.hover(id, on, Instant::now()));
}

#[tauri::command]
pub fn alerts_dismiss(id: u64) {
    remove(id);
}

#[tauri::command]
pub async fn alerts_answer(id: u64) -> Result<(), String> {
    answer(id).await
}

#[tauri::command]
pub async fn alerts_decline(id: u64) -> Result<(), String> {
    decline(id).await
}

#[tauri::command]
pub async fn alerts_reply(id: u64, text: String) -> Result<(), String> {
    reply(id, text).await
}

#[tauri::command]
pub async fn alerts_call_message(id: u64, text: String) -> Result<(), String> {
    call_message(id, text).await
}

#[tauri::command]
pub async fn alerts_call(id: u64) -> Result<(), String> {
    call_back(id).await
}

#[tauri::command]
pub async fn alerts_read(id: u64) -> Result<(), String> {
    mark_read(id).await
}

#[tauri::command]
pub fn alerts_open(id: u64) -> Result<(), String> {
    open(id)
}

#[tauri::command]
pub fn alerts_hotkey(action: String) {
    if matches!(action.as_str(), "answer" | "decline") {
        hotkey(&action);
    }
}

#[tauri::command]
pub fn alerts_settings() -> Settings {
    store::get()
}

#[tauri::command]
pub fn alerts_set_settings(app: AppHandle, settings: Settings) -> Result<Settings, String> {
    let saved = store::set(settings)?;
    let _ = app.emit("alerts-settings", &saved);
    publish(&app);
    Ok(saved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_phone_call_card_names_the_caller() {
        let a = phone_call_alert(Some("p1".into()), "+8801700000000", Some("Ann"), false, "hfp");
        assert_eq!((a.title.as_str(), a.body.as_str(), a.can_reply), ("Ann", "Incoming call", true));
        let b = phone_call_alert(None, "+8801700000000", None, true, "app");
        assert_eq!((b.title.as_str(), b.body.as_str(), b.can_reply), ("+8801700000000", "Call waiting", false));
    }
}

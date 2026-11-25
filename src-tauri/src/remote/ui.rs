//! What the PC's user sees of a remote session: the question "<phone> wants to control this PC"
//! and, while it runs, a bright pill with a Stop button, on every monitor. Both are small
//! frameless always-on-top windows ("remote-ui-N", one per monitor, made when needed and gone
//! when the session ends) drawn by `src/remote/window.ts`; this module owns what they show.

use std::sync::mpsc;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

pub const PREFIX: &str = "remote-ui-";
const ASK_WINDOW: (f64, f64) = (460.0, 152.0);
/// The running session's sign: a small badge with Stop in each monitor's top-right corner.
const LIVE_WINDOW: (f64, f64) = (108.0, 56.0);
/// How long the question waits before it counts as "no".
const ASK_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Once,
    Always,
    Deny,
}

impl Choice {
    pub fn parse(s: &str) -> Choice {
        match s {
            "once" => Choice::Once,
            "always" => Choice::Always,
            _ => Choice::Deny,
        }
    }
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UiState {
    /// "idle", "ask" or "live".
    pub mode: &'static str,
    pub phone: String,
    pub view_only: bool,
    /// What the question asks for: "control" (the screen) or "camera".
    pub purpose: &'static str,
    /// The phone is on the internet path, not this network.
    pub internet: bool,
    /// The phone sees the PC's camera / hears its microphone right now.
    pub camera: bool,
    pub mic: bool,
}

struct Inner {
    state: UiState,
    answer: Option<mpsc::Sender<Choice>>,
}

static INNER: Mutex<Inner> = Mutex::new(Inner { state: UiState { mode: "idle", phone: String::new(), view_only: false, purpose: "", internet: false, camera: false, mic: false }, answer: None });
static APP: OnceLock<AppHandle> = OnceLock::new();

fn inner() -> MutexGuard<'static, Inner> {
    INNER.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn init(app: AppHandle) {
    let _ = APP.set(app);
}

pub fn is_ui(label: &str) -> bool {
    label.starts_with(PREFIX)
}

/// Shows the question and waits for the answer (no answer in a minute is a no).
pub fn ask(phone: &str, purpose: &'static str) -> Choice {
    let (tx, rx) = mpsc::channel();
    {
        let mut i = inner();
        i.answer = Some(tx);
        i.state = UiState { mode: "ask", phone: phone.to_string(), purpose, ..UiState::default() };
    }
    show();
    let choice = rx.recv_timeout(ASK_TIMEOUT).unwrap_or(Choice::Deny);
    inner().answer = None;
    if choice == Choice::Deny {
        hide();
    }
    choice
}

/// The pill for a running session.
pub fn live(phone: &str, view_only: bool, internet: bool) {
    inner().state = UiState { mode: "live", phone: phone.to_string(), view_only, internet, ..UiState::default() };
    show();
}

/// The camera or the microphone went on or off: the pill says so (it can't be hidden).
pub fn media(camera: bool, mic: bool) {
    {
        let mut i = inner();
        if i.state.mode != "live" || (i.state.camera == camera && i.state.mic == mic) {
            return;
        }
        i.state.camera = camera;
        i.state.mic = mic;
    }
    show();
}

pub fn hide() {
    inner().state = UiState::default();
    let Some(app) = APP.get() else { return };
    for (label, window) in app.webview_windows() {
        if is_ui(&label) {
            let _ = window.destroy();
        }
    }
}

pub fn state() -> UiState {
    inner().state.clone()
}

pub fn answer(choice: Choice) {
    if let Some(tx) = inner().answer.take() {
        let _ = tx.send(choice);
    }
}

/// Makes (or re-places) one window per monitor for the current state and tells them about it.
fn show() {
    let Some(app) = APP.get() else { return };
    let state = state();
    // The badge grows by an icon for the camera, the microphone and the internet.
    let extra = [state.camera, state.mic, state.internet].iter().filter(|b| **b).count() as f64 * 28.0;
    let (w, h) = if state.mode == "ask" { ASK_WINDOW } else { (LIVE_WINDOW.0 + extra, LIVE_WINDOW.1) };
    let monitors = app.available_monitors().unwrap_or_default();
    let count = monitors.len().max(1);
    for i in 0..count {
        let label = format!("{PREFIX}{i}");
        let window = match app.get_webview_window(&label) {
            Some(window) => window,
            None => {
                let start = serde_json::json!({ "module": "remote-ui" });
                let built = tauri::WebviewWindowBuilder::new(app, &label, tauri::WebviewUrl::App("index.html".into()))
                    .title("Remote control — Zazabor Home")
                    .inner_size(w, h)
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
                    .build();
                match built {
                    Ok(window) => {
                        #[cfg(target_os = "linux")]
                        if let Ok(gtk_window) = window.gtk_window() {
                            use gtk::prelude::GtkWindowExt;
                            gtk_window.set_focus_on_map(false);
                        }
                        window
                    }
                    Err(err) => {
                        eprintln!("Remote: no window for the indicator: {err}");
                        continue;
                    }
                }
            }
        };
        let _ = window.set_size(tauri::LogicalSize::new(w, h));
        if let Some(m) = monitors.get(i) {
            let scale = m.scale_factor();
            // The question sits at the top centre; the running session's badge in the top-right
            // corner, below the desktop's top bar, out of the way.
            let x = if state.mode == "ask" {
                m.position().x + ((m.size().width as f64 - w * scale) / 2.0).round() as i32
            } else {
                m.position().x + (m.size().width as f64 - (w + 12.0) * scale).round() as i32
            };
            let y = m.position().y + (if state.mode == "ask" { 28.0 } else { 40.0 } * scale).round() as i32;
            let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
        }
        let _ = window.set_always_on_top(true);
        let _ = window.show();
    }
    let _ = app.emit("remote-ui-state", &state);
}

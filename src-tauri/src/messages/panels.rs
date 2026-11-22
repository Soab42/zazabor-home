//! The panels: one web view per account inside the main window, each with its own data
//! directory (so its own logins), placed over the Messages content area.
//!
//! They are plain WebKitGTK views, not Tauri webviews. Tauri's `Window::add_child` would put a
//! second webview in the main window, and then `get_webview_window("main")` (used all over the
//! app) returns None, since a window with several webviews is no longer a "webview window". And
//! on Linux `add_child` only packs the child beside the main web view, it doesn't place it.
//! So the main web view is moved into a GtkOverlay and each panel floats above it at the
//! rectangle the page reports. That gives the z-order (native web views draw above the page) and
//! keeps keyboard focus working. The panels talk to Zazabor only through inject.js.
//!
//! GTK runs on the main thread: everything here goes through `on_main`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, OnceLock};
use std::time::Duration;

use gtk::prelude::*;
use javascriptcore::ValueExt;
use serde::Deserialize;
use tauri::{AppHandle, Manager};
use webkit2gtk::{
    CookieManagerExt, CookiePersistentStorage, NavigationPolicyDecision, NavigationPolicyDecisionExt, PermissionRequestExt, PolicyDecisionExt, PolicyDecisionType, SettingsExt, URIRequestExt, UserContentInjectedFrames,
    UserContentManager, UserContentManagerExt, UserScript, UserScriptInjectionTime, WebContext, WebView, WebViewExt, WebsiteDataManager, WebsiteDataManagerExt,
};

use super::events;

pub const MAIN: &str = "main";

/// A rectangle in the main window, in CSS pixels from its top-left corner.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// A current Safari on macOS: WhatsApp, Teams and others refuse an unknown WebKitGTK as "outdated".
pub const SAFARI_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.1 Safari/605.1.15";

pub struct Spec {
    pub id: String,
    /// Shown in notifications.
    pub name: String,
    pub url: String,
    pub user_agent: Option<String>,
}

struct Panel {
    view: WebView,
    name: String,
    rect: Rect,
}

thread_local! {
    /// The panels by account id (GTK objects stay on the main thread).
    static PANELS: RefCell<HashMap<String, Panel>> = RefCell::new(HashMap::new());
}

static DATA_ROOT: OnceLock<PathBuf> = OnceLock::new();

pub fn init(app_data: &Path) {
    let _ = DATA_ROOT.set(app_data.join("messages"));
}

/// An account id becomes a folder name, so it stays plain.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 40 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The account's own data folder: cookies, local storage, IndexedDB, caches.
pub fn data_dir(id: &str) -> Result<PathBuf, String> {
    if !valid_id(id) {
        return Err(format!("bad account id {id:?}"));
    }
    Ok(DATA_ROOT.get().ok_or("messages not started")?.join(id))
}

/// Runs `f` on the main thread and waits for its answer. Never call it from the main thread.
fn on_main<T: Send + 'static>(app: &AppHandle, f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    let (tx, rx) = mpsc::channel();
    app.run_on_main_thread(move || {
        let _ = tx.send(f());
    })
    .map_err(|e| e.to_string())?;
    rx.recv_timeout(Duration::from_secs(10)).map_err(|_| "the main thread didn't answer".to_string())?
}

fn with_panel<T>(id: &str, f: impl FnOnce(&mut Panel) -> T) -> Result<T, String> {
    PANELS.with(|p| p.borrow_mut().get_mut(id).map(f)).ok_or_else(|| format!("no panel {id}"))
}

/// Whether the panel itself may go to this address.
fn navigation_allowed(id: &str, uri: &str) -> bool {
    let Ok(url) = uri.parse::<tauri::Url>() else { return false };
    let allowed = matches!(url.scheme(), "http" | "https" | "about" | "blob" | "data");
    if !allowed {
        eprintln!("[messages] {id}: refused a {}: navigation", url.scheme());
    }
    allowed
}

/// A pop-up or `target=_blank` link goes to the system browser, never into the panel.
fn open_outside(id: &str, uri: &str) {
    // Only the site: a link from a chat can carry private details in its path or query.
    let site = uri.parse::<tauri::Url>().ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_else(|| "?".into());
    eprintln!("[messages] {id}: new window on {site}");
    if uri.starts_with("http://") || uri.starts_with("https://") {
        let _ = crate::open_with_system(std::ffi::OsStr::new(uri));
    }
}

/// The overlay that holds the main web view, made the first time a panel arrives.
fn overlay(app: &AppHandle) -> Result<gtk::Overlay, String> {
    let vbox = app.get_webview_window(MAIN).ok_or("no main window")?.default_vbox().map_err(|e| e.to_string())?;
    let children = vbox.children();
    if let Some(o) = children.iter().find_map(|c| c.clone().downcast::<gtk::Overlay>().ok()) {
        return Ok(o);
    }
    let main = children.into_iter().next().ok_or("the main window is empty")?;
    vbox.remove(&main);
    let overlay = gtk::Overlay::new();
    overlay.add(&main);
    // Where each panel goes: the rectangle the page reported.
    overlay.connect_get_child_position(|_, child| {
        PANELS.with(|p| p.borrow().values().find(|p| p.view.upcast_ref::<gtk::Widget>() == child).map(|p| p.rect)).map(|r| gtk::Allocation::new(r.x as i32, r.y as i32, r.w as i32, r.h as i32))
    });
    vbox.pack_start(&overlay, true, true, 0);
    vbox.reorder_child(&overlay, 0);
    overlay.show_all();
    Ok(overlay)
}

fn build_view(app: &AppHandle, spec: &Spec, dir: &Path) -> Result<WebView, String> {
    // Its own data manager: cookies, local storage, IndexedDB and caches never mix with another
    // account's or with Zazabor's own pages.
    let manager = WebsiteDataManager::builder().base_cache_directory(dir.to_string_lossy()).base_data_directory(dir.to_string_lossy()).build();
    if let Some(cookies) = manager.cookie_manager() {
        cookies.set_persistent_storage(&dir.join("cookies.sqlite").to_string_lossy(), CookiePersistentStorage::Sqlite);
    }
    let context = WebContext::builder().website_data_manager(&manager).build();
    let content = UserContentManager::new();
    // Spike switches: ZAZABOR_MSG_OFF = "all", or some of "notify,sw,title", leaves shims out.
    let off = std::env::var("ZAZABOR_MSG_OFF").unwrap_or_default();
    let script = include_str!("inject.js").replace("__OFF__", &off);
    if off != "all" {
        content.add_script(&UserScript::new(&script, UserContentInjectedFrames::TopFrame, UserScriptInjectionTime::Start, &[], &[]));
    }
    // The page's reports (inject.js) arrive here; they are untrusted and validated in events.rs.
    content.register_script_message_handler("zmsg");
    let (report_app, report_id) = (app.clone(), spec.id.clone());
    content.connect_script_message_received(Some("zmsg"), move |_, result| {
        let Some(text) = result.js_value().filter(|v| v.is_string()).map(|v| v.to_str().to_string()) else { return };
        if let Some(event) = events::parse_json(&text) {
            let name = PANELS.with(|p| p.borrow().get(&report_id).map(|p| p.name.clone())).unwrap_or_else(|| report_id.clone());
            events::deliver(&report_app, &report_id, &name, event);
        }
    });
    let view = WebView::builder().web_context(&context).user_content_manager(&content).build();
    if let Some(settings) = WebViewExt::settings(&view) {
        settings.set_user_agent(spec.user_agent.as_deref());
        settings.set_enable_webgl(true);
        settings.set_enable_webaudio(true);
        settings.set_enable_page_cache(true);
        settings.set_javascript_can_open_windows_automatically(true);
        settings.set_enable_developer_extras(cfg!(debug_assertions));
        settings.set_enable_write_console_messages_to_stdout(std::env::var_os("ZAZABOR_MSG_SPIKE").is_some());
    }
    let nav_id = spec.id.clone();
    view.connect_decide_policy(move |_, decision, kind| {
        let uri = match kind {
            PolicyDecisionType::NavigationAction | PolicyDecisionType::NewWindowAction => decision
                .dynamic_cast_ref::<NavigationPolicyDecision>()
                .and_then(|d| d.navigation_action())
                .and_then(|a| a.request())
                .and_then(|r| r.uri()),
            _ => return false,
        };
        let Some(uri) = uri else { return false };
        if kind == PolicyDecisionType::NewWindowAction {
            open_outside(&nav_id, &uri);
            decision.ignore();
        } else if navigation_allowed(&nav_id, &uri) {
            decision.use_();
        } else {
            decision.ignore();
        }
        true
    });
    let create_id = spec.id.clone();
    view.connect_create(move |_, action| {
        if let Some(uri) = action.clone().request().and_then(|r| r.uri()) {
            open_outside(&create_id, &uri);
        }
        None
    });
    // Camera, microphone, screen and location are never granted: calls go to the browser.
    view.connect_permission_request(|_, request| {
        request.deny();
        true
    });
    Ok(view)
}

/// Makes a panel at `rect`, shown or hidden, and starts loading its page.
pub fn create(app: &AppHandle, spec: Spec, rect: Rect, shown: bool) -> Result<(), String> {
    let dir = data_dir(&spec.id)?;
    if !spec.url.starts_with("https://") && !spec.url.starts_with("http://127.0.0.1") {
        return Err("a panel address must be https".into());
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let app2 = app.clone();
    on_main(app, move || {
        if PANELS.with(|p| p.borrow().contains_key(&spec.id)) {
            return Err(format!("panel {} exists", spec.id));
        }
        let overlay = overlay(&app2)?;
        let view = build_view(&app2, &spec, &dir)?;
        view.set_size_request(rect.w as i32, rect.h as i32);
        overlay.add_overlay(&view);
        overlay.set_overlay_pass_through(&view, false);
        PANELS.with(|p| p.borrow_mut().insert(spec.id.clone(), Panel { view: view.clone(), name: spec.name.clone(), rect }));
        if shown {
            view.show();
        }
        view.load_uri(&spec.url);
        Ok(())
    })
}

/// Moves and resizes a panel (the page's content area changed).
pub fn set_rect(app: &AppHandle, id: &str, rect: Rect) -> Result<(), String> {
    let id = id.to_string();
    on_main(app, move || {
        with_panel(&id, |p| {
            p.rect = rect;
            p.view.set_size_request(rect.w as i32, rect.h as i32);
            if let Some(parent) = p.view.parent() {
                parent.queue_resize();
            }
        })
    })
}

pub fn set_visible(app: &AppHandle, id: &str, visible: bool) -> Result<(), String> {
    let id = id.to_string();
    on_main(app, move || with_panel(&id, |p| if visible { p.view.show() } else { p.view.hide() }))
}

pub fn reload(app: &AppHandle, id: &str) -> Result<(), String> {
    let id = id.to_string();
    on_main(app, move || with_panel(&id, |p| p.view.reload()))
}

pub fn eval(app: &AppHandle, id: &str, js: &str) -> Result<(), String> {
    let (id, js) = (id.to_string(), js.to_string());
    on_main(app, move || with_panel(&id, |p| p.view.evaluate_javascript(&js, None, None, gio::Cancellable::NONE, |_| {})))
}

pub fn zoom(app: &AppHandle, id: &str, factor: f64) -> Result<(), String> {
    let id = id.to_string();
    on_main(app, move || with_panel(&id, |p| p.view.set_zoom_level(factor.clamp(0.5, 3.0))))
}

pub fn focus(app: &AppHandle, id: &str) -> Result<(), String> {
    let id = id.to_string();
    on_main(app, move || with_panel(&id, |p| p.view.grab_focus()))
}

/// Closes a panel; with `wipe` its data folder is deleted too (sign out and forget).
pub fn destroy(app: &AppHandle, id: &str, wipe: bool) -> Result<(), String> {
    let dir = data_dir(id)?;
    let key = id.to_string();
    on_main(app, move || {
        if let Some(panel) = PANELS.with(|p| p.borrow_mut().remove(&key)) {
            if let Some(parent) = panel.view.parent().and_then(|p| p.downcast::<gtk::Container>().ok()) {
                parent.remove(&panel.view);
            }
        }
        Ok(())
    })?;
    if wipe {
        // The web processes let go of the folder asynchronously; try for a moment.
        for _ in 0..20 {
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => return Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(_) => std::thread::sleep(Duration::from_millis(150)),
            }
        }
        return std::fs::remove_dir_all(&dir).map_err(|e| e.to_string());
    }
    Ok(())
}

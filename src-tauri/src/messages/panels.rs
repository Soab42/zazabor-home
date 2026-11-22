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
use tauri::Emitter;
use webkit2gtk::{
    CookieManagerExt, CookiePersistentStorage, DownloadExt, NavigationPolicyDecision, NavigationPolicyDecisionExt, NavigationType, PermissionRequestExt, PolicyDecisionExt, PolicyDecisionType, SettingsExt,
    SnapshotOptions, SnapshotRegion, URIRequestExt, UserContentInjectedFrames, UserContentManager, UserContentManagerExt, UserScript, UserScriptInjectionTime, WebContext, WebContextExt, WebView, WebViewExt,
    WebsiteDataManager, WebsiteDataManagerExt,
};

use super::{events, services, store};

pub const MAIN: &str = "main";

/// A rectangle in the main window, in CSS pixels from its top-left corner.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

pub struct Spec {
    pub id: String,
    pub service: String,
    pub url: String,
    pub zoom: f64,
    /// "light", "dark" or "system" (store::valid_appearance).
    pub appearance: String,
}

struct Panel {
    view: WebView,
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

/// The profile folder of an account's browser window (beside its panel data, same wipe).
pub fn browser_profile(id: &str) -> Result<PathBuf, String> {
    data_dir(id).map(|d| d.with_file_name(format!("{id}-browser")))
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

/// Sends an address to the system browser. Only the site is logged: a link from a chat can carry
/// private details in its path or query.
fn open_outside(id: &str, app: &AppHandle, service: &str, uri: &str) {
    let site = services::host_of(uri).unwrap_or_else(|| "?".into());
    eprintln!("[messages] {id}: opening {site} in the browser");
    if uri.starts_with("http://") || uri.starts_with("https://") {
        let _ = crate::open_with_system(std::ffi::OsStr::new(uri));
        if services::is_call(service, uri) {
            super::hint(app, "Calls open in your browser. Sign in there once and they work.");
        }
    }
}

/// Program files from a chat are refused: they could be anything.
fn risky_file(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    ["exe", "msi", "bat", "cmd", "com", "scr", "sh", "run", "appimage", "deb", "rpm", "jar", "ps1", "vbs", "apk", "desktop"].iter().any(|e| lower.ends_with(&format!(".{e}")))
}

fn downloads_dir() -> PathBuf {
    gtk::glib::user_special_dir(gtk::glib::UserDirectory::Downloads).unwrap_or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Downloads")).unwrap_or_else(std::env::temp_dir))
}

/// A file name that is safe to create in a folder: no path parts, not hidden, not taken.
pub fn download_target(dir: &Path, suggested: &str) -> PathBuf {
    let base = suggested.rsplit(['/', '\\']).next().unwrap_or("").trim().trim_start_matches('.');
    let base: String = base.chars().filter(|c| !c.is_control()).collect();
    let base = if base.is_empty() { "download".to_string() } else { base };
    let (stem, ext) = match base.rfind('.') {
        Some(i) if i > 0 => (base[..i].to_string(), base[i..].to_string()),
        _ => (base.clone(), String::new()),
    };
    let mut path = dir.join(&base);
    let mut n = 1;
    while path.exists() {
        path = dir.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    path
}

/// Downloads from every panel of this context go to ~/Downloads, with a toast from the page.
fn watch_downloads(app: &AppHandle, context: &WebContext) {
    let app = app.clone();
    context.connect_download_started(move |_, download| {
        let name = std::rc::Rc::new(RefCell::new(String::new()));
        let failed = std::rc::Rc::new(std::cell::Cell::new(false));
        let (app1, name1) = (app.clone(), name.clone());
        download.connect_decide_destination(move |d, suggested| {
            if risky_file(suggested) {
                d.cancel();
                let _ = app1.emit("msg-download", serde_json::json!({ "ok": false, "name": suggested, "reason": "program" }));
                return true;
            }
            let target = download_target(&downloads_dir(), suggested);
            let _ = std::fs::create_dir_all(downloads_dir());
            *name1.borrow_mut() = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            d.set_destination(&gio::File::for_path(&target).uri());
            true
        });
        let (app2, failed2) = (app.clone(), failed.clone());
        download.connect_failed(move |_, _| {
            failed2.set(true);
            let _ = app2.emit("msg-download", serde_json::json!({ "ok": false, "name": "", "reason": "failed" }));
        });
        let app3 = app.clone();
        download.connect_finished(move |_| {
            if !failed.get() && !name.borrow().is_empty() {
                let _ = app3.emit("msg-download", serde_json::json!({ "ok": true, "name": &*name.borrow() }));
            }
        });
    });
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
    if std::env::var_os("ZAZABOR_MSG_DEBUG").is_some() {
        gtk::glib::timeout_add_seconds_local(1, debug_dump);
    }
    Ok(overlay)
}

/// ZAZABOR_MSG_DEBUG: prints the state of every panel's widget whenever it changes.
fn debug_dump() -> gtk::glib::ControlFlow {
    thread_local!(static LAST: RefCell<String> = RefCell::new(String::new()));
    let mut lines: Vec<String> = PANELS.with(|p| {
        p.borrow()
            .iter()
            .map(|(id, panel)| {
                let w = panel.view.upcast_ref::<gtk::Widget>();
                let a = w.allocation();
                format!("{id}: visible={} mapped={} focus={} alloc={},{} {}x{} rect={:?}", w.is_visible(), w.is_mapped(), w.has_focus(), a.x(), a.y(), a.width(), a.height(), panel.rect)
            })
            .collect()
    });
    lines.sort();
    let text = lines.join(" | ");
    LAST.with(|l| {
        if *l.borrow() != text {
            eprintln!("[msgdbg] {text}");
            *l.borrow_mut() = text;
        }
    });
    gtk::glib::ControlFlow::Continue
}

fn build_view(app: &AppHandle, spec: &Spec, dir: &Path) -> Result<WebView, String> {
    // Its own data manager: cookies, local storage, IndexedDB and caches never mix with another
    // account's or with Zazabor's own pages.
    let manager = WebsiteDataManager::builder().base_cache_directory(dir.to_string_lossy()).base_data_directory(dir.to_string_lossy()).build();
    if let Some(cookies) = manager.cookie_manager() {
        cookies.set_persistent_storage(&dir.join("cookies.sqlite").to_string_lossy(), CookiePersistentStorage::Sqlite);
    }
    let context = WebContext::builder().website_data_manager(&manager).build();
    watch_downloads(app, &context);
    let content = UserContentManager::new();
    // The colour scheme the page sees (inject.js reads it). Only the three checked values ever get here.
    let scheme = if super::store::valid_appearance(&spec.appearance) { spec.appearance.as_str() } else { "light" };
    content.add_script(&UserScript::new(&format!("window.__zmsgScheme = \"{scheme}\";"), UserContentInjectedFrames::AllFrames, UserScriptInjectionTime::Start, &[], &[]));
    content.add_script(&UserScript::new(include_str!("inject.js"), UserContentInjectedFrames::TopFrame, UserScriptInjectionTime::Start, &[], &[]));
    // The page's reports (inject.js) arrive here; they are untrusted and validated in events.rs.
    content.register_script_message_handler("zmsg");
    let (report_app, report_id) = (app.clone(), spec.id.clone());
    content.connect_script_message_received(Some("zmsg"), move |_, result| {
        let Some(text) = result.js_value().filter(|v| v.is_string()).map(|v| v.to_str().to_string()) else { return };
        if let Some(event) = events::parse_json(&text) {
            events::deliver(&report_app, &report_id, event);
        }
    });
    let view = WebView::builder().web_context(&context).user_content_manager(&content).build();
    if let Some(settings) = WebViewExt::settings(&view) {
        settings.set_enable_webgl(true);
        settings.set_enable_webaudio(true);
        settings.set_enable_page_cache(true);
        settings.set_javascript_can_open_windows_automatically(true);
        settings.set_enable_developer_extras(cfg!(debug_assertions));
    }
    view.set_zoom_level(spec.zoom.clamp(store::MIN_ZOOM, store::MAX_ZOOM));
    // A click into the panel gives it the keyboard focus (typing must reach it, not the page).
    view.connect_button_press_event(|view, _| {
        if !view.has_focus() {
            view.grab_focus();
        }
        gtk::glib::Propagation::Proceed
    });
    let (service, home) = (spec.service.clone(), spec.url.clone());
    let (nav_app, nav_id) = (app.clone(), spec.id.clone());
    view.connect_decide_policy(move |_, decision, kind| {
        let (uri, link) = match kind {
            PolicyDecisionType::NavigationAction | PolicyDecisionType::NewWindowAction => {
                let action = decision.dynamic_cast_ref::<NavigationPolicyDecision>().and_then(|d| d.navigation_action());
                let link = action.as_ref().is_some_and(|a| a.navigation_type() == NavigationType::LinkClicked);
                (action.and_then(|a| a.request()).and_then(|r| r.uri()), link)
            }
            _ => return false,
        };
        let Some(uri) = uri.map(|u| u.to_string()) else { return false };
        let scheme_ok = ["http://", "https://", "about:", "blob:", "data:"].iter().any(|p| uri.starts_with(p));
        if !scheme_ok {
            eprintln!("[messages] {nav_id}: refused a navigation to an unusual scheme");
            decision.ignore();
        } else if kind == PolicyDecisionType::NewWindowAction || (link && !services::stays_inside(&service, &home, &uri)) {
            // A pop-up or a link to another site goes to the browser. A pop-up on the service's own
            // site (a login window) is loaded in the panel instead, see connect_create.
            if kind == PolicyDecisionType::NewWindowAction && services::stays_inside(&service, &home, &uri) && !services::is_call(&service, &uri) {
                decision.use_();
            } else {
                open_outside(&nav_id, &nav_app, &service, &uri);
                decision.ignore();
            }
        } else {
            decision.use_();
        }
        true
    });
    let (create_app, create_id, create_service, create_home) = (app.clone(), spec.id.clone(), spec.service.clone(), spec.url.clone());
    view.connect_create(move |view, action| {
        if let Some(uri) = action.clone().request().and_then(|r| r.uri()) {
            let uri = uri.to_string();
            if services::stays_inside(&create_service, &create_home, &uri) && !services::is_call(&create_service, &uri) {
                view.load_uri(&uri);
            } else {
                open_outside(&create_id, &create_app, &create_service, &uri);
            }
        }
        None
    });
    // Camera, microphone, screen and location are never granted: calls go to the browser.
    let perm_app = app.clone();
    view.connect_permission_request(move |_, request| {
        request.deny();
        super::hint(&perm_app, "Calls open in your browser. Sign in there once and they work.");
        true
    });
    // A crashed web process comes back by reloading.
    let crash_id = spec.id.clone();
    view.connect_web_process_terminated(move |view, _| {
        eprintln!("[messages] {crash_id}: the web process ended, reloading");
        view.reload();
    });
    Ok(view)
}

/// Makes a panel at `rect`, hidden, and starts loading its page.
pub fn create(app: &AppHandle, spec: Spec, rect: Rect) -> Result<(), String> {
    let dir = data_dir(&spec.id)?;
    if !services::url_ok(&spec.service, &spec.url) {
        return Err("a panel address must be https".into());
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let app2 = app.clone();
    on_main(app, move || {
        if PANELS.with(|p| p.borrow().contains_key(&spec.id)) {
            return Ok(());
        }
        let overlay = overlay(&app2)?;
        let view = build_view(&app2, &spec, &dir)?;
        view.set_size_request(rect.w as i32, rect.h as i32);
        overlay.add_overlay(&view);
        overlay.set_overlay_pass_through(&view, false);
        view.set_no_show_all(true);
        view.hide();
        PANELS.with(|p| p.borrow_mut().insert(spec.id.clone(), Panel { view: view.clone(), rect }));
        view.load_uri(&spec.url);
        Ok(())
    })
}

/// The ids of the panels that are alive. Call it on the main thread only.
fn alive_ids() -> Vec<String> {
    PANELS.with(|p| p.borrow().keys().cloned().collect())
}

pub fn exists_on_main(app: &AppHandle, id: &str) -> bool {
    alive(app).iter().any(|a| a == id)
}

pub fn alive(app: &AppHandle) -> Vec<String> {
    on_main(app, || Ok(alive_ids())).unwrap_or_default()
}

/// Moves and resizes a panel (the page's content area changed).
pub fn set_rect(app: &AppHandle, id: &str, rect: Rect) -> Result<(), String> {
    let id = id.to_string();
    on_main(app, move || {
        with_panel(&id, |p| {
            // The page says it again every couple of seconds: nothing to do when it hasn't moved.
            if p.rect == rect && p.view.is_visible() {
                return;
            }
            p.rect = rect;
            p.view.set_size_request(rect.w as i32, rect.h as i32);
            if let Some(parent) = p.view.parent() {
                parent.queue_resize();
            }
        })
    })
}

/// Gives the keyboard focus back to the main web view (the page: sheets, menus, shortcuts). A hidden
/// or closed panel that had the focus leaves the window with none, and then keys, Escape included,
/// reach nothing.
fn focus_main(overlay: &gtk::Overlay) {
    if let Some(main) = overlay.child() {
        main.grab_focus();
    }
}

/// Shows this panel (with the keyboard focus) and hides every other; None hides them all.
pub fn show_only(app: &AppHandle, id: Option<&str>) -> Result<(), String> {
    let id = id.map(str::to_string);
    on_main(app, move || {
        PANELS.with(|p| {
            for (key, panel) in p.borrow().iter() {
                if Some(key) == id.as_ref() {
                    // Whenever the page asks (it repeats the request now and then), the panel ends up
                    // shown, on top and placed. It takes the keyboard focus only when it comes up, so
                    // a repeat never pulls the focus away from the page.
                    let was_up = panel.view.is_visible();
                    panel.view.show();
                    if let Some(window) = panel.view.window() {
                        window.raise();
                    }
                    if !was_up {
                        panel.view.queue_resize();
                        panel.view.queue_draw();
                        panel.view.grab_focus();
                    }
                } else {
                    let had_focus = panel.view.has_focus();
                    panel.view.hide();
                    if had_focus && id.is_none() {
                        if let Some(overlay) = panel.view.parent().and_then(|p| p.downcast::<gtk::Overlay>().ok()) {
                            focus_main(&overlay);
                        }
                    }
                }
            }
        });
        Ok(())
    })
}

/// Runs a script in a panel (the alerts' "decline" asks the page to click its own button).
#[allow(deprecated)]
pub fn eval(app: &AppHandle, id: &str, script: &str) -> Result<(), String> {
    let (id, script) = (id.to_string(), script.to_string());
    on_main(app, move || with_panel(&id, |p| p.view.run_javascript(&script, gio::Cancellable::NONE, |_| {})))
}

pub fn reload(app: &AppHandle, id: &str) -> Result<(), String> {
    let id = id.to_string();
    on_main(app, move || with_panel(&id, |p| p.view.reload()))
}

pub fn zoom(app: &AppHandle, id: &str, factor: f64) -> Result<(), String> {
    let id = id.to_string();
    on_main(app, move || with_panel(&id, |p| p.view.set_zoom_level(factor.clamp(store::MIN_ZOOM, store::MAX_ZOOM))))
}

/// The address the panel shows now (where "open in browser" goes).
pub fn current_uri(app: &AppHandle, id: &str) -> Option<String> {
    let id = id.to_string();
    on_main(app, move || with_panel(&id, |p| p.view.uri().map(|u| u.to_string()))).ok().flatten()
}

/// A still picture of what the panel shows, as a PNG data URL (shown under sheets while the live view is hidden).
pub fn snapshot(app: &AppHandle, id: &str) -> Result<String, String> {
    let id = id.to_string();
    let (tx, rx) = mpsc::channel::<Result<String, String>>();
    on_main(app, move || {
        with_panel(&id, |p| {
            p.view.snapshot(SnapshotRegion::Visible, SnapshotOptions::NONE, gio::Cancellable::NONE, move |result| {
                let _ = tx.send(result.map_err(|e| e.to_string()).and_then(|surface| png_data_url(surface)));
            });
        })
    })?;
    rx.recv_timeout(Duration::from_secs(4)).map_err(|_| "the snapshot took too long".to_string())?
}

fn png_data_url(surface: gtk::cairo::Surface) -> Result<String, String> {
    let image = gtk::cairo::ImageSurface::try_from(surface).map_err(|_| "the snapshot isn't an image".to_string())?;
    let pixbuf = gtk::gdk::pixbuf_get_from_surface(&image, 0, 0, image.width(), image.height()).ok_or("couldn't read the snapshot")?;
    let png = pixbuf.save_to_bufferv("png", &[]).map_err(|e| e.to_string())?;
    use base64::Engine;
    Ok(format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png)))
}

/// Closes a panel (its web processes exit and the memory comes back); with `wipe` its data folder
/// is deleted too (sign out and forget).
pub fn destroy(app: &AppHandle, id: &str, wipe: bool) -> Result<(), String> {
    let dir = data_dir(id)?;
    let key = id.to_string();
    on_main(app, move || {
        if let Some(panel) = PANELS.with(|p| p.borrow_mut().remove(&key)) {
            let had_focus = panel.view.has_focus();
            if let Some(parent) = panel.view.parent().and_then(|p| p.downcast::<gtk::Container>().ok()) {
                parent.remove(&panel.view);
                if had_focus {
                    if let Ok(overlay) = parent.downcast::<gtk::Overlay>() {
                        focus_main(&overlay);
                    }
                }
            }
        }
        Ok(())
    })?;
    if wipe {
        wipe_dir(&dir)?;
    }
    Ok(())
}

/// Deletes a data folder. The web processes let go of it asynchronously; try for a moment.
pub fn wipe_dir(dir: &Path) -> Result<(), String> {
    for _ in 0..20 {
        match std::fs::remove_dir_all(dir) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => std::thread::sleep(Duration::from_millis(150)),
        }
    }
    match std::fs::remove_dir_all(dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
        _ => Ok(()),
    }
}

/// Bytes used by a folder and everything below it.
pub fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    entries
        .flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

pub fn data_root() -> Option<&'static Path> {
    DATA_ROOT.get().map(PathBuf::as_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_become_folder_names() {
        assert!(valid_id("whatsapp-1"));
        assert!(!valid_id("../etc"));
        assert!(!valid_id("A"));
        assert!(!valid_id(""));
        assert!(!valid_id(&"a".repeat(41)));
    }

    #[test]
    fn downloads_get_safe_names() {
        let dir = std::env::temp_dir().join(format!("zazabor-msg-dl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(download_target(&dir, "../../etc/passwd").file_name().unwrap(), "passwd");
        assert_eq!(download_target(&dir, ".bashrc").file_name().unwrap(), "bashrc");
        assert_eq!(download_target(&dir, "").file_name().unwrap(), "download");
        std::fs::write(dir.join("a.pdf"), b"x").unwrap();
        assert_eq!(download_target(&dir, "a.pdf").file_name().unwrap(), "a (1).pdf");
        assert!(risky_file("Setup.EXE") && risky_file("x.sh") && !risky_file("photo.jpg"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn wiping_removes_the_folder() {
        let dir = std::env::temp_dir().join(format!("zazabor-msg-wipe-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("a/b")).unwrap();
        std::fs::write(dir.join("a/b/cookies.sqlite"), vec![0u8; 1000]).unwrap();
        std::fs::write(dir.join("x"), vec![0u8; 24]).unwrap();
        assert_eq!(dir_size(&dir), 1024);
        wipe_dir(&dir).unwrap();
        assert!(!dir.exists());
        assert_eq!(dir_size(&dir), 0);
        wipe_dir(&dir).unwrap();
    }
}

//! What a panel's injected script (inject.js) reports. The page is untrusted: every report is
//! size-capped, shape-checked and rate-limited here before anything acts on it.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Deserialize;
use tauri::{AppHandle, Emitter};

use super::{services, store};
use crate::alerts::{self, Detected};
use crate::desktop_notify;

/// A report may not be longer than this (inject.js caps it lower).
const MAX_REPORT: usize = 8192;
const MAX_TITLE: usize = 200;
const MAX_BODY: usize = 500;
const MAX_TAG: usize = 100;
/// Desktop notifications one account may raise per minute.
const NOTIFY_PER_MINUTE: usize = 12;
/// A page can't claim more unread messages than this.
const MAX_UNREAD: u32 = 99_999;
/// The keys inject.js forwards: Ctrl+1..9, Ctrl+Shift+[ and ], Ctrl+R, Ctrl+Shift+K (the quick switcher).
const SHORTCUTS: &[&str] = &["1", "2", "3", "4", "5", "6", "7", "8", "9", "prev", "next", "reload", "switcher"];

#[derive(Debug, PartialEq)]
pub enum Event {
    Notify { via: String, title: String, body: String, tag: String },
    Title(String),
    /// A count the page reports itself (a custom web app, or a service's own badge).
    Unread(u32),
    /// A shortcut pressed inside the panel (the page never sees keys typed in a panel).
    Key(String),
    /// A call dialog the page shows (ringing) or took away (inject.js watches the DOM).
    Call { ringing: bool, name: String, video: bool },
    /// The page closed a notification (a call's, usually: it stops ringing).
    NotifyClosed { tag: String, title: String },
}

#[derive(Deserialize)]
struct Raw {
    t: String,
    #[serde(default)]
    via: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    tag: String,
    #[serde(default)]
    n: u32,
    #[serde(default)]
    key: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    video: bool,
}

/// Text with control characters dropped, cut to `max` characters.
fn clip(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control() || *c == '\n').take(max).collect()
}

/// Reads a report (the JSON text inject.js posted). None when it isn't valid.
pub fn parse_json(json: &str) -> Option<Event> {
    if json.len() > MAX_REPORT {
        return None;
    }
    let raw: Raw = serde_json::from_str(json).ok()?;
    match raw.t.as_str() {
        "notify" => {
            let title = clip(&raw.title, MAX_TITLE);
            if title.is_empty() {
                return None;
            }
            let via = if raw.via == "sw" { "sw" } else { "page" };
            Some(Event::Notify { via: via.into(), title, body: clip(&raw.body, MAX_BODY), tag: clip(&raw.tag, MAX_TAG) })
        }
        "title" => Some(Event::Title(clip(&raw.title, MAX_TITLE))),
        "unread" => Some(Event::Unread(raw.n.min(MAX_UNREAD))),
        "call" => Some(Event::Call { ringing: raw.state == "ringing", name: clip(&raw.name, MAX_TITLE), video: raw.video }),
        "notify-close" => Some(Event::NotifyClosed { tag: clip(&raw.tag, MAX_TAG), title: clip(&raw.title, MAX_TITLE) }),
        "key" if SHORTCUTS.contains(&raw.key.as_str()) => Some(Event::Key(raw.key)),
        _ => None,
    }
}

static RECENT: Mutex<Option<HashMap<String, VecDeque<Instant>>>> = Mutex::new(None);

/// Whether this account may raise another notification now (a page can't flood the desktop).
fn rate_ok(id: &str) -> bool {
    let mut all = RECENT.lock().unwrap_or_else(|e| e.into_inner());
    let recent = all.get_or_insert_with(HashMap::new).entry(id.to_string()).or_default();
    let now = Instant::now();
    while recent.front().is_some_and(|t| now.duration_since(*t) > Duration::from_secs(60)) {
        recent.pop_front();
    }
    if recent.len() >= NOTIFY_PER_MINUTE {
        return false;
    }
    recent.push_back(now);
    true
}

/// Acts on a report from account `id`. Message text never goes to the log: only lengths.
pub fn deliver(app: &AppHandle, id: &str, event: Event) {
    let Some(account) = store::read(|s| s.get(id).cloned()) else { return };
    match event {
        Event::Notify { via, title, body, tag } => {
            eprintln!("[messages] {id}: notification via {via} (title {} chars, body {} chars)", title.chars().count(), body.chars().count());
            let (dnd, previews) = store::read(|s| (s.settings.dnd, s.settings.previews));
            // A ringing call is not an ordinary notification: it gets its own card (alerts), and a
            // muted account or Do not disturb only silences it.
            match alerts::detect(&account.service, &title, &body, &tag) {
                Some(Detected::Ringing { name, video }) => {
                    if alerts::web_call(&account, &name, video, true) {
                        return;
                    }
                }
                Some(Detected::Missed { name, video }) => {
                    if !account.muted && !dnd {
                        alerts::web_call_missed(&account, &name, video);
                    }
                    return;
                }
                Some(Detected::Ended) => {
                    alerts::web_call(&account, "", false, false);
                    return;
                }
                None => {}
            }
            if account.muted || dnd {
                return;
            }
            if !rate_ok(id) {
                eprintln!("[messages] {id}: notification dropped (rate limit)");
                return;
            }
            let _ = app.emit("msg-notified", serde_json::json!({ "id": id }));
            // The alerts window shows it; the desktop's own notification is the fallback.
            if alerts::web_message(&account, &title, &body) {
                return;
            }
            // Previews off: only that there is something, and where.
            let (shown_title, shown_body) = if previews { (format!("{title} · {}", account.name), body) } else { (format!("New message · {}", account.name), String::new()) };
            let (app, id) = (app.clone(), id.to_string());
            // Clicking it shows the window on this account.
            let click: Box<dyn Fn() + Send> = Box::new(move || {
                crate::show_main(&app, None);
                let _ = app.emit("msg-open", serde_json::json!({ "id": id }));
            });
            desktop_notify::show(desktop_notify::Note { title: &shown_title, body: &shown_body, icon: "dialog-information", category: "im.received", replaces: 0 }, Some(click));
        }
        Event::Call { ringing, name, video } => {
            eprintln!("[messages] {id}: call dialog {}", if ringing { "ringing" } else { "gone" });
            alerts::web_call(&account, &name, video, ringing);
        }
        Event::NotifyClosed { tag: _, title } => {
            // A closed notification ends the call it announced; another chat's closing doesn't.
            alerts::web_call_closed(&account, &title);
        }
        Event::Title(title) => {
            // A page title can name a contact ("Ann messaged you"): its text is never logged.
            let count = services::unread_from_title(&account.service, &title);
            set_unread(app, id, count);
        }
        Event::Unread(count) => set_unread(app, id, count),
        Event::Key(key) => {
            let _ = app.emit("msg-key", serde_json::json!({ "key": key }));
        }
    }
}

fn set_unread(app: &AppHandle, id: &str, count: u32) {
    if super::set_unread(id, count) {
        let _ = app.emit("msg-unread", serde_json::json!({ "id": id, "count": count }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_notification() {
        let e = parse_json(r#"{"t":"notify","via":"sw","title":"Ada","body":"Hi","tag":"c1"}"#);
        assert_eq!(e, Some(Event::Notify { via: "sw".into(), title: "Ada".into(), body: "Hi".into(), tag: "c1".into() }));
    }

    #[test]
    fn caps_and_cleans_text() {
        let long = "x".repeat(5000);
        let json = format!(r#"{{"t":"notify","title":"a\u0007b","body":"{long}"}}"#);
        let Some(Event::Notify { title, body, .. }) = parse_json(&json) else { panic!("not a notification") };
        assert_eq!(title, "ab");
        assert_eq!(body.chars().count(), MAX_BODY);
    }

    #[test]
    fn refuses_what_isnt_ours() {
        assert_eq!(parse_json(r#"{"t":"notify","title":""}"#), None);
        assert_eq!(parse_json(r#"{"t":"evil"}"#), None);
        assert_eq!(parse_json("not json"), None);
        assert_eq!(parse_json(&"a".repeat(MAX_REPORT + 1)), None);
        assert_eq!(parse_json(r#"{"t":"title"#), None);
    }

    #[test]
    fn reads_unread_and_keys() {
        assert_eq!(parse_json(r#"{"t":"unread","n":7}"#), Some(Event::Unread(7)));
        assert_eq!(parse_json(r#"{"t":"unread","n":4000000}"#), Some(Event::Unread(MAX_UNREAD)));
        assert_eq!(parse_json(r#"{"t":"key","key":"3"}"#), Some(Event::Key("3".into())));
        assert_eq!(parse_json(r#"{"t":"key","key":"quit"}"#), None);
        assert_eq!(parse_json(r#"{"t":"dbg","msg":"x"}"#), None);
    }

    #[test]
    fn reads_call_reports() {
        assert_eq!(parse_json(r#"{"t":"call","state":"ringing","name":"Ann","video":true}"#), Some(Event::Call { ringing: true, name: "Ann".into(), video: true }));
        assert_eq!(parse_json(r#"{"t":"call","state":"ended"}"#), Some(Event::Call { ringing: false, name: String::new(), video: false }));
        assert_eq!(parse_json(r#"{"t":"notify-close","title":"Ann","tag":"c"}"#), Some(Event::NotifyClosed { tag: "c".into(), title: "Ann".into() }));
    }

    #[test]
    fn limits_the_rate() {
        let allowed = (0..NOTIFY_PER_MINUTE + 5).filter(|_| rate_ok("rate-test")).count();
        assert_eq!(allowed, NOTIFY_PER_MINUTE);
    }
}

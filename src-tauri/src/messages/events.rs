//! What a panel's injected script (inject.js) reports. The page is untrusted: every report is
//! size-capped, shape-checked and rate-limited here before anything acts on it.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Deserialize;
use tauri::{AppHandle, Emitter};

use crate::desktop_notify;

/// A report may not be longer than this (inject.js caps it lower).
const MAX_REPORT: usize = 8192;
const MAX_TITLE: usize = 200;
const MAX_BODY: usize = 500;
const MAX_TAG: usize = 100;
/// Desktop notifications one account may raise per minute.
const NOTIFY_PER_MINUTE: usize = 12;

#[derive(Debug, PartialEq)]
pub enum Event {
    Notify { via: String, title: String, body: String, tag: String },
    Title(String),
    /// Spike diagnostics from the test pages; refused unless ZAZABOR_MSG_SPIKE is set.
    Debug(String),
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
    msg: String,
}

/// Text with control characters dropped, cut to `max` characters.
fn clip(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control() || *c == '\n').take(max).collect()
}

/// The unread count in a tab title: "(3) WhatsApp" or "[12] Telegram".
pub fn unread_from_title(title: &str) -> u32 {
    let t = title.trim_start();
    let Some(rest) = t.strip_prefix('(').or_else(|| t.strip_prefix('[')) else { return 0 };
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() || digits.len() > 5 {
        return 0;
    }
    let after = &rest[digits.len()..];
    if after.starts_with(')') || after.starts_with(']') {
        digits.parse().unwrap_or(0)
    } else {
        0
    }
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
        "dbg" if std::env::var_os("ZAZABOR_MSG_SPIKE").is_some() => Some(Event::Debug(clip(&raw.msg, 400))),
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

/// Acts on a report from account `id` (shown as `name`).
pub fn deliver(app: &AppHandle, id: &str, name: &str, event: Event) {
    match event {
        Event::Notify { via, title, body, tag } => {
            eprintln!("[messages] {id}: notification via {via}: {title:?} / {body:?} (tag {tag:?})");
            if !rate_ok(id) {
                eprintln!("[messages] {id}: notification dropped (rate limit)");
                return;
            }
            let _ = app.emit("msg-notified", serde_json::json!({ "id": id, "title": title }));
            let shown_title = format!("{title} · {name}");
            let (app, id) = (app.clone(), id.to_string());
            // Clicking it asks the page to open this account.
            let click: Box<dyn Fn() + Send> = Box::new(move || {
                let _ = app.emit("msg-open", serde_json::json!({ "id": id }));
            });
            desktop_notify::show(desktop_notify::Note { title: &shown_title, body: &body, icon: "dialog-information", category: "im.received", replaces: 0 }, Some(click));
        }
        Event::Title(title) => {
            let count = unread_from_title(&title);
            eprintln!("[messages] {id}: title {title:?} -> unread {count}");
            let _ = app.emit("msg-unread", serde_json::json!({ "id": id, "count": count }));
        }
        Event::Debug(msg) => eprintln!("[messages] {id}: dbg {msg}"),
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
    fn counts_from_titles() {
        assert_eq!(unread_from_title("(3) WhatsApp"), 3);
        assert_eq!(unread_from_title("[12] Telegram"), 12);
        assert_eq!(unread_from_title("WhatsApp"), 0);
        assert_eq!(unread_from_title("(x) Hi"), 0);
        assert_eq!(unread_from_title("(123456789) Hi"), 0);
    }

    #[test]
    fn limits_the_rate() {
        let allowed = (0..NOTIFY_PER_MINUTE + 5).filter(|_| rate_ok("rate-test")).count();
        assert_eq!(allowed, NOTIFY_PER_MINUTE);
    }
}

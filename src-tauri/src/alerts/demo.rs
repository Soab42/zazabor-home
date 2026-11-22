//! A dev-only trigger for trying the alerts without a phone or a chat account: with
//! `ZAZABOR_ALERTS_DEMO=1` the app watches `alerts-demo.txt` in its data folder and acts on each
//! line (then deletes the file). Demo alerts never reach a phone or a chat: their buttons only log.
//!
//!   phone-call ringing NAME | NUMBER [| video]   a ringing phone call
//!   phone-call ended                             it ended
//!   sms NAME | TEXT                              a text
//!   web-call SERVICE | NAME [| video]            a chat call (whatsapp, teams, messenger, ...)
//!   web-call-end SERVICE
//!   web-msg SERVICE | NAME | TEXT                a chat message
//!   burst N                                      N rapid texts
//!   clear

use std::path::Path;
use std::time::Duration;

use serde_json::json;
use tauri::AppHandle;

use super::queue::{Alert, Kind};
use super::{now_ms, push, remove_key, store, toast_ttl};

pub fn start(_app: &AppHandle, data_dir: &Path) {
    if std::env::var("ZAZABOR_ALERTS_DEMO").ok().as_deref() != Some("1") {
        return;
    }
    let file = data_dir.join("alerts-demo.txt");
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(300));
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        let _ = std::fs::remove_file(&file);
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
            run(line);
        }
    });
}

fn parts(rest: &str) -> Vec<String> {
    rest.split('|').map(|p| p.trim().to_string()).collect()
}

fn run(line: &str) {
    let (command, rest) = line.split_once(' ').unwrap_or((line, ""));
    let p = parts(rest);
    let arg = |n: usize| p.get(n).cloned().unwrap_or_default();
    let settings = store::get();
    match command {
        "phone-call" if rest.trim() == "ended" => remove_key("call:phone"),
        "phone-call" => {
            let name = arg(0).trim_start_matches("ringing").trim().to_string();
            let mut a = Alert::new(Kind::Call, "phone", &name, "Incoming call");
            a.key = "call:phone".into();
            a.via = "app".into();
            a.at = now_ms();
            a.number = Some(arg(1));
            a.phone_id = Some("demo-phone".into());
            a.can_reply = true;
            a.silent = settings.dnd || !settings.ringtone;
            a.demo = true;
            push(a);
        }
        "sms" => {
            let text = if settings.previews { arg(1) } else { "New message".to_string() };
            let mut a = Alert::new(Kind::Message, "sms", &arg(0), &text);
            a.at = now_ms();
            a.ttl = toast_ttl(&settings);
            a.phone_id = Some("demo-phone".into());
            a.number = Some("+8801700000000".into());
            a.addresses = vec!["+8801700000000".into()];
            a.thread_id = Some(json!(1));
            a.can_reply = true;
            a.can_call = true;
            a.can_read = true;
            a.demo = true;
            push(a);
        }
        "web-call" => {
            let video = arg(2) == "video";
            let mut a = Alert::new(Kind::Call, &arg(0), &arg(1), if video { "Incoming video call" } else { "Incoming call" });
            a.key = format!("call:demo-{}", arg(0));
            a.via = "web".into();
            a.video = video;
            a.at = now_ms();
            a.account_id = Some(format!("demo-{}", arg(0)));
            a.account_name = Some(arg(0));
            a.silent = settings.dnd || !settings.ringtone;
            a.can_reply = true;
            a.demo = true;
            push(a);
        }
        "web-call-end" => remove_key(&format!("call:demo-{}", rest.trim())),
        "web-msg" => {
            let mut a = Alert::new(Kind::Message, &arg(0), &arg(1), &arg(2));
            a.at = now_ms();
            a.ttl = toast_ttl(&settings);
            a.account_id = Some(format!("demo-{}", arg(0)));
            a.account_name = Some(arg(0));
            a.can_reply = true;
            a.can_call = true;
            a.demo = true;
            push(a);
        }
        "burst" => {
            let n: usize = rest.trim().parse().unwrap_or(5);
            for k in 1..=n.min(12) {
                run(&format!("sms Sender {k} | Message number {k}"));
            }
        }
        "clear" => {
            if let Some(app) = super::APP.get() {
                super::with(|i| i.queue = Default::default());
                super::publish(app);
            }
        }
        other => eprintln!("[alerts] demo: unknown command {other:?}"),
    }
}

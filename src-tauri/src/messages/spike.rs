//! THROWAWAY (phase 0 spike): drives the panels without the Messages page, which doesn't exist
//! yet. When ZAZABOR_MSG_CTL names a file, lines appended to it are run as commands:
//!   add ID URL UA X Y W H     UA is `safari` or `-` (the default); the panel starts shown
//!   rect ID X Y W H | show ID | hide ID | park ID | focus ID | reload ID
//!   eval ID JS...             runs JS in the panel
//!   destroy ID [wipe]         close the panel (and delete its data folder)
//!   resize W H                resizes the main window
//! `park` moves a panel far off-screen instead of hiding it (`rect` brings it back), to compare
//! background behavior.
//! Delete this file and its two lines in mod.rs when the real page drives the panels.

use std::io::{Read, Seek, SeekFrom};
use std::time::Duration;

use tauri::{AppHandle, Manager};

use super::panels::{self, Rect, Spec};

pub fn init(app: AppHandle) {
    let Some(path) = std::env::var_os("ZAZABOR_MSG_CTL") else { return };
    std::thread::spawn(move || {
        let mut offset = 0u64;
        loop {
            std::thread::sleep(Duration::from_millis(300));
            let Ok(mut file) = std::fs::File::open(&path) else { continue };
            if file.seek(SeekFrom::Start(offset)).is_err() {
                continue;
            }
            let mut text = String::new();
            if file.read_to_string(&mut text).is_err() {
                continue;
            }
            // Only whole lines.
            let whole = text.rfind('\n').map(|i| i + 1).unwrap_or(0);
            offset += whole as u64;
            for line in text[..whole].lines().filter(|l| !l.trim().is_empty()) {
                let result = run(&app, line);
                eprintln!("[messages-spike] {line} -> {result:?}");
            }
        }
    });
}

fn num(s: Option<&str>) -> Result<f64, String> {
    s.and_then(|s| s.parse().ok()).ok_or_else(|| "bad number".to_string())
}

fn rect(p: &mut std::str::SplitN<'_, char>) -> Result<Rect, String> {
    Ok(Rect { x: num(p.next())?, y: num(p.next())?, w: num(p.next())?, h: num(p.next())? })
}

fn run(app: &AppHandle, line: &str) -> Result<(), String> {
    let mut p = line.splitn(8, ' ');
    let cmd = p.next().unwrap_or("");
    let id = if cmd == "resize" { "" } else { p.next().unwrap_or("") };
    match cmd {
        "add" => {
            let url = p.next().ok_or("no url")?.to_string();
            let ua = match p.next() {
                Some("safari") => Some(panels::SAFARI_UA.to_string()),
                _ => None,
            };
            let r = rect(&mut p)?;
            panels::create(app, Spec { id: id.into(), name: id.into(), url, user_agent: ua }, r, true)
        }
        "rect" => panels::set_rect(app, id, rect(&mut p)?),
        "show" => panels::set_visible(app, id, true),
        "hide" => panels::set_visible(app, id, false),
        "park" => {
            let (w, h) = (1000.0, 700.0);
            panels::set_rect(app, id, Rect { x: -20000.0, y: -20000.0, w, h })?;
            panels::set_visible(app, id, true)
        }
        "focus" => panels::focus(app, id),
        "reload" => panels::reload(app, id),
        "eval" => panels::eval(app, id, line.splitn(3, ' ').nth(2).unwrap_or("")),
        "destroy" => panels::destroy(app, id, p.next() == Some("wipe")),
        "resize" => {
            let w = app.get_webview_window(panels::MAIN).ok_or("no main window")?;
            let (width, height) = (num(p.next())?, num(p.next())?);
            w.set_size(tauri::LogicalSize::new(width, height)).map_err(|e| e.to_string())
        }
        _ => Err("unknown command".into()),
    }
}

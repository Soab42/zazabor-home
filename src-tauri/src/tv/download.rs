//! Downloads for the desktop: a title's video file (or an episode, a season, a whole series) saved
//! to `~/Downloads/Zazabor TV/<Title (Year)>/`. Files stream into `<name>.part` in chunks asked
//! for with `Range`, so a stopped or failed download carries on from the bytes it has (after a
//! pause, an error or a restart); only when the last byte is in does the file get its real name.
//! At most `PARALLEL` run at once; the rest wait their turn. The Android app doesn't use this: it
//! hands downloads to the system's DownloadManager (`mobile-tv`'s `MainActivity.kt`).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::Manager;

use super::catalog::db_err;
use super::{lock, now_ms, state, Result, State};

/// Downloads running at the same time.
const PARALLEL: usize = 2;
/// How much one request asks for: a stalled server costs a chunk's timeout, not the whole file.
const CHUNK: u64 = 8 << 20;
/// Kept free on the disk beyond the file itself.
const SPARE: u64 = 64 << 20;

pub const RUN: u8 = 0;
pub const PAUSE: u8 = 1;
pub const CANCEL: u8 = 2;

pub fn schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS downloads (
           id TEXT PRIMARY KEY, key TEXT NOT NULL, item_id TEXT NOT NULL DEFAULT '', title TEXT NOT NULL, year INTEGER,
           label TEXT NOT NULL DEFAULT '', url TEXT NOT NULL, path TEXT NOT NULL, size INTEGER NOT NULL DEFAULT 0,
           got INTEGER NOT NULL DEFAULT 0, state TEXT NOT NULL, error TEXT NOT NULL DEFAULT '',
           added INTEGER NOT NULL, finished INTEGER NOT NULL DEFAULT 0
         );",
    )
    .map_err(db_err)
}

/// One download as the page sees it.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Download {
    pub id: String,
    /// The title's key (`name|year|kind`, made by the page) the page matches cards with.
    pub key: String,
    pub item_id: String,
    pub title: String,
    pub year: Option<i32>,
    /// What this file is within the title: an episode's file name; empty for a film.
    pub label: String,
    pub url: String,
    /// Where the finished file is (the `.part` file lives next to it until then).
    pub path: String,
    pub size: i64,
    pub got: i64,
    /// "queued", "running", "paused", "failed" or "done".
    pub state: String,
    pub error: String,
    pub added: i64,
    pub finished: i64,
    /// A finished download whose file is still where it was put.
    pub exists: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub key: String,
    #[serde(default)]
    pub item_id: String,
    pub title: String,
    pub year: Option<i32>,
    #[serde(default)]
    pub label: String,
    pub url: String,
    /// The expected size when the listing told it (shown before the first byte arrives).
    #[serde(default)]
    pub size: i64,
}

const COLS: &str = "id,key,item_id,title,year,label,url,path,size,got,state,error,added,finished";

fn row(r: &rusqlite::Row) -> rusqlite::Result<Download> {
    let (path, state): (String, String) = (r.get(7)?, r.get(10)?);
    let exists = state == "done" && Path::new(&path).exists();
    Ok(Download { id: r.get(0)?, key: r.get(1)?, item_id: r.get(2)?, title: r.get(3)?, year: r.get(4)?, label: r.get(5)?, url: r.get(6)?, path, size: r.get(8)?, got: r.get(9)?, state, error: r.get(11)?, added: r.get(12)?, finished: r.get(13)?, exists })
}

fn list_of(conn: &Connection) -> Vec<Download> {
    let Ok(mut stmt) = conn.prepare(&format!("SELECT {COLS} FROM downloads ORDER BY added DESC, rowid DESC")) else { return Vec::new() };
    let Ok(rows) = stmt.query_map([], row) else { return Vec::new() };
    rows.filter_map(|r| r.ok()).collect()
}

fn get(conn: &Connection, id: &str) -> Option<Download> {
    conn.query_row(&format!("SELECT {COLS} FROM downloads WHERE id=?1"), [id], row).optional().ok().flatten()
}

fn set_state(conn: &Connection, id: &str, state: &str, error: &str) {
    let _ = conn.execute("UPDATE downloads SET state=?2, error=?3 WHERE id=?1", params![id, state, error]);
}

// ---- names and places --------------------------------------------------------------------------

/// A file or folder name the disk accepts everywhere: no separators or reserved characters.
pub fn safe_name(text: &str) -> String {
    let cleaned: String = text.chars().map(|c| if c.is_control() || "/\\:*?\"<>|".contains(c) { '_' } else { c }).collect();
    let cleaned = cleaned.trim().trim_matches('.').trim().to_string();
    let cut: String = cleaned.chars().take(120).collect();
    if cut.is_empty() { "Untitled".to_string() } else { cut }
}

fn decode(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `<dir>/<Title (Year)>/<file name of the URL>`.
pub fn destination(dir: &Path, title: &str, year: Option<i32>, url: &str) -> PathBuf {
    let folder = match year {
        Some(y) => format!("{title} ({y})"),
        None => title.to_string(),
    };
    let file = decode(url.split(['?', '#']).next().unwrap_or(url).rsplit('/').next().unwrap_or("video"));
    dir.join(safe_name(&folder)).join(safe_name(if file.is_empty() { "video" } else { &file }))
}

#[cfg(unix)]
fn free_bytes(dir: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    (unsafe { libc::statvfs(c.as_ptr(), &mut s) } == 0).then(|| s.f_bavail as u64 * s.f_frsize as u64)
}

#[cfg(not(unix))]
fn free_bytes(_dir: &Path) -> Option<u64> {
    None
}

fn human(bytes: u64) -> String {
    let gb = bytes as f64 / 1073741824.0;
    if gb >= 1.0 { format!("{gb:.1} GB") } else { format!("{} MB", (bytes / 1048576).max(1)) }
}

/// Errors unless `dir` has room for `need` more bytes (and a little spare). Unknown free space passes.
fn check_space(dir: &Path, need: u64) -> Result<()> {
    match free_bytes(dir) {
        Some(free) if free < need + SPARE => Err(format!("Not enough free space: {} needed, {} free.", human(need), human(free))),
        _ => Ok(()),
    }
}

// ---- the transfer ------------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Done,
    Paused,
    Canceled,
}

pub fn chunk_agent() -> ureq::Agent {
    super::listing::agent(8, 90)
}

/// "bytes 0-99/1000" -> (0, 99, Some(1000)); "bytes */1000" -> total only.
fn content_range(text: &str) -> (Option<(u64, u64)>, Option<u64>) {
    let rest = text.trim().strip_prefix("bytes").unwrap_or("").trim();
    let (span, total) = rest.split_once('/').unwrap_or((rest, "*"));
    let total = total.trim().parse().ok();
    let span = span.split_once('-').and_then(|(a, b)| Some((a.trim().parse().ok()?, b.trim().parse().ok()?)));
    (span, total)
}

/// Writes `url` into `part`, continuing from what `part` already holds. `need(bytes)` is asked once
/// the size is known (the disk-space check); `progress(have, total)` is called as bytes arrive.
/// Stops with `Paused` or `Canceled` when `control` says so; the file keeps what it has.
pub fn transfer(agent: &ureq::Agent, url: &str, part: &Path, control: &AtomicU8, chunk: u64, need: &dyn Fn(u64) -> Result<()>, progress: &mut dyn FnMut(u64, u64)) -> Result<Outcome> {
    let mut have = part.metadata().map(|m| m.len()).unwrap_or(0);
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(part).map_err(|e| format!("Can't write {}: {e}", part.display()))?;
    let mut total: Option<u64> = None;
    let mut restarted = false;
    let mut asked = false;
    let stop = |control: &AtomicU8| match control.load(Ordering::SeqCst) {
        PAUSE => Some(Outcome::Paused),
        CANCEL => Some(Outcome::Canceled),
        _ => None,
    };
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        if let Some(out) = stop(control) {
            return Ok(out);
        }
        let response = agent
            .get(url)
            .header("User-Agent", "Zazabor")
            .header("Range", format!("bytes={have}-{}", have + chunk - 1))
            .call()
            .map_err(|e| format!("Couldn't reach the server: {e}"))?;
        let status = response.status().as_u16();
        let header = |name: &str| response.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
        let streaming_all = match status {
            206 => {
                let (span, t) = content_range(&header("content-range").unwrap_or_default());
                if span.map(|(a, _)| a) != Some(have) {
                    return Err("The server answered with the wrong part of the file.".into());
                }
                total = t.or(total);
                false
            }
            200 => {
                // the server ignores ranges: the whole file comes, from the start
                if have > 0 {
                    file.set_len(0).map_err(|e| e.to_string())?;
                    have = 0;
                }
                total = header("content-length").and_then(|v| v.parse().ok());
                true
            }
            416 => {
                let (_, t) = content_range(&header("content-range").unwrap_or_default());
                if t == Some(have) && have > 0 {
                    return Ok(Outcome::Done);
                }
                if restarted {
                    return Err("The server won't give this file.".into());
                }
                restarted = true;
                file.set_len(0).map_err(|e| e.to_string())?;
                have = 0;
                continue;
            }
            404 => return Err("The server no longer has this file (404).".into()),
            other => return Err(format!("The server answered {other}.")),
        };
        if !asked {
            if let Some(t) = total {
                need(t.saturating_sub(have))?;
            }
            asked = true;
        }
        let mut body = if streaming_all {
            // a body of unknown length could take hours: no overall timeout for it
            drop(response);
            let long = super::listing::agent(8, 3 * 86400);
            long.get(url).header("User-Agent", "Zazabor").call().map_err(|e| format!("Couldn't reach the server: {e}"))?.into_body().into_reader()
        } else {
            response.into_body().into_reader()
        };
        let before = have;
        loop {
            if let Some(out) = stop(control) {
                file.flush().map_err(|e| e.to_string())?;
                return Ok(out);
            }
            let n = body.read(&mut buf).map_err(|e| format!("The download stopped: {e}"))?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n]).map_err(|e| format!("Can't write the file: {e}"))?;
            have += n as u64;
            progress(have, total.unwrap_or(0).max(have));
        }
        file.flush().map_err(|e| e.to_string())?;
        match total {
            Some(t) if have >= t => return Ok(Outcome::Done),
            None if streaming_all => return Ok(Outcome::Done),
            _ if have == before => return Err("The server stopped sending the file.".into()),
            _ => {}
        }
    }
}

/// Downloads into `<dest>.part` and renames it to `dest` once every byte is in.
pub fn download_file(agent: &ureq::Agent, url: &str, dest: &Path, control: &AtomicU8, chunk: u64, progress: &mut dyn FnMut(u64, u64)) -> Result<Outcome> {
    let dir = dest.parent().ok_or("That path has no folder")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("Can't make {}: {e}", dir.display()))?;
    let part = PathBuf::from(format!("{}.part", dest.display()));
    let out = transfer(agent, url, &part, control, chunk, &|n| check_space(dir, n), progress)?;
    if out == Outcome::Done {
        std::fs::rename(&part, dest).map_err(|e| format!("Can't finish {}: {e}", dest.display()))?;
    }
    Ok(out)
}

// ---- the manager -------------------------------------------------------------------------------

fn jobs() -> &'static Mutex<HashMap<String, Arc<AtomicU8>>> {
    static JOBS: OnceLock<Mutex<HashMap<String, Arc<AtomicU8>>>> = OnceLock::new();
    JOBS.get_or_init(Default::default)
}

fn publish(st: &State) {
    let all = list_of(&st.db());
    st.emit("tv-downloads", all);
}

/// Downloads that were running when the app closed come back paused.
pub fn on_start(conn: &Connection) {
    let _ = conn.execute("UPDATE downloads SET state='paused' WHERE state IN ('running','queued')", []);
}

/// Starts queued downloads while fewer than `PARALLEL` run.
fn pump(st: &'static State) {
    loop {
        let (id, control) = {
            let mut running = lock(jobs());
            if running.len() >= PARALLEL {
                return;
            }
            let next: Option<String> = st.db().query_row("SELECT id FROM downloads WHERE state='queued' ORDER BY added, rowid LIMIT 1", [], |r| r.get(0)).optional().ok().flatten();
            let Some(id) = next else { return };
            let control = Arc::new(AtomicU8::new(RUN));
            running.insert(id.clone(), control.clone());
            set_state(&st.db(), &id, "running", "");
            (id, control)
        };
        std::thread::spawn(move || {
            run(st, &id, &control);
            lock(jobs()).remove(&id);
            publish(st);
            pump(st);
        });
    }
}

fn run(st: &'static State, id: &str, control: &AtomicU8) {
    let Some(d) = get(&st.db(), id) else { return };
    let dest = PathBuf::from(&d.path);
    let mut last = Instant::now() - Duration::from_secs(1);
    let mut progress = |got: u64, total: u64| {
        if last.elapsed() >= Duration::from_millis(400) {
            last = Instant::now();
            let _ = st.db().execute("UPDATE downloads SET got=?2, size=MAX(size,?3) WHERE id=?1", params![id, got as i64, total as i64]);
            publish(st);
        }
    };
    let result = download_file(&chunk_agent(), &d.url, &dest, control, CHUNK, &mut progress);
    let part = PathBuf::from(format!("{}.part", d.path));
    let db = st.db();
    match result {
        Ok(Outcome::Done) => {
            let size = dest.metadata().map(|m| m.len() as i64).unwrap_or(d.size);
            let _ = db.execute("UPDATE downloads SET state='done', error='', got=?2, size=?2, finished=?3 WHERE id=?1", params![id, size, now_ms()]);
        }
        Ok(Outcome::Paused) => {
            let got = part.metadata().map(|m| m.len() as i64).unwrap_or(0);
            let _ = db.execute("UPDATE downloads SET state='paused', got=?2 WHERE id=?1", params![id, got]);
        }
        Ok(Outcome::Canceled) => {
            let _ = std::fs::remove_file(&part);
            // the title's folder goes too when nothing else is in it
            if let Some(dir) = dest.parent() {
                let _ = std::fs::remove_dir(dir);
            }
            let _ = db.execute("DELETE FROM downloads WHERE id=?1", [id]);
        }
        Err(err) => {
            let got = part.metadata().map(|m| m.len() as i64).unwrap_or(0);
            let _ = db.execute("UPDATE downloads SET state='failed', error=?2, got=?3 WHERE id=?1", params![id, err, got]);
        }
    }
}

/// The folder downloads go to: the user's choice, else `~/Downloads/Zazabor TV`.
fn base_dir(st: &State, chosen: Option<&str>) -> PathBuf {
    match chosen.map(str::trim).filter(|d| !d.is_empty()) {
        Some(d) => PathBuf::from(d),
        None => st.app.path().download_dir().or_else(|_| st.app.path().home_dir().map(|h| h.join("Downloads"))).unwrap_or_else(|_| PathBuf::from(".")).join("Zazabor TV"),
    }
}

fn new_id(n: usize) -> String {
    format!("{:x}{:02x}", now_ms(), n)
}

#[tauri::command]
pub fn tv_dl_dir(dir: Option<String>) -> Result<String> {
    Ok(base_dir(state()?, dir.as_deref()).to_string_lossy().into_owned())
}

/// Queues files. One that is already queued or finished (same address) isn't queued twice.
#[tauri::command]
pub fn tv_dl_add(requests: Vec<Request>, dir: Option<String>) -> Result<Vec<Download>> {
    let st = state()?;
    let base = base_dir(st, dir.as_deref());
    let mut added = Vec::new();
    {
        let db = st.db();
        for (n, r) in requests.iter().enumerate() {
            let have: Option<String> = db.query_row("SELECT id FROM downloads WHERE url=?1", [&r.url], |x| x.get(0)).optional().map_err(db_err)?;
            if let Some(id) = have {
                // a failed or paused one is asked to go on
                let _ = db.execute("UPDATE downloads SET state='queued', error='' WHERE id=?1 AND state IN ('failed','paused')", [&id]);
                continue;
            }
            let id = new_id(n);
            let path = destination(&base, &r.title, r.year, &r.url);
            db.execute(
                "INSERT INTO downloads(id,key,item_id,title,year,label,url,path,size,state,added) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'queued',?10)",
                params![id, r.key, r.item_id, r.title, r.year, r.label, r.url, path.to_string_lossy(), r.size, now_ms()],
            )
            .map_err(db_err)?;
            added.push(id);
        }
    }
    pump(st);
    publish(st);
    let db = st.db();
    Ok(added.iter().filter_map(|id| get(&db, id)).collect())
}

#[tauri::command]
pub fn tv_dl_list() -> Result<Vec<Download>> {
    Ok(list_of(&state()?.db()))
}

#[tauri::command]
pub fn tv_dl_pause(id: String) -> Result<()> {
    let st = state()?;
    // (the lock is let go of before anything else runs: `pump` takes it again)
    let running = lock(jobs()).get(&id).cloned();
    if let Some(control) = running {
        control.store(PAUSE, Ordering::SeqCst);
    } else {
        let _ = st.db().execute("UPDATE downloads SET state='paused' WHERE id=?1 AND state='queued'", [&id]);
    }
    publish(st);
    Ok(())
}

#[tauri::command]
pub fn tv_dl_resume(id: String) -> Result<()> {
    let st = state()?;
    let running = lock(jobs()).get(&id).cloned();
    if let Some(control) = running {
        // still winding down from a pause: carry on
        control.store(RUN, Ordering::SeqCst);
    } else {
        let _ = st.db().execute("UPDATE downloads SET state='queued', error='' WHERE id=?1 AND state IN ('paused','failed')", [&id]);
        pump(st);
    }
    publish(st);
    Ok(())
}

/// Cancels a download (its partial file goes) or removes a finished one from the list; the finished
/// file is deleted too when `delete_file`.
#[tauri::command]
pub fn tv_dl_remove(id: String, delete_file: Option<bool>) -> Result<()> {
    let st = state()?;
    let running = lock(jobs()).get(&id).cloned();
    if let Some(control) = running {
        // the job's thread removes the partial file and the entry
        control.store(CANCEL, Ordering::SeqCst);
        return Ok(());
    }
    if let Some(d) = get(&st.db(), &id) {
        let _ = std::fs::remove_file(format!("{}.part", d.path));
        if d.state == "done" && delete_file == Some(true) {
            let _ = std::fs::remove_file(&d.path);
        }
        // the title's folder goes when nothing else is in it
        if let Some(dir) = Path::new(&d.path).parent() {
            let _ = std::fs::remove_dir(dir);
        }
        let _ = st.db().execute("DELETE FROM downloads WHERE id=?1", [&id]);
    }
    publish(st);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;
    use std::sync::atomic::AtomicBool;

    /// A little HTTP server for one file; answers `Range` requests unless `ranges` is false.
    struct Server {
        url: String,
        asked: Arc<Mutex<Vec<String>>>,
        stop: Arc<AtomicBool>,
    }

    impl Drop for Server {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            let _ = std::net::TcpStream::connect(self.url.trim_start_matches("http://").split('/').next().unwrap());
        }
    }

    fn serve(data: Vec<u8>, ranges: bool) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let asked = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (log, halt) = (asked.clone(), stop.clone());
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                if halt.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(mut conn) = conn else { continue };
                let mut reader = BufReader::new(conn.try_clone().unwrap());
                let mut range = None;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                        range = Some(v.trim().to_string());
                    }
                }
                log.lock().unwrap().push(range.clone().unwrap_or_default());
                let len = data.len() as u64;
                let head = |status: &str, extra: String, n: usize| format!("HTTP/1.1 {status}\r\n{extra}Content-Length: {n}\r\nConnection: close\r\n\r\n");
                let reply = match range.as_deref().filter(|_| ranges).and_then(|r| r.split_once('-')).and_then(|(a, b)| Some((a.parse::<u64>().ok()?, b.parse::<u64>().ok()?))) {
                    Some((a, _)) if a >= len => head("416 Range Not Satisfiable", format!("Content-Range: bytes */{len}\r\n"), 0).into_bytes(),
                    Some((a, b)) => {
                        let b = b.min(len - 1);
                        let mut out = head("206 Partial Content", format!("Content-Range: bytes {a}-{b}/{len}\r\n"), (b - a + 1) as usize).into_bytes();
                        out.extend_from_slice(&data[a as usize..=b as usize]);
                        out
                    }
                    None => {
                        let mut out = head("200 OK", String::new(), data.len()).into_bytes();
                        out.extend_from_slice(&data);
                        out
                    }
                };
                let _ = conn.write_all(&reply);
            }
        });
        Server { url: format!("http://{addr}/movie.mkv"), asked, stop }
    }

    fn bytes(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i * 7 % 251) as u8).collect()
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zazabor-tv-dl-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn quiet() -> impl FnMut(u64, u64) {
        |_, _| {}
    }

    #[test]
    fn resumes_from_a_partial_file_and_renames_on_completion() {
        let data = bytes(10_000);
        let server = serve(data.clone(), true);
        let dir = scratch("resume");
        let dest = dir.join("Film (2025)").join("movie.mkv");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(format!("{}.part", dest.display()), &data[..3_000]).unwrap();
        let control = AtomicU8::new(RUN);
        let out = download_file(&chunk_agent(), &server.url, &dest, &control, 2_000, &mut quiet()).unwrap();
        assert_eq!(out, Outcome::Done);
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        assert!(!Path::new(&format!("{}.part", dest.display())).exists(), "the .part file is renamed away");
        let asked = server.asked.lock().unwrap().clone();
        assert_eq!(asked.first().map(String::as_str), Some("3000-4999"), "the first request carries on after the 3000 bytes it had");
        assert_eq!(asked.len(), 4);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pauses_keeping_the_bytes_then_goes_on() {
        let data = bytes(20_000);
        let server = serve(data.clone(), true);
        let dir = scratch("pause");
        let dest = dir.join("a.mkv");
        let control = AtomicU8::new(RUN);
        let mut seen = 0;
        let out = download_file(&chunk_agent(), &server.url, &dest, &control, 4_000, &mut |got, total| {
            seen = got;
            assert_eq!(total, 20_000);
            if got >= 5_000 {
                control.store(PAUSE, Ordering::SeqCst);
            }
        })
        .unwrap();
        assert_eq!(out, Outcome::Paused);
        let part = PathBuf::from(format!("{}.part", dest.display()));
        let held = part.metadata().unwrap().len();
        assert!(held >= 5_000 && held < 20_000 && held == seen, "held {held}, seen {seen}");
        assert!(!dest.exists());
        control.store(RUN, Ordering::SeqCst);
        assert_eq!(download_file(&chunk_agent(), &server.url, &dest, &control, 4_000, &mut quiet()).unwrap(), Outcome::Done);
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancel_stops_and_leaves_no_finished_file() {
        let server = serve(bytes(20_000), true);
        let dir = scratch("cancel");
        let dest = dir.join("a.mkv");
        let control = AtomicU8::new(RUN);
        let out = download_file(&chunk_agent(), &server.url, &dest, &control, 4_000, &mut |got, _| {
            if got >= 4_000 {
                control.store(CANCEL, Ordering::SeqCst);
            }
        })
        .unwrap();
        assert_eq!(out, Outcome::Canceled);
        assert!(!dest.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_server_without_ranges_restarts_from_the_top() {
        let data = bytes(6_000);
        let server = serve(data.clone(), false);
        let dir = scratch("noranges");
        let dest = dir.join("a.mkv");
        std::fs::write(format!("{}.part", dest.display()), vec![9u8; 1_000]).unwrap();
        let control = AtomicU8::new(RUN);
        assert_eq!(download_file(&chunk_agent(), &server.url, &dest, &control, 2_000, &mut quiet()).unwrap(), Outcome::Done);
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_part_file_that_is_already_whole_just_finishes() {
        let data = bytes(3_000);
        let server = serve(data.clone(), true);
        let dir = scratch("whole");
        let dest = dir.join("a.mkv");
        std::fs::write(format!("{}.part", dest.display()), &data).unwrap();
        let control = AtomicU8::new(RUN);
        assert_eq!(download_file(&chunk_agent(), &server.url, &dest, &control, 2_000, &mut quiet()).unwrap(), Outcome::Done);
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refuses_when_the_disk_is_too_small() {
        let server = serve(bytes(2_000), true);
        let dir = scratch("space");
        let part = dir.join("a.part");
        let control = AtomicU8::new(RUN);
        let err = transfer(&chunk_agent(), &server.url, &part, &control, 4_000, &|_| Err("Not enough free space".to_string()), &mut quiet()).unwrap_err();
        assert!(err.contains("Not enough free space"));
        assert!(check_space(&dir, u64::MAX / 2).is_err(), "no disk has that much room");
        assert!(check_space(&dir, 1).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn names_and_places() {
        assert_eq!(safe_name("A/B: C?*"), "A_B_ C__");
        assert_eq!(safe_name("..."), "Untitled");
        let dest = destination(Path::new("/d"), "Spider-Man: Far", Some(2025), "http://h/x/Spider%20Man%20%281%29.mkv?x=1");
        assert_eq!(dest, PathBuf::from("/d/Spider-Man_ Far (2025)/Spider Man (1).mkv"));
        assert_eq!(destination(Path::new("/d"), "Show", None, "http://h/S01/e%.mkv"), PathBuf::from("/d/Show/e%.mkv"));
        assert_eq!(content_range("bytes 0-99/1000"), (Some((0, 99)), Some(1000)));
        assert_eq!(content_range("bytes */1000"), (None, Some(1000)));
    }

    #[test]
    fn the_list_round_trips() {
        let c = Connection::open_in_memory().unwrap();
        schema(&c).unwrap();
        c.execute("INSERT INTO downloads(id,key,title,url,path,state,added) VALUES('a','k','T','http://h/a','/d/a','queued',5)", []).unwrap();
        c.execute("INSERT INTO downloads(id,key,title,url,path,state,added) VALUES('b','k','T','http://h/b','/d/b','running',6)", []).unwrap();
        on_start(&c);
        let all = list_of(&c);
        assert_eq!(all.iter().map(|d| (d.id.as_str(), d.state.as_str())).collect::<Vec<_>>(), vec![("b", "paused"), ("a", "paused")]);
    }
}

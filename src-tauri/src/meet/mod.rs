//! Meet: voice calls, group video calls and scheduled meetings hosted by this PC
//! (docs/plans/meet.md). Guests join from a browser link; the audio and video go peer to peer, and
//! this PC serves the meeting page and relays the signalling (`server.rs`, `room.rs`). A Cloudflare
//! quick tunnel (`tunnel.rs`), or the user's own domain through a named tunnel, makes the link
//! reachable from outside the Wi-Fi. The host's own page
//! records the meeting and uploads it here (`/rec`), into ~/Videos/Zazabor Meetings.
//!
//! The `Hub` holds everything and reaches the app only through `Host`, so the tests run it with
//! no window.

pub mod ai;
mod remux;
pub mod browser;
pub mod invite;
pub mod room;
pub mod secret;
pub mod server;
pub mod store;
pub mod transcript;
pub mod tunnel;

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use room::Room;
use store::{Fields, Invitee, Recording, Stored};

pub type Result<T> = std::result::Result<T, String>;

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

const DEFAULT_STUN: [&str; 2] = ["stun:stun.l.google.com:19302", "stun:stun.cloudflare.com:3478"];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Turn {
    pub url: String,
    pub username: String,
    pub credential: String,
}

/// A meeting for the page: what's stored plus what the room knows.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Meeting {
    pub id: String,
    pub code: String,
    pub title: String,
    pub kind: String,
    pub starts_at: Option<i64>,
    pub minutes: i64,
    pub auto_record: bool,
    pub lobby: bool,
    pub invitees: Vec<Invitee>,
    pub created_at: i64,
    pub live: bool,
    pub host_in: bool,
    pub people: usize,
    pub waiting: usize,
    pub recording: bool,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MeetState {
    pub port: u16,
    pub tunnel: tunnel::Info,
    pub browser: Option<String>,
    pub recordings_dir: String,
    pub stun: Vec<String>,
    pub turn: Option<Turn>,
    /// "quick" (a random trycloudflare.com address) or "domain" (the user's own, through a named tunnel).
    pub mode: String,
    pub hostname: String,
    /// Whether a tunnel token is saved. The token itself never goes to the page.
    pub token_set: bool,
}

/// The answer to Settings' "Test": what `https://<hostname>/api/health` led to.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct DomainCheck {
    /// "this-pc", "other" or "unreachable".
    pub result: String,
    pub message: String,
}

/// What the hub needs from the app.
pub trait Host: Send + Sync + 'static {
    fn emit(&self, event: &str, payload: Value);
    /// A scheduled meeting is about to start (a desktop notification with Start).
    fn starting(&self, _hub: &Arc<Hub>, _m: &Stored) {}
}

/// A recording being written.
struct RecFile {
    code: String,
    id: String,
    path: PathBuf,
    next: u64,
    bytes: i64,
}

pub struct Hub {
    host: Box<dyn Host>,
    db: Mutex<Connection>,
    rooms: Mutex<HashMap<String, Arc<Room>>>,
    recs: Mutex<HashMap<String, Arc<Mutex<RecFile>>>>,
    /// Transcripts and notes (`ai.rs`): open track files and running jobs.
    ai: ai::State,
    pub data_dir: PathBuf,
    rec_dir: Mutex<PathBuf>,
    host_token: String,
    port: Mutex<Option<u16>>,
    pub tunnel: Arc<tunnel::Tunnel>,
    notified: Mutex<HashSet<String>>,
    /// Random per run; `/api/health` shows it so a test can tell this PC from another server.
    nonce: String,
    /// Why the fixed port couldn't be opened (domain mode).
    port_error: Mutex<Option<String>>,
}

impl Hub {
    pub fn new(host: Box<dyn Host>, conn: Connection, data_dir: PathBuf, rec_dir: PathBuf) -> Result<Arc<Hub>> {
        store::migrate(&conn)?;
        Ok(Arc::new(Hub {
            host,
            db: Mutex::new(conn),
            rooms: Mutex::new(HashMap::new()),
            recs: Mutex::new(HashMap::new()),
            ai: ai::State::default(),
            data_dir,
            rec_dir: Mutex::new(rec_dir),
            host_token: store::hex(&store::random_bytes::<32>()),
            port: Mutex::new(None),
            tunnel: tunnel::Tunnel::new(),
            notified: Mutex::new(HashSet::new()),
            nonce: store::hex(&store::random_bytes::<16>()),
            port_error: Mutex::new(None),
        }))
    }

    pub fn with_db<T>(&self, f: impl FnOnce(&mut Connection) -> Result<T>) -> Result<T> {
        f(&mut lock(&self.db))
    }

    pub fn emit(&self, event: &str, payload: Value) {
        self.host.emit(event, payload);
    }

    pub fn changed(&self) {
        self.emit("meet-changed", Value::Null);
    }

    pub fn rec_dir(&self) -> PathBuf {
        lock(&self.rec_dir).clone()
    }

    pub fn is_host_token(&self, token: &str) -> bool {
        let (a, b) = (token.as_bytes(), self.host_token.as_bytes());
        // Compares every byte, so the time taken doesn't tell how much of a guess was right.
        a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
    }

    pub fn host_token(&self) -> &str {
        &self.host_token
    }

    /// Starts the server on first use. Returns the port.
    pub fn ensure_server(self: &Arc<Self>, preferred: u16) -> Result<u16> {
        let mut port = lock(&self.port);
        if let Some(p) = *port {
            return Ok(p);
        }
        let p = server::start(self, preferred)?;
        *port = Some(p);
        Ok(p)
    }

    /// Starts the server for the current mode: the quick tunnel takes 47230 or the next free port;
    /// the domain's Cloudflare hostname points at exactly 47230, so there it's that port or an error.
    pub fn ensure_port(self: &Arc<Self>) -> Result<u16> {
        self.ensure_port_for(self.domain_settings().0, server::PREFERRED_PORT)
    }

    fn ensure_port_for(self: &Arc<Self>, domain: bool, fixed: u16) -> Result<u16> {
        let mut port = lock(&self.port);
        let found = match (*port, domain) {
            (Some(p), false) => Ok(p),
            (Some(p), true) if p == fixed => Ok(p),
            (_, true) => server::start_exact(self, fixed),
            (None, false) => server::start(self, fixed),
        };
        *lock(&self.port_error) = found.as_ref().err().cloned();
        if let Ok(p) = &found {
            *port = Some(*p);
        }
        found
    }

    pub fn port(&self) -> Option<u16> {
        *lock(&self.port)
    }

    pub fn nonce(&self) -> &str {
        &self.nonce
    }

    /// (own domain on, hostname, token saved)
    fn domain_settings(&self) -> (bool, String, bool) {
        self.with_db(|c| Ok((store::meta(c, "mode").as_deref() == Some("domain"), store::meta(c, "hostname").unwrap_or_default(), store::meta(c, "token_set").as_deref() == Some("1")))).unwrap_or_default()
    }

    // ---- rooms

    pub fn room(&self, code: &str) -> Arc<Room> {
        lock(&self.rooms).entry(code.to_string()).or_insert_with(|| Arc::new(Room::new(code))).clone()
    }

    fn existing_room(&self, code: &str) -> Option<Arc<Room>> {
        lock(&self.rooms).get(code).cloned()
    }

    pub fn drop_room(&self, code: &str) {
        lock(&self.rooms).remove(code);
    }

    pub fn meeting_by_code(&self, code: &str) -> Option<Stored> {
        self.with_db(|c| store::by_code(c, code)).ok().flatten()
    }

    /// The STUN and TURN servers the pages are given.
    pub fn ice(&self) -> Value {
        let (stun, turn) = self.ice_settings();
        let mut out: Vec<Value> = stun.iter().map(|u| json!({"urls": [u]})).collect();
        if let Some(t) = turn {
            out.push(json!({"urls": [t.url], "username": t.username, "credential": t.credential}));
        }
        Value::Array(out)
    }

    fn ice_settings(&self) -> (Vec<String>, Option<Turn>) {
        self.with_db(|c| {
            let stun = store::meta(c, "stun").and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok()).unwrap_or_else(|| DEFAULT_STUN.iter().map(|s| s.to_string()).collect());
            let turn = store::meta(c, "turn").and_then(|s| serde_json::from_str::<Turn>(&s).ok());
            Ok((stun, turn))
        })
        .unwrap_or_default()
    }

    // ---- what the page sees

    fn view(&self, m: Stored) -> Meeting {
        let (people, waiting, host_in, recording) = self.existing_room(&m.code).map(|r| r.counts()).unwrap_or((0, 0, false, false));
        Meeting {
            id: m.id, code: m.code, title: m.title, kind: m.kind, starts_at: m.starts_at, minutes: m.minutes, auto_record: m.auto_record, lobby: m.lobby,
            invitees: m.invitees, created_at: m.created_at, live: people > 0, host_in, people, waiting, recording,
        }
    }

    pub fn state(&self) -> MeetState {
        let (stun, turn) = self.ice_settings();
        let (domain, hostname, token_set) = self.domain_settings();
        let mut tunnel = self.tunnel.info();
        if let (true, Some(why)) = (domain, lock(&self.port_error).clone()) {
            tunnel = tunnel::Info { state: "error".into(), url: None, error: Some(why) };
        }
        MeetState {
            port: self.port().unwrap_or(0),
            tunnel,
            browser: browser::label(),
            recordings_dir: self.rec_dir().to_string_lossy().into_owned(),
            stun,
            turn,
            mode: if domain { "domain" } else { "quick" }.into(),
            hostname,
            token_set,
        }
    }

    pub fn list(&self) -> Result<Vec<Meeting>> {
        let now = now_ms();
        let mut all: Vec<Meeting> = self.with_db(|c| store::all(c))?.into_iter().map(|m| self.view(m)).collect();
        let upcoming = |m: &Meeting| m.live || m.starts_at.is_some_and(|t| t + m.minutes.max(30) * 60_000 > now);
        all.sort_by_key(|m| (!upcoming(m), if upcoming(m) { m.starts_at.unwrap_or(0) } else { -m.starts_at.unwrap_or(m.created_at) }));
        Ok(all)
    }

    fn fields(title: &str, kind: &str, starts_at: Option<i64>, minutes: i64, auto_record: bool, lobby: bool, invitees: Vec<InviteeIn>, keep: &[Invitee]) -> Result<Fields> {
        let kind = if kind == "call" { "call" } else { "meeting" };
        let title = title.trim();
        let title: String = if title.is_empty() { if kind == "call" { "Call".into() } else { "Meeting".into() } } else { title.chars().take(120).collect() };
        let mut seen = HashSet::new();
        let mut list = Vec::new();
        for i in invitees {
            let email = i.email.trim().to_string();
            if email.is_empty() {
                continue;
            }
            if !email.contains('@') || email.contains(char::is_whitespace) {
                return Err(format!("“{email}” isn't an email address."));
            }
            if !seen.insert(email.to_lowercase()) {
                continue;
            }
            let old = keep.iter().find(|k| k.email.eq_ignore_ascii_case(&email));
            list.push(Invitee { email, name: i.name.trim().chars().take(80).collect(), sent_at: old.and_then(|o| o.sent_at), error: old.and_then(|o| o.error.clone()) });
        }
        Ok(Fields { title, kind: kind.into(), starts_at, minutes: minutes.clamp(0, 24 * 60), auto_record, lobby, invitees: list })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create(&self, title: &str, kind: &str, starts_at: Option<i64>, minutes: i64, auto_record: bool, lobby: bool, invitees: Vec<InviteeIn>) -> Result<Meeting> {
        let f = Self::fields(title, kind, starts_at, minutes, auto_record, lobby, invitees, &[])?;
        let m = self.with_db(|c| store::create(c, &f, now_ms()))?;
        self.changed();
        Ok(self.view(m))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update(&self, id: &str, title: &str, kind: &str, starts_at: Option<i64>, minutes: i64, auto_record: bool, lobby: bool, invitees: Vec<InviteeIn>) -> Result<Meeting> {
        let old = self.with_db(|c| store::get(c, id))?;
        let f = Self::fields(title, kind, starts_at, minutes, auto_record, lobby, invitees, &old.invitees)?;
        let m = self.with_db(|c| store::update(c, id, &f))?;
        self.changed();
        Ok(self.view(m))
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        let m = self.with_db(|c| store::get(c, id))?;
        if let Some(r) = self.existing_room(&m.code) {
            let (people, waiting, ..) = r.counts();
            if people + waiting > 0 {
                return Err("This meeting is live. End it first.".into());
            }
        }
        self.drop_room(&m.code);
        self.with_db(|c| store::delete(c, id))?;
        self.changed();
        Ok(())
    }

    pub fn end(&self, id: &str) -> Result<()> {
        let m = self.with_db(|c| store::get(c, id))?;
        if let Some(r) = self.existing_room(&m.code) {
            r.end(self);
            self.drop_room(&m.code);
        }
        Ok(())
    }

    /// The link guests use: the tunnel's address. Starts the tunnel if it's off and waits up to 20 s.
    pub fn join_url(self: &Arc<Self>, id: &str) -> Result<String> {
        let m = self.with_db(|c| store::get(c, id))?;
        let port = self.ensure_port()?;
        if !self.tunnel.wanted() {
            self.start_tunnel(port)?;
        }
        let base = self.tunnel.wait_on(Duration::from_secs(20))?;
        Ok(format!("{base}/m/{}", m.code))
    }

    /// What to run for the current mode; the token is read from the keyring only now.
    fn tunnel_spec(&self, port: u16) -> Result<tunnel::Spec> {
        let (domain, hostname, _) = self.domain_settings();
        if !domain {
            return Ok(tunnel::Spec::Quick { port });
        }
        let token = secret::get(&self.data_dir).ok_or("The tunnel token is missing. Paste it again in Settings, Meet.")?;
        Ok(tunnel::Spec::Domain { hostname, token })
    }

    fn start_tunnel(self: &Arc<Self>, port: u16) -> Result<()> {
        let spec = self.tunnel_spec(port)?;
        let hub = self.clone();
        self.tunnel.start(spec, self.data_dir.clone(), Arc::new(move || hub.changed()));
        Ok(())
    }

    pub fn set_tunnel(self: &Arc<Self>, on: bool) -> Result<MeetState> {
        let port = self.ensure_port()?;
        if on {
            self.start_tunnel(port)?;
        } else {
            if self.list()?.iter().any(|m| m.live) {
                return Err("A meeting is live. End it before turning the tunnel off.".into());
            }
            self.tunnel.stop();
            self.changed();
        }
        Ok(self.state())
    }

    pub fn set_settings(&self, stun: Vec<String>, turn: Option<Turn>) -> Result<MeetState> {
        let stun: Vec<String> = stun.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        for s in &stun {
            if !(s.starts_with("stun:") || s.starts_with("stuns:")) {
                return Err(format!("“{s}” isn't a STUN address (it starts with stun:)."));
            }
        }
        let turn = turn.filter(|t| !t.url.trim().is_empty());
        if let Some(t) = &turn {
            if !(t.url.starts_with("turn:") || t.url.starts_with("turns:")) {
                return Err("The TURN address starts with turn: or turns:.".into());
            }
        }
        self.with_db(|c| {
            store::set_meta(c, "stun", &serde_json::to_string(&stun).map_err(|e| e.to_string())?)?;
            store::set_meta(c, "turn", &turn.as_ref().map(|t| serde_json::to_string(t).unwrap_or_default()).unwrap_or_default())
        })?;
        self.changed();
        Ok(self.state())
    }

    /// Quick link or the user's own domain. `hostname` and `token` of None keep what's saved;
    /// `clear_token` forgets the token. The tunnel restarts if it was on and something changed.
    pub fn set_domain(self: &Arc<Self>, mode: &str, hostname: Option<String>, token: Option<String>, clear_token: bool) -> Result<MeetState> {
        let domain = match mode {
            "domain" => true,
            "quick" => false,
            _ => return Err("The mode is quick or domain.".into()),
        };
        let (was_domain, old_host, had_token) = self.domain_settings();
        let hostname = match hostname.as_deref().map(str::trim).filter(|h| !h.is_empty()) {
            Some(h) => clean_hostname(h)?,
            None => old_host.clone(),
        };
        let token = match token.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
            Some(t) => Some(clean_token(t)?),
            None => None,
        };
        let has_token = token.is_some() || (had_token && !clear_token);
        if domain && hostname.is_empty() {
            return Err("Enter the hostname guests will use, like meet.example.com.".into());
        }
        if domain && !has_token {
            return Err("Paste the tunnel token first.".into());
        }
        let changed = domain != was_domain || hostname != old_host || token.is_some() || clear_token;
        let was_on = self.tunnel.wanted();
        if changed && was_on && self.list()?.iter().any(|m| m.live) {
            return Err("A meeting is live. End it before changing the link.".into());
        }
        // Before anything is saved: no point switching to a domain whose port can't be opened.
        let port = self.ensure_port_for(domain, server::PREFERRED_PORT)?;
        if let Some(t) = &token {
            secret::set(&self.data_dir, t)?;
        } else if clear_token {
            secret::clear(&self.data_dir);
        }
        self.with_db(|c| {
            store::set_meta(c, "mode", if domain { "domain" } else { "quick" })?;
            store::set_meta(c, "hostname", &hostname)?;
            store::set_meta(c, "token_set", if has_token { "1" } else { "" })
        })?;
        if changed && was_on {
            self.tunnel.stop();
            self.start_tunnel(port)?;
        }
        self.changed();
        Ok(self.state())
    }

    /// Settings' "Test": does `https://<hostname>/api/health` lead to this PC? Starts the tunnel
    /// first if it's off.
    pub fn check_domain(self: &Arc<Self>) -> Result<DomainCheck> {
        let (domain, hostname, _) = self.domain_settings();
        if !domain || hostname.is_empty() {
            return Err("Turn on “My own domain” and save it first.".into());
        }
        let port = self.ensure_port()?;
        if !self.tunnel.wanted() {
            self.start_tunnel(port)?;
        }
        if let Err(e) = self.tunnel.wait_on(Duration::from_secs(20)) {
            return Ok(DomainCheck { result: "unreachable".into(), message: e });
        }
        Ok(check_url(&format!("https://{hostname}/api/health"), &self.nonce, &hostname))
    }

    pub fn host_url(self: &Arc<Self>, id: &str) -> Result<String> {
        let m = self.with_db(|c| store::get(c, id))?;
        let port = self.ensure_port()?;
        Ok(format!("http://127.0.0.1:{port}/m/{}?host={}", m.code, self.host_token))
    }

    pub fn open(self: &Arc<Self>, id: &str) -> Result<()> {
        let url = self.host_url(id)?;
        browser::open(&url, &self.data_dir.join("meet-browser"))
    }

    // ---- recordings

    pub fn recordings(&self) -> Result<Vec<Recording>> {
        self.with_db(|c| store::recordings(c))
    }

    pub fn delete_recording(&self, id: &str) -> Result<()> {
        let r = self.with_db(|c| store::recording(c, id))?;
        let path = PathBuf::from(&r.path);
        // Only files in the recordings folder are ours to delete.
        if path.parent() == Some(self.rec_dir().as_path()) {
            let _ = std::fs::remove_file(&path);
            let _ = std::fs::remove_file(room::chat_path(&path));
            // Its tracks, transcript, subtitles and notes go with it.
            self.ai_cancel(id);
            let _ = std::fs::remove_dir_all(ai::paths(&path).tracks);
            for f in ai::extra_files(&path) {
                let _ = std::fs::remove_file(f);
            }
        }
        lock(&self.recs).remove(id);
        self.with_db(|c| store::delete_recording(c, id))?;
        self.changed();
        Ok(())
    }

    /// Appends a chunk of the host page's recording. Returns the next sequence number expected.
    pub fn rec_append(&self, code: &str, rec: &str, seq: u64, body: &[u8]) -> std::result::Result<u64, (&'static str, String)> {
        if rec.len() < 8 || rec.len() > 40 || !rec.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(("400 Bad Request", "bad recording id".into()));
        }
        let file = {
            let mut recs = lock(&self.recs);
            if let Some(f) = recs.get(rec) {
                f.clone()
            } else if seq == 0 {
                let m = self.meeting_by_code(code).ok_or(("404 Not Found", "no such meeting".to_string()))?;
                let (path, meeting_id, title) = self.room(code).claim_path(self);
                std::fs::File::create(&path).map_err(|e| ("500 Internal Server Error", e.to_string()))?;
                let now = now_ms();
                let row = Recording { id: rec.to_string(), meeting_id: if meeting_id.is_empty() { m.id } else { meeting_id }, title, path: path.to_string_lossy().into_owned(), started_at: now, ended_at: None, bytes: 0 };
                self.with_db(|c| store::add_recording(c, &row)).map_err(|e| ("500 Internal Server Error", e))?;
                let f = Arc::new(Mutex::new(RecFile { code: code.to_string(), id: rec.to_string(), path, next: 0, bytes: 0 }));
                recs.insert(rec.to_string(), f.clone());
                f
            } else {
                return Err(("404 Not Found", "unknown recording".into()));
            }
        };
        let mut f = lock(&file);
        if f.code != code {
            return Err(("403 Forbidden", "wrong meeting".into()));
        }
        if seq < f.next {
            return Ok(f.next);
        }
        if seq > f.next {
            return Err(("409 Conflict", format!("expected {}", f.next)));
        }
        let mut out = std::fs::OpenOptions::new().append(true).open(&f.path).map_err(|e| ("500 Internal Server Error", e.to_string()))?;
        out.write_all(body).map_err(|e| ("500 Internal Server Error", e.to_string()))?;
        f.next += 1;
        f.bytes += body.len() as i64;
        let (id, bytes) = (f.id.clone(), f.bytes);
        drop(out);
        let _ = self.with_db(|c| store::grow_recording(c, &id, bytes, now_ms()));
        if seq == 0 || seq % 5 == 0 {
            self.changed();
        }
        Ok(f.next)
    }

    // ---- invitations

    /// Sends the invitations; each invitee's `sentAt` or `error` is set.
    pub fn invite(self: &Arc<Self>, id: &str, account_id: &str, message: &str) -> Result<Vec<Invitee>> {
        let m = self.with_db(|c| store::get(c, id))?;
        if m.invitees.is_empty() {
            return Err("Add someone to invite first.".into());
        }
        let url = self.join_url(id)?;
        let (account_name, account_email) = crate::mail::account_address(account_id)?;
        let host = full_name_or(&account_name);
        let invites = self.send_all(&m, &url, account_id, (&host, &account_email), message, false);
        self.with_db(|c| store::set_invitees(c, id, &invites, Some(&url), Some(account_id)))?;
        self.changed();
        Ok(invites)
    }

    fn send_all(&self, m: &Stored, url: &str, account_id: &str, organizer: (&str, &str), message: &str, starting: bool) -> Vec<Invitee> {
        let when = m.starts_at.map(when_text);
        let mail = invite::compose(m, url, organizer.0, when.as_deref(), message, starting);
        let files = if starting {
            Vec::new()
        } else {
            invite::ics(m, url, organizer, now_ms()).map(|ics| vec![("invite.ics".to_string(), "text/calendar; method=REQUEST; charset=UTF-8".to_string(), ics.into_bytes())]).unwrap_or_default()
        };
        m.invitees
            .iter()
            .map(|i| {
                let mut out = i.clone();
                match crate::mail::send_message(account_id, (&i.name, &i.email), &mail.subject, &mail.html, &mail.text, files.clone()) {
                    Ok(()) => {
                        out.sent_at = Some(now_ms());
                        out.error = None;
                    }
                    Err(e) => out.error = Some(e),
                }
                out
            })
            .collect()
    }

    // ---- scheduled meetings

    /// Every 30 s: five minutes before a meeting, the tunnel is made sure of, the invitees get
    /// the new link if it changed, and the host gets a notification.
    pub fn tick(self: &Arc<Self>) {
        self.ai_tick();
        let now = now_ms();
        let Ok(all) = self.with_db(|c| store::all(c)) else { return };
        for m in all {
            let Some(start) = m.starts_at else { continue };
            if now < start - 5 * 60_000 || now > start + m.minutes.max(30) * 60_000 {
                continue;
            }
            let port = match self.ensure_port() {
                Ok(p) => p,
                Err(_) => continue,
            };
            if !self.tunnel.wanted() && self.start_tunnel(port).is_err() {
                continue;
            }
            if lock(&self.notified).insert(m.id.clone()) {
                self.host.starting(self, &m);
            }
            let Some(base) = self.tunnel.info().url else { continue };
            let url = format!("{base}/m/{}", m.code);
            if let (Some(old), Some(account)) = (m.invite_url.as_deref(), m.invite_account.as_deref()) {
                if needs_resend(&m, old, &url) {
                    if let Ok((name, email)) = crate::mail::account_address(account) {
                        let host = full_name_or(&name);
                        let sent = self.send_all(&m, &url, account, (&host, &email), "", true);
                        let _ = self.with_db(|c| {
                            store::set_invitees(c, &m.id, &sent, None, None)?;
                            store::set_started_mail(c, &m.id, &url)
                        });
                        self.changed();
                    }
                }
            }
        }
    }
}

/// Whether the invitees must be told the link changed: not once they've had the start-time
/// mail, not when the address is the one they were invited with (always so with a domain).
fn needs_resend(m: &Stored, invited_with: &str, url: &str) -> bool {
    !m.started_mail && invited_with != url && !m.invitees.is_empty()
}

/// A plain DNS name, lowercased: `meet.example.com`. No scheme, path, port or address.
pub fn clean_hostname(raw: &str) -> Result<String> {
    let bad = || format!("“{}” isn't a hostname. Use just the name, like meet.example.com.", raw.chars().take(60).collect::<String>());
    let host = raw.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() || host.len() > 253 {
        return Err(bad());
    }
    let labels: Vec<&str> = host.split('.').collect();
    let ok = labels.len() >= 2
        && labels.iter().all(|l| !l.is_empty() && l.len() <= 63 && !l.starts_with('-') && !l.ends_with('-') && l.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'))
        && !labels.last().is_some_and(|t| t.bytes().all(|b| b.is_ascii_digit()));
    if ok { Ok(host) } else { Err(bad()) }
}

/// The token, from what was pasted: the long string, or the whole install command (its last word).
fn clean_token(raw: &str) -> Result<String> {
    let token = raw.split_whitespace().last().unwrap_or("");
    let ok = token.len() >= 20 && token.len() <= 4096 && token.bytes().all(|b| b.is_ascii_alphanumeric() || b"+/=_-.".contains(&b));
    if ok { Ok(token.to_string()) } else { Err("That doesn't look like a tunnel token. Copy the long string after --token in Cloudflare's install command.".into()) }
}

/// Fetches `url` (10 s) and says whether it is this PC's meeting server (its `nonce` shows).
pub fn check_url(url: &str, nonce: &str, hostname: &str) -> DomainCheck {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(10))).max_redirects(0).http_status_as_error(false).build().into();
    match agent.get(url).header("User-Agent", "Zazabor").call() {
        Ok(response) => {
            let status = response.status().as_u16();
            let body = response.into_body().with_config().limit(64 << 10).read_to_string().unwrap_or_default();
            classify_health(status, &body, nonce, hostname)
        }
        Err(e) => {
            let text = e.to_string().to_lowercase();
            let message = if matches!(e, ureq::Error::HostNotFound) || text.contains("host not found") || text.contains("lookup") || text.contains("resolve") {
                format!("{hostname} doesn't exist yet. Check the hostname, and that the domain's DNS is on Cloudflare and the tunnel has a public hostname for it.")
            } else if text.contains("certificate") || text.contains("tls") || text.contains("handshake") {
                format!("{hostname} has a certificate problem. Cloudflare issues it a few minutes after the hostname is added; try again shortly.")
            } else if text.contains("timeout") || text.contains("timed out") {
                format!("{hostname} didn't answer within 10 seconds.")
            } else {
                format!("Couldn't reach {hostname}: {e}")
            };
            DomainCheck { result: "unreachable".into(), message }
        }
    }
}

fn classify_health(status: u16, body: &str, nonce: &str, hostname: &str) -> DomainCheck {
    let out = |result: &str, message: String| DomainCheck { result: result.into(), message };
    if matches!(status, 502 | 503 | 504 | 521 | 522 | 523 | 530 | 1033) {
        return out("unreachable", format!("Cloudflare answers for {hostname} (error {status}), but nothing is behind it: the tunnel isn't connected, or its service URL isn't localhost:47230. Wait a few seconds and try again."));
    }
    let json: Option<Value> = serde_json::from_str(body).ok();
    let app = json.as_ref().and_then(|j| j.get("app")).and_then(Value::as_str) == Some("zazabor-meet");
    if status == 200 && app {
        if json.as_ref().and_then(|j| j.get("nonce")).and_then(Value::as_str) == Some(nonce) {
            return out("this-pc", format!("It works: guests who open {hostname} reach this PC."));
        }
        return out("other", format!("{hostname} reaches another Zazabor Meet, not this PC. Is the tunnel also running somewhere else?"));
    }
    out("other", format!("{hostname} answers (status {status}), but it isn't this PC's meeting server. Check the tunnel's public hostname points at this tunnel."))
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct InviteeIn {
    pub email: String,
    #[serde(default)]
    pub name: String,
}

// ---- small helpers shared by the modules

fn local_tm(ms: i64) -> libc::tm {
    let secs = ms.div_euclid(1000) as libc::time_t;
    // SAFETY: `tm` is plain data; localtime_r only writes to it.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&secs, &mut tm);
        tm
    }
}

/// `2026-11-21 14.30` for a file name.
pub fn stamp_for_file(ms: i64) -> String {
    let tm = local_tm(ms);
    format!("{:04}-{:02}-{:02} {:02}.{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min)
}

/// `Sat 21 Nov 2026, 14:30 (UTC+6)`
pub fn when_text(ms: i64) -> String {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let tm = local_tm(ms);
    let off = tm.tm_gmtoff / 60;
    let zone = if off == 0 { "UTC".to_string() } else { format!("UTC{}{}{}", if off < 0 { '-' } else { '+' }, off.abs() / 60, if off % 60 != 0 { format!(":{:02}", off.abs() % 60) } else { String::new() }) };
    format!("{} {} {} {}, {:02}:{:02} ({zone})", DAYS[tm.tm_wday as usize % 7], tm.tm_mday, MONTHS[tm.tm_mon as usize % 12], tm.tm_year + 1900, tm.tm_hour, tm.tm_min)
}

/// A title as a file name: no slashes or other characters file managers dislike.
pub fn file_safe(title: &str) -> String {
    let s: String = title.chars().map(|c| if c.is_control() || "/\\:*?\"<>|".contains(c) { ' ' } else { c }).collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let s: String = s.chars().take(60).collect();
    let s = s.trim().trim_start_matches(|c: char| c == '.' || c.is_whitespace()).to_string();
    if s.is_empty() {
        "Meeting".into()
    } else {
        s
    }
}

/// The system user's full name (the GECOS field), else the login name.
pub fn full_name() -> String {
    full_name_or("Host")
}

fn full_name_or(fallback: &str) -> String {
    let mut buf = vec![0 as libc::c_char; 4096];
    // SAFETY: an all-zero passwd is a valid out-parameter for getpwuid_r.
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: the buffer and out-pointers outlive the call; strings are copied before they drop.
    let rc = unsafe { libc::getpwuid_r(libc::getuid(), &mut pwd, buf.as_mut_ptr(), buf.len(), &mut result) };
    if rc == 0 && !result.is_null() {
        // SAFETY: on success these point at NUL-terminated strings inside `buf`.
        let gecos = unsafe { std::ffi::CStr::from_ptr(pwd.pw_gecos) }.to_string_lossy().into_owned();
        let name = gecos.split(',').next().unwrap_or("").trim().to_string();
        if !name.is_empty() {
            return name;
        }
    }
    if fallback.is_empty() { "Host".into() } else { fallback.to_string() }
}

// ---- the app

static HUB: OnceLock<Arc<Hub>> = OnceLock::new();

struct AppHost(AppHandle);

impl Host for AppHost {
    fn emit(&self, event: &str, payload: Value) {
        let _ = self.0.emit(event, payload);
    }

    fn starting(&self, hub: &Arc<Hub>, m: &Stored) {
        let (hub, id) = (hub.clone(), m.id.clone());
        let when = m.starts_at.map(when_text).unwrap_or_default();
        let click: Box<dyn Fn() + Send + 'static> = Box::new(move || {
            let (hub, id) = (hub.clone(), id.clone());
            std::thread::spawn(move || {
                if let Err(e) = hub.open(&id) {
                    eprintln!("Meet: {e}");
                }
            });
        });
        crate::desktop_notify::show(crate::desktop_notify::Note { title: &format!("{} is about to start", m.title), body: &format!("{when}. Click to start the meeting."), icon: "camera-web", category: "x-zazabor.meet", replaces: 0 }, Some(click));
    }
}

/// Opens `meet.db`; the server starts when the page first asks, and a clock watches the scheduled meetings.
pub fn init(app: AppHandle, data_dir: PathBuf) {
    let conn = match store::open(&data_dir.join("meet.db")) {
        Ok(c) => c,
        Err(err) => {
            eprintln!("Meet: {err}");
            return;
        }
    };
    let rec_dir = app.path().video_dir().ok().or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Videos"))).unwrap_or_else(std::env::temp_dir).join("Zazabor Meetings");
    match Hub::new(Box::new(AppHost(app)), conn, data_dir, rec_dir) {
        Ok(hub) => {
            let _ = HUB.set(hub.clone());
            // Recordings made before 0.3.5 have no duration or cues: fix them once, quietly.
            let fixer = hub.clone();
            std::thread::spawn(move || fixer.make_old_recordings_seekable());
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_secs(30));
                hub.tick();
            });
        }
        Err(err) => eprintln!("Meet: {err}"),
    }
}

fn the_hub() -> Result<Arc<Hub>> {
    HUB.get().cloned().ok_or_else(|| "Meet isn't available.".to_string())
}

async fn blocking<T: Send + 'static>(f: impl FnOnce(Arc<Hub>) -> Result<T> + Send + 'static) -> Result<T> {
    crate::ensure_unlocked()?;
    let hub = the_hub()?;
    tauri::async_runtime::spawn_blocking(move || f(hub)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn meet_state() -> Result<MeetState> {
    blocking(|hub| {
        // A busy port 47230 in domain mode shows in the state's tunnel error.
        let _ = hub.ensure_port();
        Ok(hub.state())
    })
    .await
}

#[tauri::command]
pub async fn meet_list() -> Result<Vec<Meeting>> {
    blocking(|hub| hub.list()).await
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn meet_create(title: String, kind: String, starts_at: Option<i64>, minutes: i64, auto_record: bool, lobby: bool, invitees: Vec<InviteeIn>) -> Result<Meeting> {
    blocking(move |hub| hub.create(&title, &kind, starts_at, minutes, auto_record, lobby, invitees)).await
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn meet_update(id: String, title: String, kind: String, starts_at: Option<i64>, minutes: i64, auto_record: bool, lobby: bool, invitees: Vec<InviteeIn>) -> Result<Meeting> {
    blocking(move |hub| hub.update(&id, &title, &kind, starts_at, minutes, auto_record, lobby, invitees)).await
}

#[tauri::command]
pub async fn meet_delete(id: String) -> Result<()> {
    blocking(move |hub| hub.delete(&id)).await
}

#[tauri::command]
pub async fn meet_join_url(id: String) -> Result<String> {
    blocking(move |hub| hub.join_url(&id)).await
}

#[tauri::command]
pub async fn meet_open(id: String) -> Result<()> {
    blocking(move |hub| hub.open(&id)).await
}

#[tauri::command]
pub async fn meet_invite(id: String, account_id: String, message: String) -> Result<Vec<Invitee>> {
    blocking(move |hub| hub.invite(&id, &account_id, &message)).await
}

#[tauri::command]
pub async fn meet_end(id: String) -> Result<()> {
    blocking(move |hub| hub.end(&id)).await
}

#[tauri::command]
pub async fn meet_recordings() -> Result<Vec<Recording>> {
    blocking(|hub| hub.recordings()).await
}

#[tauri::command]
pub async fn meet_recording_delete(id: String) -> Result<()> {
    blocking(move |hub| hub.delete_recording(&id)).await
}

// ---- transcripts and notes (ai.rs)

#[tauri::command]
pub async fn meet_ai_settings() -> Result<ai::Settings> {
    blocking(|hub| Ok(hub.ai_settings())).await
}

#[tauri::command]
pub async fn meet_ai_set_settings(settings: ai::Settings) -> Result<ai::Settings> {
    blocking(move |hub| hub.set_ai_settings(&settings)).await
}

#[tauri::command]
pub async fn meet_ai_status() -> Result<Vec<ai::Info>> {
    blocking(|hub| hub.ai_infos()).await
}

#[tauri::command]
pub async fn meet_ai_models() -> Result<Vec<crate::ai::models::Status>> {
    blocking(|hub| Ok(hub.ai_models())).await
}

/// Starts (or continues) a model download; the page asked the user about its size first.
#[tauri::command]
pub async fn meet_ai_model_download(id: String) -> Result<()> {
    blocking(move |hub| hub.ai_model_download(&id)).await
}

#[tauri::command]
pub async fn meet_ai_model_cancel(id: String) -> Result<()> {
    crate::ai::models::cancel(&id);
    Ok(())
}

#[tauri::command]
pub async fn meet_ai_model_remove(id: String) -> Result<()> {
    blocking(move |hub| crate::ai::models::remove(&hub.data_dir, &id)).await
}

#[tauri::command]
pub async fn meet_ai_run(id: String, options: Option<ai::RunOptions>) -> Result<()> {
    blocking(move |hub| hub.ai_start(&id, options.unwrap_or_default())).await
}

#[tauri::command]
pub async fn meet_ai_cancel(id: String) -> Result<()> {
    blocking(move |hub| {
        hub.ai_cancel(&id);
        Ok(())
    })
    .await
}

/// The cloud providers and whether a key is saved for each (never the key).
#[tauri::command]
pub async fn meet_ai_cloud_state() -> Result<Vec<ai::ProviderState>> {
    blocking(|hub| Ok(hub.cloud_state())).await
}

#[tauri::command]
pub async fn meet_ai_cloud_key_set(provider: String, key: String) -> Result<Vec<ai::ProviderState>> {
    blocking(move |hub| hub.cloud_key_set(&provider, &key)).await
}

#[tauri::command]
pub async fn meet_ai_cloud_key_clear(provider: String) -> Result<Vec<ai::ProviderState>> {
    blocking(move |hub| hub.cloud_key_clear(&provider)).await
}

/// Settings' "Test key": a tiny request with the saved key and the provider's notes model.
#[tauri::command]
pub async fn meet_ai_cloud_test(provider: String) -> Result<String> {
    let (client, model) = blocking(move |hub| hub.cloud_test_client(&provider)).await?;
    client.ping(&std::sync::atomic::AtomicBool::new(false)).await.map_err(|e| e.text())?;
    Ok(format!("The key works with {model}."))
}

#[tauri::command]
pub async fn meet_ai_off(id: String, off: bool) -> Result<()> {
    blocking(move |hub| hub.ai_off(&id, off)).await
}

#[tauri::command]
pub async fn meet_transcript(id: String) -> Result<transcript::Doc> {
    blocking(move |hub| hub.transcript(&id)).await
}

#[tauri::command]
pub async fn meet_notes(id: String) -> Result<Option<Value>> {
    blocking(move |hub| hub.notes(&id)).await
}

#[tauri::command]
pub async fn meet_notes_save(id: String, notes: Value) -> Result<()> {
    blocking(move |hub| hub.notes_save(&id, notes)).await
}

#[tauri::command]
pub async fn meet_tunnel(on: bool) -> Result<MeetState> {
    blocking(move |hub| hub.set_tunnel(on)).await
}

#[tauri::command]
pub async fn meet_settings(stun: Vec<String>, turn: Option<Turn>) -> Result<MeetState> {
    blocking(move |hub| hub.set_settings(stun, turn)).await
}

#[tauri::command]
pub async fn meet_domain(mode: String, hostname: Option<String>, token: Option<String>, clear_token: Option<bool>) -> Result<MeetState> {
    blocking(move |hub| hub.set_domain(&mode, hostname, token, clear_token.unwrap_or(false))).await
}

#[tauri::command]
pub async fn meet_domain_test() -> Result<DomainCheck> {
    blocking(|hub| hub.check_domain()).await
}


impl Hub {
    /// Rewrites one finished recording so players know its length and can seek (remux.rs), and
    /// remembers that it's done.
    pub(crate) fn make_seekable(&self, rec_id: &str) {
        let Ok(r) = self.with_db(|c| store::recording(c, rec_id)) else { return };
        let key = format!("seekable:{rec_id}");
        if r.ended_at.is_none() || self.with_db(|c| Ok(store::meta(c, &key).is_some())).unwrap_or(false) {
            return;
        }
        let path = PathBuf::from(&r.path);
        if !path.exists() {
            return;
        }
        match remux::make_seekable(&path) {
            Ok(()) => {
                let _ = self.with_db(|c| store::set_meta(c, &key, "1"));
                if let Ok(size) = std::fs::metadata(&path).map(|m| m.len() as i64) {
                    let _ = self.with_db(|c| store::grow_recording(c, rec_id, size, r.ended_at.unwrap_or_else(now_ms)));
                }
                self.changed();
            }
            Err(err) => eprintln!("Meet: recording {rec_id} stays unseekable: {err}"),
        }
    }

    /// Every finished recording that hasn't been made seekable yet, oldest first.
    fn make_old_recordings_seekable(&self) {
        let Ok(all) = self.with_db(|c| store::recordings(c)) else { return };
        for r in all.iter().rev() {
            self.make_seekable(&r.id);
        }
    }
}

#[cfg(test)]
mod tests;

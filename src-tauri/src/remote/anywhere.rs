//! Any network (docs/plans/remote.md "Any network"): Phone Link and Remote away from home,
//! through the user's own domain (`link.<domain>`) on the named Cloudflare tunnel Meet already
//! has a token for.
//!
//! A small HTTP server on `127.0.0.1:47214` (loopback only: only cloudflared reaches it) answers
//! `/health` and turns WebSockets into plain byte pipes to this PC's own listeners:
//!
//! - `/ws/link` → the ZLNK listener (Phone Link: texts, files, notifications, `remote.start`)
//! - `/ws/remote/<port>` → a remote session's one-shot port, only while that port waits for its
//!   phone
//!
//! The bridge never reads what it carries: the ZLNK handshake and its sealing (per-phone link
//! key, ChaCha20-Poly1305) run end to end inside the pipe, so Cloudflare only ever sees
//! ciphertext, and a client that isn't a paired phone gets nothing. Connections that come
//! through the bridge are marked (`is_bridged`), so the link refuses to pair over the internet
//! and the remote pill can say where the phone is.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};

use super::{lock, store};
use crate::meet::tunnel::{Info, Spec, Tunnel};

type Result<T> = std::result::Result<T, String>;

/// The port the tunnel's public hostname points at (`http://localhost:47214`).
pub const PORT: u16 = 47_214;
/// A WebSocket message from the phone: a 4 MiB keyframe never comes this way (it goes to the
/// phone), so input and file chunks fit with room to spare.
const MAX_MESSAGE: u64 = 8 << 20;
/// Bytes per message towards the phone.
const CHUNK: usize = 64 << 10;
/// WebSocket connections one address may open in `WINDOW`.
const PER_WINDOW: usize = 30;
const WINDOW: Duration = Duration::from_secs(600);
/// Pipes open at once.
const MAX_PIPES: usize = 16;

static SERVER: Mutex<Option<Arc<AtomicBool>>> = Mutex::new(None);
/// The hostname the running tunnel serves, so saving another setting doesn't restart it.
static SERVING: Mutex<Option<String>> = Mutex::new(None);
/// Why the internet path isn't running although it's switched on, for Settings.
static PROBLEM: Mutex<Option<String>> = Mutex::new(None);
static TUNNEL: OnceLock<Arc<Tunnel>> = OnceLock::new();
/// The local ends of the bridge's connections to the link listener, as the listener sees them.
static BRIDGED: Mutex<Option<HashSet<SocketAddr>>> = Mutex::new(None);
/// One-shot remote ports the bridge may reach, until when.
static REMOTE_PORTS: Mutex<Option<HashMap<u16, Instant>>> = Mutex::new(None);
static HITS: Mutex<Option<HashMap<String, VecDeque<Instant>>>> = Mutex::new(None);
static PIPES: AtomicUsize = AtomicUsize::new(0);
static NONCE: OnceLock<String> = OnceLock::new();

fn tunnel() -> &'static Arc<Tunnel> {
    TUNNEL.get_or_init(Tunnel::new)
}

fn nonce() -> &'static str {
    NONCE.get_or_init(|| crate::phonelink::link::hex(&super::random::<12>()))
}

// ---- what the rest of the app asks ----------------------------------------------------------------

/// Whether a connection to one of this PC's listeners came through the bridge (from the internet).
pub fn is_bridged(peer: &SocketAddr) -> bool {
    lock(&BRIDGED).as_ref().is_some_and(|s| s.contains(peer))
}

/// Lets the bridge reach a remote session's one-shot port until `until`.
pub fn allow_remote_port(port: u16, until: Instant) {
    lock(&REMOTE_PORTS).get_or_insert_with(HashMap::new).insert(port, until);
}

pub fn forbid_remote_port(port: u16) {
    if let Some(m) = lock(&REMOTE_PORTS).as_mut() {
        m.remove(&port);
    }
}

fn remote_port_open(port: u16) -> bool {
    let mut g = lock(&REMOTE_PORTS);
    let Some(m) = g.as_mut() else { return false };
    let now = Instant::now();
    m.retain(|_, until| *until > now);
    m.contains_key(&port)
}

/// Where a phone away from home dials the link (`wss://link.example.com/ws/link`), when the
/// internet path is on.
pub fn link_url() -> Option<String> {
    let s = store::get();
    (s.internet && !s.internet_host.is_empty()).then(|| format!("wss://{}/ws/link", s.internet_host))
}

/// Where the phone dials a remote session's port over the internet.
pub fn remote_url(port: u16) -> Option<String> {
    let s = store::get();
    (s.internet && !s.internet_host.is_empty()).then(|| format!("wss://{}/ws/remote/{port}", s.internet_host))
}

/// Tells a phone that just connected (or every connected phone, after a change) where to find
/// this PC from anywhere. An empty address means "not any more".
pub fn tell(link: &crate::phonelink::link::Link) {
    let _ = link.send(json!({"t": "net", "wss": link_url().unwrap_or_default()}));
}

fn tell_all() {
    let Some(hub) = crate::phonelink::hub() else { return };
    for link in hub.links() {
        tell(&link);
    }
}

// ---- switching it on and off -------------------------------------------------------------------

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub on: bool,
    pub tunnel: Info,
    /// What to type into Cloudflare, when the hostname is set.
    pub service: String,
    /// Meet's own hostname, to suggest `link.<domain>`.
    pub meet_host: String,
    /// Whether Meet has the named tunnel's token (this path needs it).
    pub has_token: bool,
    /// Why it isn't running although it's switched on.
    pub problem: Option<String>,
}

pub fn status() -> Status {
    let s = store::get();
    let named = crate::meet::named_tunnel();
    Status {
        on: s.internet,
        tunnel: tunnel().info(),
        service: format!("http://localhost:{PORT}"),
        meet_host: crate::meet::own_hostname().unwrap_or_default(),
        has_token: named.is_some(),
        problem: lock(&PROBLEM).clone(),
    }
}

/// Brings the server and the tunnel in line with the settings (on start, and after each change).
pub fn apply() -> Result<()> {
    let result = apply_inner();
    *lock(&PROBLEM) = result.as_ref().err().cloned();
    super::emit_changed();
    result
}

fn apply_inner() -> Result<()> {
    let s = store::get();
    if !(s.enabled && s.internet) || s.internet_host.is_empty() {
        stop();
        tell_all();
        return Ok(());
    }
    let (token, data_dir) = crate::meet::named_tunnel().ok_or("The internet path uses Meet's tunnel token. Set up “My own domain” in Settings → Meet first.")?;
    start_server()?;
    let t = tunnel();
    let mut serving = lock(&SERVING);
    if t.wanted() && serving.as_deref() == Some(s.internet_host.as_str()) {
        return Ok(());
    }
    t.stop();
    t.start(Spec::Domain { hostname: s.internet_host.clone(), token }, data_dir, Arc::new(super::emit_changed));
    *serving = Some(s.internet_host.clone());
    drop(serving);
    tell_all();
    Ok(())
}

pub fn stop() {
    *lock(&SERVING) = None;
    if let Some(stop) = lock(&SERVER).take() {
        stop.store(true, Ordering::SeqCst);
    }
    tunnel().stop();
}

fn start_server() -> Result<()> {
    let mut server = lock(&SERVER);
    if server.is_some() {
        return Ok(());
    }
    let listener = TcpListener::bind(("127.0.0.1", PORT)).map_err(|e| format!("Port {PORT} is taken, so the internet path can't start: {e}"))?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    std::thread::Builder::new()
        .name("remote-anywhere".into())
        .spawn(move || {
            while !stopped.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = std::thread::Builder::new().name("remote-anywhere-conn".into()).spawn(move || {
                            if let Err(e) = handle(stream, crate::phonelink::hub().and_then(|h| h.port())) {
                                eprintln!("Remote (internet): {e}");
                            }
                        });
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(100)),
                    Err(e) => {
                        eprintln!("Remote (internet): {e}");
                        std::thread::sleep(Duration::from_millis(500));
                    }
                }
            }
        })
        .map_err(|e| e.to_string())?;
    *server = Some(stop);
    Ok(())
}

/// Settings' Test: does `https://<host>/health` reach this PC?
pub fn check() -> Result<Value> {
    let s = store::get();
    if !s.internet || s.internet_host.is_empty() {
        return Err("Turn on “Over the internet” and save a hostname first.".into());
    }
    if let Err(e) = tunnel().wait_on(Duration::from_secs(20)) {
        return Ok(json!({"ok": false, "message": e}));
    }
    let url = format!("https://{}/health", s.internet_host);
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(10))).max_redirects(0).http_status_as_error(false).build().into();
    let (ok, message) = match agent.get(&url).header("User-Agent", "Zazabor").call() {
        Ok(r) => {
            let status = r.status().as_u16();
            let body = r.into_body().with_config().limit(16 << 10).read_to_string().unwrap_or_default();
            let v: Value = serde_json::from_str(&body).unwrap_or_default();
            if status == 200 && v.get("app").and_then(Value::as_str) == Some("zazabor-link") && v.get("nonce").and_then(Value::as_str) == Some(nonce()) {
                (true, format!("It works: phones reach this PC through {} from any network.", s.internet_host))
            } else if status == 200 && v.get("app").and_then(Value::as_str) == Some("zazabor-link") {
                (false, format!("{} leads to Zazabor on another PC.", s.internet_host))
            } else if matches!(status, 502 | 503 | 504 | 521 | 522 | 523 | 530) {
                (false, format!("Cloudflare answers for {} (error {status}), but nothing is behind it. In the tunnel's public hostname, the service must be http://localhost:{PORT}.", s.internet_host))
            } else if status == 200 && body.contains("zazabor-meet") {
                (false, format!("{} leads to the meeting server. Point it at http://localhost:{PORT} instead.", s.internet_host))
            } else {
                (false, format!("{} answered with error {status}.", s.internet_host))
            }
        }
        Err(e) => (false, format!("Couldn't reach {}: {e}. Check that the tunnel has a public hostname for it.", s.internet_host)),
    };
    Ok(json!({"ok": ok, "message": message}))
}

// ---- the HTTP side ----------------------------------------------------------------------------

/// What a request asked for.
#[derive(Debug, PartialEq)]
pub struct Request {
    pub method: String,
    pub path: String,
    /// Header names in lower case.
    pub headers: HashMap<String, String>,
}

impl Request {
    fn header(&self, name: &str) -> &str {
        self.headers.get(name).map(String::as_str).unwrap_or("")
    }

    fn wants_websocket(&self) -> bool {
        self.header("upgrade").eq_ignore_ascii_case("websocket") && self.header("connection").to_ascii_lowercase().contains("upgrade") && !self.header("sec-websocket-key").is_empty()
    }

    /// Who is calling: Cloudflare's view of the client, else the socket's.
    fn client(&self) -> String {
        let ip = self.header("cf-connecting-ip");
        if ip.is_empty() { "local".into() } else { ip.to_string() }
    }
}

/// Reads a request's head (at most 8 KiB).
pub fn read_request(r: &mut impl BufRead) -> Result<Request> {
    let mut lines = Vec::new();
    let mut total = 0;
    loop {
        let mut line = Vec::new();
        let n = r.take(4096).read_until(b'\n', &mut line).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("closed before the request ended".into());
        }
        total += n;
        if total > 8192 {
            return Err("request head too long".into());
        }
        let text = String::from_utf8_lossy(&line).trim_end_matches(['\r', '\n']).to_string();
        if text.is_empty() {
            break;
        }
        lines.push(text);
    }
    let first = lines.first().ok_or("empty request")?;
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    let headers = lines[1..].iter().filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())).collect();
    Ok(Request { method, path, headers })
}

fn respond(out: &mut TcpStream, status: &str, body: &str) {
    let _ = write!(out, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    let _ = out.flush();
}

/// Counts a connection from `client`; false once it has used up its share.
pub fn admit(hits: &mut HashMap<String, VecDeque<Instant>>, client: &str, now: Instant) -> bool {
    hits.retain(|_, q| q.back().is_some_and(|t| now.duration_since(*t) < WINDOW));
    let q = hits.entry(client.to_string()).or_default();
    while q.front().is_some_and(|t| now.duration_since(*t) >= WINDOW) {
        q.pop_front();
    }
    if q.len() >= PER_WINDOW {
        return false;
    }
    q.push_back(now);
    true
}

/// Where a WebSocket path leads on this PC, if anywhere.
pub fn route(path: &str, link_port: Option<u16>, remote_open: impl Fn(u16) -> bool) -> Option<(u16, bool)> {
    let path = path.split('?').next().unwrap_or("");
    if path == "/ws/link" {
        return link_port.map(|p| (p, true));
    }
    let port: u16 = path.strip_prefix("/ws/remote/")?.parse().ok()?;
    remote_open(port).then_some((port, false))
}

/// One connection to the server: `/health`, or a WebSocket piped to `link_port` (the ZLNK
/// listener) or to a remote session's port.
pub fn handle(stream: TcpStream, link_port: Option<u16>) -> Result<()> {
    stream.set_nonblocking(false).map_err(|e| e.to_string())?;
    stream.set_read_timeout(Some(Duration::from_secs(15))).map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
    let mut out = stream.try_clone().map_err(|e| e.to_string())?;
    let req = read_request(&mut reader)?;
    if req.method != "GET" {
        respond(&mut out, "405 Method Not Allowed", "{}");
        return Ok(());
    }
    if req.path.split('?').next() == Some("/health") {
        respond(&mut out, "200 OK", &json!({"app": "zazabor-link", "nonce": nonce()}).to_string());
        return Ok(());
    }
    if !req.wants_websocket() {
        respond(&mut out, "404 Not Found", "{}");
        return Ok(());
    }
    if !admit(lock(&HITS).get_or_insert_with(HashMap::new), &req.client(), Instant::now()) {
        respond(&mut out, "429 Too Many Requests", "{}");
        return Err(format!("too many connections from {}", req.client()));
    }
    let Some((port, is_link)) = route(&req.path, link_port, remote_port_open) else {
        respond(&mut out, "404 Not Found", "{}");
        return Ok(());
    };
    if PIPES.load(Ordering::SeqCst) >= MAX_PIPES {
        respond(&mut out, "503 Service Unavailable", "{}");
        return Err("too many pipes open".into());
    }
    let target = TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), Duration::from_secs(3)).map_err(|e| {
        respond(&mut out, "502 Bad Gateway", "{}");
        format!("couldn't reach port {port}: {e}")
    })?;
    let accept = tungstenite::handshake::derive_accept_key(req.header("sec-websocket-key").as_bytes());
    write!(out, "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n").map_err(|e| e.to_string())?;
    out.flush().map_err(|e| e.to_string())?;
    stream.set_read_timeout(None).map_err(|e| e.to_string())?;
    // Marked before the first byte goes through, so the listener knows by the time it reads.
    let local = target.local_addr().ok();
    if is_link {
        if let Some(a) = local {
            lock(&BRIDGED).get_or_insert_with(HashSet::new).insert(a);
        }
    } else {
        forbid_remote_port(port);
    }
    PIPES.fetch_add(1, Ordering::SeqCst);
    let result = pipe(reader, stream, target);
    PIPES.fetch_sub(1, Ordering::SeqCst);
    if let Some(a) = local {
        if let Some(s) = lock(&BRIDGED).as_mut() {
            s.remove(&a);
        }
    }
    result
}

/// Pumps bytes both ways until either side ends: WebSocket messages in, a byte stream out to the
/// listener, and the listener's bytes back as binary messages.
fn pipe(mut ws_in: BufReader<TcpStream>, ws: TcpStream, target: TcpStream) -> Result<()> {
    target.set_nodelay(true).ok();
    ws.set_nodelay(true).ok();
    let ws_out = Arc::new(Mutex::new(ws.try_clone().map_err(|e| e.to_string())?));
    let back = {
        let (ws_out, mut from) = (ws_out.clone(), target.try_clone().map_err(|e| e.to_string())?);
        let ws = ws.try_clone().map_err(|e| e.to_string())?;
        std::thread::Builder::new()
            .name("remote-anywhere-out".into())
            .spawn(move || {
                let mut buf = vec![0u8; CHUNK];
                loop {
                    let n = match from.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    if write_frame(&mut *lock(&ws_out), OP_BINARY, &buf[..n], None).is_err() {
                        break;
                    }
                }
                let _ = write_frame(&mut *lock(&ws_out), OP_CLOSE, &[], None);
                let _ = ws.shutdown(Shutdown::Both);
            })
            .map_err(|e| e.to_string())?
    };
    let mut to = target.try_clone().map_err(|e| e.to_string())?;
    let result = loop {
        // A read that fails is the phone or the listener hanging up: the normal end.
        let Ok(frame) = read_frame(&mut ws_in, MAX_MESSAGE) else { break Ok(()) };
        if !frame.masked {
            break Err("a client frame wasn't masked".into());
        }
        match frame.op {
            OP_BINARY | OP_CONT | OP_TEXT => {
                if to.write_all(&frame.payload).is_err() {
                    break Ok(());
                }
            }
            OP_PING => {
                let _ = write_frame(&mut *lock(&ws_out), OP_PONG, &frame.payload, None);
            }
            OP_PONG => {}
            _ => break Ok(()),
        }
    };
    let _ = target.shutdown(Shutdown::Both);
    let _ = ws.shutdown(Shutdown::Both);
    let _ = back.join();
    result
}

// ---- WebSocket frames (RFC 6455) ----------------------------------------------------------------

pub const OP_CONT: u8 = 0;
pub const OP_TEXT: u8 = 1;
pub const OP_BINARY: u8 = 2;
pub const OP_CLOSE: u8 = 8;
pub const OP_PING: u8 = 9;
pub const OP_PONG: u8 = 10;

#[derive(Debug, PartialEq)]
pub struct Frame {
    pub op: u8,
    pub masked: bool,
    pub payload: Vec<u8>,
}

/// Writes one final frame; `mask` is for a client's frames (the server's go unmasked).
pub fn write_frame(w: &mut impl Write, op: u8, payload: &[u8], mask: Option<[u8; 4]>) -> Result<()> {
    let mut out = Vec::with_capacity(payload.len() + 14);
    out.push(0x80 | op);
    let m = if mask.is_some() { 0x80 } else { 0 };
    match payload.len() {
        n if n < 126 => out.push(m | n as u8),
        n if n <= 0xffff => {
            out.push(m | 126);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            out.push(m | 127);
            out.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    match mask {
        Some(key) => {
            out.extend_from_slice(&key);
            out.extend(payload.iter().enumerate().map(|(i, b)| b ^ key[i % 4]));
        }
        None => out.extend_from_slice(payload),
    }
    w.write_all(&out).map_err(|e| e.to_string())?;
    w.flush().map_err(|e| e.to_string())
}

/// Reads one frame (unmasked for the caller); payloads over `max` are refused before reading.
pub fn read_frame(r: &mut impl Read, max: u64) -> Result<Frame> {
    let mut head = [0u8; 2];
    r.read_exact(&mut head).map_err(|e| e.to_string())?;
    let op = head[0] & 0x0f;
    let masked = head[1] & 0x80 != 0;
    let len = match head[1] & 0x7f {
        126 => {
            let mut b = [0u8; 2];
            r.read_exact(&mut b).map_err(|e| e.to_string())?;
            u16::from_be_bytes(b) as u64
        }
        127 => {
            let mut b = [0u8; 8];
            r.read_exact(&mut b).map_err(|e| e.to_string())?;
            u64::from_be_bytes(b)
        }
        n => n as u64,
    };
    if len > max {
        return Err(format!("a WebSocket message of {len} bytes is too big"));
    }
    let mut key = [0u8; 4];
    if masked {
        r.read_exact(&mut key).map_err(|e| e.to_string())?;
    }
    let mut payload = vec![0u8; len as usize];
    r.read_exact(&mut payload).map_err(|e| e.to_string())?;
    if masked {
        for (i, b) in payload.iter_mut().enumerate() {
            *b ^= key[i % 4];
        }
    }
    Ok(Frame { op, masked, payload })
}

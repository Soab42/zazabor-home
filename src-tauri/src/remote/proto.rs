//! The remote channel's wire format (`ZREM/1`, docs/plans/remote.md): the dial line, the sealed
//! frames, and the compact binary input events.
//!
//! After `remote.start` over ZLNK, the phone dials the one-shot port, writes `ZREM/1` and
//! `{"phoneId","token","nonce"}`, and the PC answers `{"ok":true,"nonce"}`. Both sides then
//! derive two keys from the link key (`zrem session`) and talk in sealed frames: a 4-byte length,
//! then ChaCha20-Poly1305 over `kind ‖ body`. The phone's first sealed frame is `{"t":"ready"}`,
//! which proves it holds the key.

use std::io::{BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::phonelink::link::{base64, derive, net_err, read_line, unbase64, Cipher};

pub const MAGIC: &str = "ZREM/1";
/// A sealed frame: a keyframe at 1080p fits well inside this.
pub const MAX_FRAME: usize = 4 << 20;
const TAG_LEN: usize = 16;

/// Frame kinds (the first plaintext byte).
pub const K_JSON: u8 = 1;
pub const K_VIDEO: u8 = 0x10;
pub const K_INPUT: u8 = 0x20;

/// Video frame flags.
pub const F_KEY: u8 = 1;

type Result<T> = std::result::Result<T, String>;

/// Equal bytes, compared in time that doesn't depend on where they differ (the lengths aren't secret).
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// This session's keys: (phone→PC, PC→phone).
pub fn session_keys(link_key: &[u8], phone_nonce: &[u8], pc_nonce: &[u8]) -> ([u8; 32], [u8; 32]) {
    let mut out = [0u8; 64];
    derive(link_key, phone_nonce, pc_nonce, b"zrem session", &mut out);
    let (a, b) = out.split_at(32);
    (a.try_into().unwrap(), b.try_into().unwrap())
}

// ---- sealed frames ------------------------------------------------------------------------------

pub fn write_sealed(w: &mut impl Write, cipher: &mut Cipher, plain: &[u8]) -> Result<()> {
    let sealed = cipher.seal(plain);
    let mut out = Vec::with_capacity(4 + sealed.len());
    out.extend_from_slice(&(sealed.len() as u32).to_be_bytes());
    out.extend_from_slice(&sealed);
    w.write_all(&out).map_err(net_err)?;
    w.flush().map_err(net_err)
}

pub fn read_sealed(r: &mut impl Read, cipher: &mut Cipher) -> Result<Vec<u8>> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len).map_err(net_err)?;
    let len = u32::from_be_bytes(len) as usize;
    if !(TAG_LEN + 1..=MAX_FRAME + TAG_LEN).contains(&len) {
        return Err("A frame from the phone was the wrong size.".into());
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).map_err(net_err)?;
    cipher.open(body)
}

/// A JSON message's plaintext.
pub fn json_frame(v: &Value) -> Vec<u8> {
    let mut out = vec![K_JSON];
    out.extend_from_slice(&serde_json::to_vec(v).expect("JSON"));
    out
}

/// A video frame's plaintext: `10 ‖ flags ‖ pts µs (8, BE) ‖ Annex-B access unit`.
pub fn video_frame(key: bool, pts_us: u64, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(10 + data.len());
    out.push(K_VIDEO);
    out.push(if key { F_KEY } else { 0 });
    out.extend_from_slice(&pts_us.to_be_bytes());
    out.extend_from_slice(data);
    out
}

// ---- input events -------------------------------------------------------------------------------

/// What the phone asks the PC's media players to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Media {
    PlayPause = 1,
    Next = 2,
    Prev = 3,
    VolUp = 4,
    VolDown = 5,
    Mute = 6,
    Stop = 7,
    SeekBack = 8,
    SeekFwd = 9,
}

impl Media {
    fn from_u8(n: u8) -> Option<Media> {
        Some(match n {
            1 => Media::PlayPause,
            2 => Media::Next,
            3 => Media::Prev,
            4 => Media::VolUp,
            5 => Media::VolDown,
            6 => Media::Mute,
            7 => Media::Stop,
            8 => Media::SeekBack,
            9 => Media::SeekFwd,
            _ => return None,
        })
    }
}

/// One input event from the phone. Positions are 0..65535 across the captured monitor.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    MoveAbs { x: u16, y: u16 },
    MoveRel { dx: i16, dy: i16 },
    /// Button 1 left, 2 middle, 3 right.
    Button { button: u8, down: bool },
    /// Eighths of a wheel notch (so a slow two-finger scroll is smooth).
    Wheel { dx: i16, dy: i16 },
    /// An X11 keysym.
    Key { keysym: u32, down: bool },
    Text(String),
    Media(Media),
}

const E_MOVE_ABS: u8 = 1;
const E_MOVE_REL: u8 = 2;
const E_BUTTON: u8 = 3;
const E_WHEEL: u8 = 4;
const E_KEY: u8 = 5;
const E_TEXT: u8 = 6;
const E_MEDIA: u8 = 7;

/// The longest text one event may carry.
pub const MAX_TEXT: usize = 4096;

impl Input {
    /// The frame's plaintext: `20 ‖ event kind ‖ fields (big-endian)`. The phone writes these; the
    /// PC only reads them, but its tests pin the bytes.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![K_INPUT];
        match self {
            Input::MoveAbs { x, y } => {
                out.push(E_MOVE_ABS);
                out.extend_from_slice(&x.to_be_bytes());
                out.extend_from_slice(&y.to_be_bytes());
            }
            Input::MoveRel { dx, dy } => {
                out.push(E_MOVE_REL);
                out.extend_from_slice(&dx.to_be_bytes());
                out.extend_from_slice(&dy.to_be_bytes());
            }
            Input::Button { button, down } => out.extend_from_slice(&[E_BUTTON, *button, *down as u8]),
            Input::Wheel { dx, dy } => {
                out.push(E_WHEEL);
                out.extend_from_slice(&dx.to_be_bytes());
                out.extend_from_slice(&dy.to_be_bytes());
            }
            Input::Key { keysym, down } => {
                out.push(E_KEY);
                out.extend_from_slice(&keysym.to_be_bytes());
                out.push(*down as u8);
            }
            Input::Text(text) => {
                let mut end = text.len().min(MAX_TEXT);
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                out.push(E_TEXT);
                out.extend_from_slice(&(end as u16).to_be_bytes());
                out.extend_from_slice(&text.as_bytes()[..end]);
            }
            Input::Media(m) => out.extend_from_slice(&[E_MEDIA, *m as u8]),
        }
        out
    }

    /// Reads the plaintext after the frame kind (`event kind ‖ fields`).
    pub fn decode(body: &[u8]) -> Result<Input> {
        let bad = || "The phone sent an input event this PC doesn't understand.".to_string();
        let (&kind, f) = body.split_first().ok_or_else(bad)?;
        let u16at = |i: usize| f.get(i..i + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
        let i16at = |i: usize| u16at(i).map(|v| v as i16);
        Ok(match kind {
            E_MOVE_ABS if f.len() == 4 => Input::MoveAbs { x: u16at(0).ok_or_else(bad)?, y: u16at(2).ok_or_else(bad)? },
            E_MOVE_REL if f.len() == 4 => Input::MoveRel { dx: i16at(0).ok_or_else(bad)?, dy: i16at(2).ok_or_else(bad)? },
            E_BUTTON if f.len() == 2 && (1..=3).contains(&f[0]) => Input::Button { button: f[0], down: f[1] != 0 },
            E_WHEEL if f.len() == 4 => Input::Wheel { dx: i16at(0).ok_or_else(bad)?, dy: i16at(2).ok_or_else(bad)? },
            E_KEY if f.len() == 5 => Input::Key { keysym: u32::from_be_bytes(f[..4].try_into().unwrap()), down: f[4] != 0 },
            E_TEXT if f.len() >= 2 => {
                let n = u16at(0).ok_or_else(bad)? as usize;
                if n > MAX_TEXT || f.len() != 2 + n {
                    return Err(bad());
                }
                Input::Text(String::from_utf8(f[2..].to_vec()).map_err(|_| bad())?)
            }
            E_MEDIA if f.len() == 1 => Input::Media(Media::from_u8(f[0]).ok_or_else(bad)?),
            _ => return Err(bad()),
        })
    }
}

// ---- the dial -----------------------------------------------------------------------------------

/// What the one-shot port expects from the one phone it was opened for.
pub struct Expect {
    pub phone_id: String,
    pub token: [u8; 16],
    pub link_key: [u8; 32],
    pub pc_nonce: [u8; 16],
}

/// An accepted dial: the stream and both ciphers (the phone's `ready` already read).
pub struct Dialed {
    pub stream: TcpStream,
    pub reader: BufReader<TcpStream>,
    pub rx: Cipher,
    pub tx: Cipher,
}

/// Waits up to `wait` for the phone to dial `listener` and checks it: the magic, the phone's id,
/// the single-use token (constant time), and a first sealed frame `ready` under the derived keys.
/// A connection that fails any of it is closed and the wait goes on (a stray connection must not
/// use up the one chance); the listener only serves one successful dial.
pub fn accept_dial(listener: &TcpListener, expect: &Expect, wait: Duration) -> Result<Dialed> {
    listener.set_nonblocking(true).map_err(net_err)?;
    let deadline = Instant::now() + wait;
    loop {
        match listener.accept() {
            Ok((stream, _)) => match dial(stream, expect) {
                Ok(d) => return Ok(d),
                Err(e) => eprintln!("Remote: refused a connection: {e}"),
            },
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err("The phone didn't connect in time.".into());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(net_err(e)),
        }
    }
}

fn dial(stream: TcpStream, expect: &Expect) -> Result<Dialed> {
    stream.set_nonblocking(false).map_err(net_err)?;
    stream.set_nodelay(true).ok();
    stream.set_read_timeout(Some(Duration::from_secs(10))).map_err(net_err)?;
    stream.set_write_timeout(Some(Duration::from_secs(10))).map_err(net_err)?;
    let mut reader = BufReader::with_capacity(64 << 10, stream.try_clone().map_err(net_err)?);
    let mut out = stream.try_clone().map_err(net_err)?;
    if read_line(&mut reader)? != MAGIC {
        return Err("not ZREM".into());
    }
    let hello: Value = serde_json::from_str(&read_line(&mut reader)?).map_err(|_| "the dial didn't make sense".to_string())?;
    let text = |k: &str| hello.get(k).and_then(Value::as_str).unwrap_or("");
    let token = unbase64(text("token")).unwrap_or_default();
    let phone_nonce = unbase64(text("nonce")).unwrap_or_default();
    // Both checks always run, so a wrong id and a wrong token take the same time.
    let id_ok = ct_eq(text("phoneId").to_ascii_lowercase().as_bytes(), expect.phone_id.as_bytes());
    let token_ok = ct_eq(&token, &expect.token);
    if !(id_ok && token_ok) || phone_nonce.len() != 16 {
        let _ = out.write_all(b"{\"ok\":false}\n");
        let _ = stream.shutdown(Shutdown::Both);
        return Err("wrong phone or token".into());
    }
    let mut line = serde_json::to_vec(&json!({"ok": true, "nonce": base64(&expect.pc_nonce)})).expect("JSON");
    line.push(b'\n');
    out.write_all(&line).map_err(net_err)?;
    let (up, down) = session_keys(&expect.link_key, &phone_nonce, &expect.pc_nonce);
    let (mut rx, tx) = (Cipher::new(&up), Cipher::new(&down));
    let first = read_sealed(&mut reader, &mut rx)?;
    let ready: Value = first.split_first().filter(|(k, _)| **k == K_JSON).and_then(|(_, b)| serde_json::from_slice(b).ok()).unwrap_or_default();
    if ready.get("t").and_then(Value::as_str) != Some("ready") {
        return Err("the phone didn't start with ready".into());
    }
    Ok(Dialed { stream, reader, rx, tx })
}

//! Decoded frames: libVLC's "vmem" callbacks write planar YUV 4:2:0 into a back buffer, and the
//! `cs://localhost/frame?after=<n>` route hands the newest picture to the webview's WebGL.

use std::ffi::{c_char, c_uint, c_void};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;
use tauri::http::{Response, StatusCode};

/// Frame numbers keep counting across players, so a waiting reader never stalls on a reset.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);
/// The frames of the open player; the route reads this without touching the player lock.
static CURRENT: Mutex<Option<Arc<Frames>>> = Mutex::new(None);

/// `ZAZABOR_VIDEO_STATS=1` logs the decoder's frame pacing and what the page reports (see
/// `serve_frame`) to stderr every two seconds.
pub fn stats_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("ZAZABOR_VIDEO_STATS").is_some_and(|v| v == "1"))
}

/// Intervals between `display` callbacks (the decoder's presentation clock), in microseconds.
static DISPLAY_TIMES: Mutex<(Option<std::time::Instant>, Vec<u32>)> = Mutex::new((None, Vec::new()));

fn note_display() {
    let now = std::time::Instant::now();
    let mut times = lock_ignore_poison(&DISPLAY_TIMES);
    if let Some(last) = times.0 {
        let gap = now.duration_since(last).as_micros() as u32;
        times.1.push(gap);
    }
    times.0 = Some(now);
    if times.1.iter().map(|&g| g as u64).sum::<u64>() >= 2_000_000 {
        let mut gaps = std::mem::take(&mut times.1);
        gaps.sort_unstable();
        let p = |q: f64| gaps[((gaps.len() as f64 * q) as usize).min(gaps.len() - 1)] as f64 / 1000.0;
        let total: u64 = gaps.iter().map(|&g| g as u64).sum();
        eprintln!(
            "VSTAT decoder {:.1} fps, gap p50 {:.1} p95 {:.1} max {:.1} ms",
            gaps.len() as f64 * 1e6 / total as f64,
            p(0.5),
            p(0.95),
            p(1.0)
        );
    }
}

/// How long a request waits for a new picture before answering "nothing new".
const LONG_POLL: Duration = Duration::from_millis(250);
/// The chroma code in the frame header (1 = planar I420).
const CHROMA_I420: u32 = 1;

/// Locks a mutex even if a panicking thread poisoned it (libVLC callbacks must never unwind).
pub fn lock_ignore_poison<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// The size to decode at: fits `max` keeping the aspect ratio, never upscales, even sides.
pub fn fit_size(src_w: u32, src_h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    let (sw, sh) = (src_w.max(2) as f64, src_h.max(2) as f64);
    let scale = (max_w as f64 / sw).min(max_h as f64 / sh).min(1.0);
    let even = |v: f64| (((v * scale) / 2.0).round() as u32 * 2).max(2);
    (even(sw), even(sh))
}

/// Luma row pitch for a visible width: a multiple of 32 (chroma gets half), so the visible
/// width, and with it the aspect ratio, stays exact while libVLC gets friendly alignment.
pub fn luma_pitch(width: u32) -> u32 {
    (width + 31) & !31
}

/// Byte offsets of the U and V planes and the total size for padded I420 planes.
pub fn plane_layout(pitch: u32, height: u32) -> (usize, usize, usize) {
    let y = pitch as usize * height as usize;
    let c = (pitch as usize / 2) * (height as usize / 2);
    (y, y + c, y + 2 * c)
}

/// The 16-byte header: width, height, chroma code and frame counter, all u32 little-endian.
pub fn header(width: u32, height: u32, counter: u64) -> [u8; 16] {
    let mut out = [0u8; 16];
    for (i, value) in [width, height, CHROMA_I420, counter as u32].into_iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&value.to_le_bytes());
    }
    out
}

struct FrameState {
    width: u32,
    height: u32,
    pitch: u32,
    back: Vec<u8>,
    front: Vec<u8>,
    counter: u64,
    /// When `front` was handed to us for display (libVLC calls `display` at presentation time).
    shown_ms: f64,
}

pub struct Frames {
    /// The largest size frames are decoded at; libVLC scales down to it.
    max: (u32, u32),
    state: Mutex<FrameState>,
    ready: Condvar,
}

impl Frames {
    pub fn new(max_width: u32, max_height: u32) -> Self {
        Frames {
            max: (max_width, max_height),
            state: Mutex::new(FrameState {
                width: 0,
                height: 0,
                pitch: 0,
                back: Vec::new(),
                front: Vec::new(),
                counter: SEQUENCE.load(Ordering::Relaxed),
                shown_ms: 0.0,
            }),
            ready: Condvar::new(),
        }
    }

    /// The decoded picture size, (0, 0) before the first frame.
    pub fn size(&self) -> (u32, u32) {
        let state = lock_ignore_poison(&self.state);
        (state.width, state.height)
    }
}

/// Makes `frames` what the frame route serves (None: nothing is playing).
pub fn set_current(frames: Option<Arc<Frames>>) {
    *lock_ignore_poison(&CURRENT) = frames;
}

fn frames_of<'a>(opaque: *mut c_void) -> &'a Frames {
    // SAFETY: opaque is the Arc<Frames> pointer the player keeps alive until libVLC has stopped.
    unsafe { &*(opaque as *const Frames) }
}

/// libVLC tells us the source size; we answer with the size and layout we want (I420).
pub unsafe extern "C" fn setup(
    opaque: *mut *mut c_void,
    chroma: *mut c_char,
    width: *mut c_uint,
    height: *mut c_uint,
    pitches: *mut c_uint,
    lines: *mut c_uint,
) -> c_uint {
    let frames = frames_of(*opaque);
    let (w, h) = fit_size(*width, *height, frames.max.0, frames.max.1);
    let pitch = luma_pitch(w);
    *width = w;
    *height = h;
    std::ptr::copy_nonoverlapping(b"I420".as_ptr(), chroma as *mut u8, 4);
    *pitches = pitch;
    *pitches.add(1) = pitch / 2;
    *pitches.add(2) = pitch / 2;
    *lines = h;
    *lines.add(1) = h / 2;
    *lines.add(2) = h / 2;
    let size = plane_layout(pitch, h).2;
    let mut state = lock_ignore_poison(&frames.state);
    state.width = w;
    state.height = h;
    state.pitch = pitch;
    state.back = vec![0; size];
    state.front = vec![0; size];
    1
}

pub unsafe extern "C" fn cleanup(_opaque: *mut c_void) {}

pub unsafe extern "C" fn lock(opaque: *mut c_void, planes: *mut *mut c_void) -> *mut c_void {
    let frames = frames_of(opaque);
    let mut state = lock_ignore_poison(&frames.state);
    let (u, v, _) = plane_layout(state.pitch, state.height);
    let base = state.back.as_mut_ptr();
    *planes = base as *mut c_void;
    *planes.add(1) = base.add(u) as *mut c_void;
    *planes.add(2) = base.add(v) as *mut c_void;
    std::ptr::null_mut()
}

pub unsafe extern "C" fn unlock(_opaque: *mut c_void, _picture: *mut c_void, _planes: *const *mut c_void) {}

pub unsafe extern "C" fn display(opaque: *mut c_void, _picture: *mut c_void) {
    let frames = frames_of(opaque);
    {
        let mut guard = lock_ignore_poison(&frames.state);
        let state = &mut *guard;
        std::mem::swap(&mut state.back, &mut state.front);
        state.counter = SEQUENCE.fetch_add(1, Ordering::Relaxed) + 1;
        state.shown_ms = now_ms();
        frames.ready.notify_all();
    }
    if stats_enabled() {
        note_display();
    }
}

/// Milliseconds on this process's monotonic clock (the page paces frames against it).
fn now_ms() -> f64 {
    static EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    EPOCH.get_or_init(std::time::Instant::now).elapsed().as_secs_f64() * 1000.0
}

/// Header plus the planes repacked tightly for the visible size (drops the pitch padding). The
/// socket stream's header is 8 bytes longer: the presentation time as an f64 in milliseconds.
fn pack_into(state: &FrameState, body: &mut Vec<u8>, timed: bool) {
    let (w, h, pitch) = (state.width as usize, state.height as usize, state.pitch as usize);
    let (u, v, _) = plane_layout(state.pitch, state.height);
    body.clear();
    body.reserve(24 + w * h * 3 / 2);
    body.extend_from_slice(&header(state.width, state.height, state.counter));
    if timed {
        body.extend_from_slice(&state.shown_ms.to_le_bytes());
    }
    for (offset, stride, width, rows) in [(0, pitch, w, h), (u, pitch / 2, w / 2, h / 2), (v, pitch / 2, w / 2, h / 2)] {
        if stride == width {
            body.extend_from_slice(&state.front[offset..offset + width * rows]);
        } else {
            for row in 0..rows {
                let start = offset + row * stride;
                body.extend_from_slice(&state.front[start..start + width]);
            }
        }
    }
}

fn pack(state: &FrameState) -> Vec<u8> {
    let mut body = Vec::new();
    pack_into(state, &mut body, false);
    body
}

fn empty(status: StatusCode) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header("Access-Control-Allow-Origin", "*")
        .header("Cache-Control", "no-store")
        .body(Vec::new())
        .expect("static response")
}

/// A small JSON answer.
pub fn json_response(body: String) -> Response<Vec<u8>> {
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/json")
        .header("Access-Control-Allow-Origin", "*")
        .header("Cache-Control", "no-store")
        .body(body.into_bytes())
        .expect("json response")
}

/// The newest frame after `after`, waiting briefly for one; 204 when there is nothing new.
pub fn response(after: u64) -> Response<Vec<u8>> {
    let Some(frames) = lock_ignore_poison(&CURRENT).clone() else {
        // Nothing is playing; don't spin the caller.
        std::thread::sleep(LONG_POLL);
        return empty(StatusCode::NO_CONTENT);
    };
    let state = lock_ignore_poison(&frames.state);
    let (state, _) = frames
        .ready
        .wait_timeout_while(state, LONG_POLL, |s| s.counter <= after)
        .unwrap_or_else(|e| e.into_inner());
    if state.counter <= after || state.width == 0 {
        return empty(StatusCode::NO_CONTENT);
    }
    let body = pack(&state);
    drop(state);
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/octet-stream")
        .header("Access-Control-Allow-Origin", "*")
        .header("Cache-Control", "no-store")
        .body(body)
        .expect("frame response")
}

/// The page connects here (see `socket_info`): one long-lived WebSocket per window carries every
/// frame as it is shown, instead of a request per frame.
static SOCKET: std::sync::OnceLock<Option<(u16, String)>> = std::sync::OnceLock::new();

/// A random token the page must present, so no other local program or web page reads the picture.
fn make_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    (0..2)
        .map(|_| format!("{:016x}", std::collections::hash_map::RandomState::new().build_hasher().finish()))
        .collect()
}

/// Starts the frame socket on first use; its port and token, None when it could not listen.
pub fn socket_info() -> Option<(u16, String)> {
    SOCKET
        .get_or_init(|| {
            let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).ok()?;
            let port = listener.local_addr().ok()?.port();
            let token = make_token();
            let wanted = token.clone();
            std::thread::Builder::new()
                .name("vframe-accept".into())
                .spawn(move || {
                    for stream in listener.incoming().flatten() {
                        let token = wanted.clone();
                        let _ = std::thread::Builder::new().name("vframe-socket".into()).spawn(move || serve_socket(stream, &token));
                    }
                })
                .ok()?;
            Some((port, token))
        })
        .clone()
}

/// Whether a page at this Origin belongs to the app (its own scheme, or the dev server).
fn origin_ok(origin: Option<&str>) -> bool {
    let Some(origin) = origin else { return true };
    let host = origin.split("://").nth(1).unwrap_or("");
    let host = host.split('/').next().unwrap_or("");
    let name = host.rsplit_once(':').filter(|(_, port)| port.chars().all(|c| c.is_ascii_digit())).map_or(host, |(name, _)| name);
    origin.starts_with("tauri://") || name == "localhost" || name == "127.0.0.1" || name.ends_with(".localhost")
}

/// Pushes the newest frame to one connected page until it goes away.
fn serve_socket(stream: std::net::TcpStream, token: &str) {
    use tungstenite::handshake::server::{ErrorResponse, Request, Response as Reply};
    use tungstenite::Message;
    let _ = stream.set_nodelay(true);
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let check = |request: &Request, reply: Reply| -> Result<Reply, ErrorResponse> {
        let origin = request.headers().get("origin").and_then(|v| v.to_str().ok());
        let query = request.uri().query().unwrap_or("");
        let presented = url::form_urlencoded::parse(query.as_bytes()).find(|(k, _)| k == "t").map(|(_, v)| v.into_owned());
        if origin_ok(origin) && presented.as_deref() == Some(token) {
            Ok(reply)
        } else {
            let mut refusal = ErrorResponse::new(None);
            *refusal.status_mut() = tungstenite::http::StatusCode::FORBIDDEN;
            Err(refusal)
        }
    };
    let Ok(mut ws) = tungstenite::accept_hdr(stream, check) else { return };
    let (mut after, mut buffer) = (0u64, Vec::new());
    let mut quiet = 0;
    loop {
        if crate::lock::is_locked() {
            return;
        }
        let current = lock_ignore_poison(&CURRENT).clone();
        let Some(frames) = current else {
            std::thread::sleep(Duration::from_millis(50));
            quiet += 1;
            if quiet % 40 == 0 && ws.send(Message::Ping(Vec::new().into())).is_err() {
                return;
            }
            continue;
        };
        let state = lock_ignore_poison(&frames.state);
        let (state, _) = frames.ready.wait_timeout_while(state, LONG_POLL, |s| s.counter <= after).unwrap_or_else(|e| e.into_inner());
        if state.counter <= after || state.width == 0 {
            drop(state);
            // A paused picture sends nothing; a ping finds out whether the page is still there.
            quiet += 1;
            if quiet % 8 == 0 && ws.send(Message::Ping(Vec::new().into())).is_err() {
                return;
            }
            continue;
        }
        pack_into(&state, &mut buffer, true);
        after = state.counter;
        drop(state);
        quiet = 0;
        if ws.send(Message::Binary(std::mem::take(&mut buffer).into())).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_keeps_aspect_and_never_upscales() {
        assert_eq!(fit_size(1920, 1080, 960, 540), (960, 540));
        assert_eq!(fit_size(1920, 1080, 4000, 4000), (1920, 1080));
        // Limited by height: 1000 wide stage, 300 tall.
        assert_eq!(fit_size(1920, 1080, 1000, 300), (534, 300));
        // Portrait.
        assert_eq!(fit_size(1080, 1920, 500, 500), (282, 500));
        // Sides are always even and at least 2.
        let (w, h) = fit_size(853, 479, 400, 400);
        assert!(w % 2 == 0 && h % 2 == 0);
        assert_eq!(fit_size(1, 1, 1, 1), (2, 2));
    }

    #[test]
    fn pitch_is_aligned_and_not_smaller_than_width() {
        assert_eq!(luma_pitch(534), 544);
        assert_eq!(luma_pitch(960), 960);
        assert_eq!(luma_pitch(2), 32);
        assert_eq!(plane_layout(64, 4), (256, 256 + 64, 256 + 128));
    }

    #[test]
    fn header_is_four_little_endian_words() {
        let h = header(0x0102, 3, 0x1_0000_0005);
        assert_eq!(&h[0..4], &[2, 1, 0, 0]);
        assert_eq!(&h[4..8], &[3, 0, 0, 0]);
        assert_eq!(&h[8..12], &[1, 0, 0, 0]);
        // The counter wraps to u32.
        assert_eq!(&h[12..16], &[5, 0, 0, 0]);
    }

    #[test]
    fn pack_drops_pitch_padding() {
        let (w, h, pitch) = (4u32, 2u32, 32u32);
        let (u, v, total) = plane_layout(pitch, h);
        let mut front = vec![0u8; total];
        for row in 0..2usize {
            for x in 0..4usize {
                front[row * 32 + x] = 10 + (row * 4 + x) as u8;
            }
        }
        front[u..u + 2].copy_from_slice(&[50, 51]);
        front[v..v + 2].copy_from_slice(&[60, 61]);
        let state = FrameState { width: w, height: h, pitch, back: Vec::new(), front, counter: 7, shown_ms: 0.0 };
        let body = pack(&state);
        assert_eq!(body.len(), 16 + 12);
        assert_eq!(&body[16..], &[10, 11, 12, 13, 14, 15, 16, 17, 50, 51, 60, 61]);
    }
}

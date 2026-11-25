//! Remote control (docs/plans/remote.md): a paired phone sees and controls this PC (Screen,
//! Touchpad, Keyboard, Media), and this PC controls the phone's screen through the Link app's
//! accessibility service (the mirror window's toolbar).
//!
//! Phone → PC: the phone asks `remote.start` over the sealed ZLNK link. This PC checks that
//! remote control is on, asks its user (unless that phone is always allowed), opens a one-shot
//! port with a single-use token and answers with where to dial. The phone dials, both sides
//! derive session keys from the link key (`proto.rs`), and from then on the screen goes out as
//! sealed H.264 frames (`capture.rs`) while input events come back (`inject.rs`, `media.rs`).
//! A bright pill (`ui.rs`) shows on every monitor while it runs, and Ctrl+Alt+Esc
//! (`killswitch.rs`) ends it from the PC's own keyboard.
//!
//! The phone can also switch the picture to the PC's camera and ask for the PC's sound or its
//! microphone (`audio.rs`); the camera and the microphone need their own setting, and the pill
//! shows them while they're on. Away from home all of it runs through `anywhere.rs`.

pub mod anywhere;
mod audio;
mod capture;
mod inject;
mod killswitch;
mod media;
pub mod proto;
pub mod store;
pub mod ui;
#[cfg(test)]
mod tests;

use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use ring::rand::{SecureRandom, SystemRandom};
use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use crate::phonelink::link::{base64, Cipher, Link};
use proto::{Dialed, Expect, Input};

type Result<T> = std::result::Result<T, String>;

/// How long the phone has to dial after the answer.
const DIAL_WAIT: Duration = Duration::from_secs(30);
/// A session whose phone says nothing for this long is over.
const DEAD_AFTER: Duration = Duration::from_secs(20);
/// A frame that takes this long to send means the phone isn't keeping up.
const SLOW_SEND: Duration = Duration::from_millis(120);

static APP: std::sync::OnceLock<AppHandle> = std::sync::OnceLock::new();
static ACTIVE: Mutex<Option<Arc<Session>>> = Mutex::new(None);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn init(app: AppHandle, data_dir: std::path::PathBuf) {
    store::init(data_dir.join("remote.json"));
    ui::init(app.clone());
    let _ = APP.set(app);
    // At login the keyring (Meet's tunnel token) may not be unlocked yet: keep trying for ten
    // minutes, quietly, while the internet path is switched on.
    let _ = std::thread::Builder::new().name("remote-anywhere-start".into()).spawn(|| {
        for _ in 0..30 {
            match anywhere::apply() {
                Ok(()) => return,
                Err(e) => eprintln!("Remote (internet): {e}"),
            }
            std::thread::sleep(Duration::from_secs(20));
            if !store::get().internet {
                return;
            }
        }
    });
}

/// Settings' page refreshes (the internet path's tunnel changed state).
pub(crate) fn emit_changed() {
    if let Some(app) = APP.get() {
        let _ = app.emit("remote-changed", json!({}));
    }
}

/// A session, from the question to the last frame.
pub struct Session {
    id: u64,
    phone_id: String,
    phone_name: String,
    /// The phone came through the internet path.
    internet: bool,
    stop: AtomicBool,
    stream: Mutex<Option<std::net::TcpStream>>,
}

impl Session {
    /// Ends it: the next read or write fails, and every thread of it winds down.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(s) = lock(&self.stream).as_ref() {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }
}

/// One monitor, as the phone's picker lists it.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct MonitorInfo {
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub primary: bool,
}

fn display_name() -> String {
    std::env::var("DISPLAY").unwrap_or_else(|_| ":0".into())
}

fn monitors(display: &str) -> Vec<MonitorInfo> {
    let mut list: Vec<MonitorInfo> =
        crate::recorder::x11::monitors(display).into_iter().map(|(name, x, y, w, h, primary)| MonitorInfo { name, x, y, w, h, primary }).filter(|m| m.w > 0 && m.h > 0).collect();
    if list.is_empty() {
        if let Some((w, h)) = crate::recorder::x11::screen_size(display) {
            list.push(MonitorInfo { name: "Screen".into(), x: 0, y: 0, w, h, primary: true });
        }
    }
    list
}

/// Which monitor a request means: the one asked for, else the primary, else the first.
pub fn pick_monitor(list: &[MonitorInfo], wanted: Option<usize>) -> Option<usize> {
    wanted.filter(|i| *i < list.len()).or_else(|| list.iter().position(|m| m.primary)).or(if list.is_empty() { None } else { Some(0) })
}

fn random<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    SystemRandom::new().fill(&mut out).expect("the system's random source");
    out
}

fn reply(link: &Link, rid: &Value, result: Result<Value>) {
    let mut re = json!({"t": "re", "rid": rid});
    match result {
        Ok(fields) => {
            re["ok"] = json!(true);
            if let (Some(to), Some(from)) = (re.as_object_mut(), fields.as_object()) {
                to.extend(from.clone());
            }
        }
        Err(e) => {
            re["ok"] = json!(false);
            re["error"] = json!(e);
        }
    }
    let _ = link.send(re);
}

/// The phone's `remote.start` / `remote.stop`, off the link's reader thread (the question waits
/// for the user).
pub fn on_message(link: Arc<Link>, msg: Value) {
    let t = msg.get("t").and_then(Value::as_str).unwrap_or("").to_string();
    let rid = msg.get("rid").cloned().unwrap_or(Value::Null);
    let _ = std::thread::Builder::new().name("remote-request".into()).spawn(move || {
        let result = match t.as_str() {
            "remote.start" => start(&link, &msg),
            _ => {
                if let Some(s) = lock(&ACTIVE).clone().filter(|s| s.phone_id == link.phone_id) {
                    s.stop();
                }
                Ok(json!({}))
            }
        };
        if !rid.is_null() {
            reply(&link, &rid, result);
        } else if let Err(e) = result {
            eprintln!("Remote: {e}");
        }
    });
}

/// Claims the one session slot.
fn claim(phone_id: &str, phone_name: &str, internet: bool) -> Result<Arc<Session>> {
    let mut active = lock(&ACTIVE);
    if let Some(old) = active.as_ref() {
        // The same phone asking again has left its old session (the app was closed or the network
        // changed): this one takes over. Another phone has to wait.
        if old.phone_id != phone_id {
            return Err("Another remote session is running on this PC.".into());
        }
        eprintln!("Remote: {phone_name} asked again; its old session ends");
        old.stop();
    }
    let s = Arc::new(Session { id: NEXT_ID.fetch_add(1, Ordering::SeqCst), phone_id: phone_id.into(), phone_name: phone_name.into(), internet, stop: AtomicBool::new(false), stream: Mutex::new(None) });
    *active = Some(s.clone());
    Ok(s)
}

/// Frees the slot if this session still holds it (false when another one took over).
fn release(s: &Session) -> bool {
    let mut active = lock(&ACTIVE);
    if active.as_ref().is_some_and(|a| a.id == s.id) {
        *active = None;
        return true;
    }
    false
}

/// Waits up to `wait` for a session's helper thread; one that hangs (a capture that won't stop)
/// is left behind rather than holding the session.
fn join_for(handle: std::thread::JoinHandle<()>, what: &str, wait: Duration) {
    let end = Instant::now() + wait;
    while !handle.is_finished() {
        if Instant::now() >= end {
            eprintln!("Remote: the {what} thread didn't end; leaving it");
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = handle.join();
}

fn start(link: &Arc<Link>, msg: &Value) -> Result<Value> {
    let hub = crate::phonelink::hub().ok_or("Phone Link is off.")?;
    let settings = store::get();
    if !settings.enabled {
        return Err("Remote control is off on this PC. Turn it on in Settings → Phone Link → Remote control.".into());
    }
    let phone_id = link.phone_id.clone();
    let paired = hub.with_db(|c| crate::phonelink::store::phone(c, &phone_id))?.ok_or("This PC doesn't know this phone.")?;
    let name = paired.name.clone();
    let session = claim(&phone_id, &name, link.via_internet)?;
    match prepare(hub, &session, msg, &settings, &phone_id) {
        Ok(answer) => Ok(answer),
        Err(e) => {
            if release(&session) {
                ui::hide();
            }
            Err(e)
        }
    }
}

fn prepare(hub: &Arc<crate::phonelink::Hub>, session: &Arc<Session>, msg: &Value, settings: &store::Settings, phone_id: &str) -> Result<Value> {
    let num = |k: &str| msg.get(k).and_then(Value::as_u64);
    let cameras = cameras();
    let wants_camera = msg.get("source").and_then(Value::as_str) == Some("camera");
    if wants_camera && !settings.media {
        return Err(MEDIA_OFF.into());
    }
    if wants_camera && cameras.is_empty() {
        return Err("This PC has no camera.".into());
    }
    if !settings.always.iter().any(|p| p == phone_id) {
        match ui::ask(&session.phone_name, if wants_camera { "camera" } else { "control" }) {
            ui::Choice::Deny => return Err("Remote control was declined on the PC.".into()),
            ui::Choice::Always => store::allow_always(phone_id),
            ui::Choice::Once => {}
        }
    }
    let want_video = msg.get("video").and_then(Value::as_bool).unwrap_or(true);
    let view_only = settings.view_only || msg.get("viewOnly").and_then(Value::as_bool).unwrap_or(false);
    let display = display_name();
    let list = monitors(&display);
    let idx = pick_monitor(&list, num("monitor").map(|n| n as usize)).ok_or("This PC has no screen to show.")?;
    let max = (num("maxW").unwrap_or(1920).clamp(320, 3840) as u32, num("maxH").unwrap_or(1080).clamp(240, 2160) as u32);
    let fps = num("fps").unwrap_or(30).clamp(5, 60) as u32;
    let kbps = num("kbps").unwrap_or(4000).clamp(300, 30_000) as u32;

    let link_key = hub.with_db(|c| crate::phonelink::store::link_key(c, phone_id))?.ok_or("This PC doesn't know this phone.")?;
    // A phone on the internet comes through the bridge on this PC, so its port stays off the network.
    let listener = TcpListener::bind((if session.internet { "127.0.0.1" } else { "0.0.0.0" }, 0)).map_err(|e| format!("Couldn't open a port for the screen: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let wss = if session.internet {
        anywhere::allow_remote_port(port, Instant::now() + DIAL_WAIT);
        Some(anywhere::remote_url(port).ok_or("The internet path was just turned off.")?)
    } else {
        None
    };
    let expect = Expect { phone_id: phone_id.to_string(), token: random::<16>(), link_key, pc_nonce: random::<16>() };
    let token = base64(&expect.token);
    let camera_names: Vec<Value> = cameras.iter().map(|c| json!({"name": c.name})).collect();
    let camera = wants_camera.then(|| num("camera").map(|n| n as usize).filter(|i| *i < cameras.len()).unwrap_or(0));
    let target = Target { monitors: list.clone(), index: idx, max, version: 0, cameras, camera };
    let size = target.size();
    let params = Params { display, fps, kbps, encoder: capture::Encoder::parse(&settings.encoder), video: want_video, media: settings.media };
    let source = target.source_name();
    let s = session.clone();
    std::thread::Builder::new().name("remote-session".into()).spawn(move || run(s, listener, expect, target, params, view_only)).map_err(|e| e.to_string())?;
    let mut answer = json!({"port": port, "token": token, "monitors": list, "monitor": idx, "w": size.0, "h": size.1, "fps": fps, "viewOnly": view_only, "video": want_video, "encoder": settings.encoder, "source": source, "cameras": camera_names, "media": settings.media, "internet": session.internet});
    if let Some(url) = wss {
        answer["wss"] = json!(url);
    }
    Ok(answer)
}

const MEDIA_OFF: &str = "The camera and microphone are off for phones on this PC. Turn them on in Settings → Phone Link → Remote control.";

/// The cameras a phone may pick (and a test picture when `ZREM_TEST_CAMERA` is set).
fn cameras() -> Vec<crate::recorder::devices::CameraDevice> {
    // The device monitor needs GStreamer started (the recorder may not have run yet).
    let mut list = if gstreamer::init().is_ok() { crate::recorder::devices::list(false, true).cameras } else { Vec::new() };
    if std::env::var_os("ZREM_TEST_CAMERA").is_some() {
        list.push(crate::recorder::devices::CameraDevice { path: "test".into(), name: "Test picture".into(), mode: None });
    }
    list
}

/// What the capture and the injector aim at; the version changes when the monitor does.
struct Target {
    monitors: Vec<MonitorInfo>,
    index: usize,
    max: (u32, u32),
    version: u64,
    cameras: Vec<crate::recorder::devices::CameraDevice>,
    /// Which camera the picture shows; None is the monitor.
    camera: Option<usize>,
}

impl Target {
    fn monitor(&self) -> &MonitorInfo {
        &self.monitors[self.index]
    }

    fn source(&self, display: &str) -> capture::Source {
        match self.camera.and_then(|i| self.cameras.get(i)) {
            Some(c) => capture::Source::Camera { path: c.path.clone(), mode: c.mode },
            None => {
                let m = self.monitor();
                capture::Source::Screen { display: display.to_string(), rect: (m.x, m.y, m.w, m.h) }
            }
        }
    }

    /// The picture's size under the phone's limit.
    fn size(&self) -> (u32, u32) {
        crate::recorder::pipeline::output_size(capture::native_size(&self.source("")), self.max)
    }

    fn source_name(&self) -> &'static str {
        if self.camera.is_some() { "camera" } else { "screen" }
    }
}

struct Params {
    display: String,
    fps: u32,
    kbps: u32,
    encoder: capture::Encoder,
    video: bool,
    /// The camera and microphone may be used.
    media: bool,
}

/// The sealed writer both the video thread and the replies share.
struct Out {
    stream: std::net::TcpStream,
    cipher: Cipher,
}

fn send(out: &Mutex<Out>, plain: &[u8]) -> Result<()> {
    let mut o = lock(out);
    let Out { stream, cipher } = &mut *o;
    proto::write_sealed(stream, cipher, plain)
}

fn info(target: &Target, params: &Params, view_only: bool) -> Value {
    let (w, h) = target.size();
    let cameras: Vec<Value> = target.cameras.iter().map(|c| json!({"name": c.name})).collect();
    json!({"t": "info", "w": w, "h": h, "monitor": target.index, "monitors": target.monitors, "fps": params.fps, "viewOnly": view_only, "video": params.video, "encoder": params.encoder.name(),
        "source": target.source_name(), "camera": target.camera, "cameras": cameras, "media": params.media})
}

fn notice(out: &Mutex<Out>, text: &str) {
    let _ = send(out, &proto::json_frame(&json!({"t": "notice", "text": text})));
}

fn run(session: Arc<Session>, listener: TcpListener, expect: Expect, target: Target, params: Params, view_only: bool) {
    let dialed = proto::accept_dial(&listener, &expect, DIAL_WAIT);
    if let Ok(addr) = listener.local_addr() {
        anywhere::forbid_remote_port(addr.port());
    }
    drop(listener);
    match dialed {
        Ok(d) => {
            if let Err(e) = serve(&session, d, target, &params, view_only) {
                eprintln!("Remote: {e}");
            }
        }
        Err(e) => eprintln!("Remote: {e}"),
    }
    session.stop();
    // A session that another one took over leaves the pill and the slot to it.
    if release(&session) {
        ui::hide();
    }
    eprintln!("Remote: session {} with {} ended", session.id, session.phone_name);
    if let Some(app) = APP.get() {
        let _ = app.emit("remote-ended", json!({"phoneId": session.phone_id}));
    }
}

fn serve(session: &Arc<Session>, d: Dialed, target: Target, params: &Params, view_only: bool) -> Result<()> {
    let Dialed { stream, mut reader, mut rx, tx } = d;
    stream.set_read_timeout(Some(DEAD_AFTER)).map_err(|e| e.to_string())?;
    stream.set_write_timeout(Some(Duration::from_secs(5))).map_err(|e| e.to_string())?;
    *lock(&session.stream) = Some(stream.try_clone().map_err(|e| e.to_string())?);
    let out = Arc::new(Mutex::new(Out { stream: stream.try_clone().map_err(|e| e.to_string())?, cipher: tx }));
    eprintln!("Remote: session {} with {} started{}", session.id, session.phone_name, if session.internet { " over the internet" } else { "" });
    send(&out, &proto::json_frame(&info(&target, params, view_only)))?;
    ui::live(&session.phone_name, view_only, session.internet);
    ui::media(target.camera.is_some(), false);
    let _kill = {
        let s = session.clone();
        killswitch::arm(&params.display, move || {
            eprintln!("Remote: Ctrl+Alt+Esc, ending the session");
            s.stop();
        })
        .map_err(|e| eprintln!("Remote: {e}"))
        .ok()
    };

    let target = Arc::new(Mutex::new(target));
    let want_key = Arc::new(AtomicBool::new(false));
    let kbps = Arc::new(Mutex::new(params.kbps));
    // The picture can be switched on and off during a session (the Touchpad tab has none).
    let video_on = Arc::new(AtomicBool::new(params.video));
    let video = {
        let (session, out, target, want_key, kbps, on) = (session.clone(), out.clone(), target.clone(), want_key.clone(), kbps.clone(), video_on.clone());
        let (display, fps, encoder) = (params.display.clone(), params.fps, params.encoder);
        Some(std::thread::Builder::new().name("remote-video".into()).spawn(move || {
            if let Err(e) = video_loop(&session, &out, &target, &want_key, &on, &kbps, &display, fps, encoder) {
                eprintln!("Remote: {e}");
                let _ = send(&out, &proto::json_frame(&json!({"t": "error", "error": e})));
                session.stop();
            }
        }))
    };

    // The phone's sound: off until it asks.
    let audio_want = Arc::new(Mutex::new(audio::Audio::Off));
    let sound = {
        let (session, out, want) = (session.clone(), out.clone(), audio_want.clone());
        std::thread::Builder::new().name("remote-audio".into()).spawn(move || audio_loop(&session, &out, &want)).ok()
    };

    let mut injector = if view_only {
        None
    } else {
        match inject::Injector::open(&params.display) {
            Ok(i) => Some(i),
            Err(e) => {
                eprintln!("Remote: {e}");
                None
            }
        }
    };
    let mut seen_version = u64::MAX;
    let mut locked = (Instant::now(), false);
    let result = loop {
        if session.stopped() {
            break Ok(());
        }
        let plain = match proto::read_sealed(&mut reader, &mut rx) {
            Ok(p) => p,
            Err(e) => break if session.stopped() { Ok(()) } else { Err(e) },
        };
        let Some((&kind, body)) = plain.split_first() else { continue };
        match kind {
            proto::K_JSON => {
                let Ok(v) = serde_json::from_slice::<Value>(body) else { continue };
                match v.get("t").and_then(Value::as_str).unwrap_or("") {
                    "ping" => {
                        let _ = send(&out, &proto::json_frame(&json!({"t": "pong", "ts": v.get("ts").cloned().unwrap_or(Value::Null)})));
                    }
                    "keyframe" => want_key.store(true, Ordering::SeqCst),
                    "video" => {
                        video_on.store(v.get("on").and_then(Value::as_bool).unwrap_or(true), Ordering::SeqCst);
                        want_key.store(true, Ordering::SeqCst);
                    }
                    "bitrate" => *lock(&kbps) = v.get("kbps").and_then(Value::as_u64).unwrap_or(4000).clamp(300, 30_000) as u32,
                    // A new picture size and bitrate (the phone's quality chip): the capture restarts.
                    "quality" => {
                        let num = |k: &str| v.get(k).and_then(Value::as_u64);
                        let mut t = lock(&target);
                        if let (Some(w), Some(h)) = (num("maxW"), num("maxH")) {
                            t.max = (w.clamp(320, 3840) as u32, h.clamp(240, 2160) as u32);
                            t.version += 1;
                        }
                        if let Some(k) = num("kbps") {
                            *lock(&kbps) = k.clamp(300, 30_000) as u32;
                        }
                        let msg = info(&t, params, view_only);
                        drop(t);
                        let _ = send(&out, &proto::json_frame(&msg));
                    }
                    // The picture shows the screen or a camera.
                    "source" => {
                        let camera = v.get("src").and_then(Value::as_str) == Some("camera");
                        let mut t = lock(&target);
                        if camera && !params.media {
                            drop(t);
                            notice(&out, MEDIA_OFF);
                            continue;
                        }
                        if camera && t.cameras.is_empty() {
                            drop(t);
                            notice(&out, "This PC has no camera.");
                            continue;
                        }
                        eprintln!("Remote: the picture switches to the {}", if camera { "camera" } else { "screen" });
                        let next = camera.then(|| v.get("i").and_then(Value::as_u64).map(|i| i as usize).filter(|i| *i < t.cameras.len()).unwrap_or(0));
                        if next != t.camera {
                            t.camera = next;
                            t.version += 1;
                        }
                        let msg = info(&t, params, view_only);
                        drop(t);
                        want_key.store(true, Ordering::SeqCst);
                        ui::media(next.is_some(), *lock(&audio_want) == audio::Audio::Mic);
                        let _ = send(&out, &proto::json_frame(&msg));
                    }
                    "audio" => {
                        let which = audio::Audio::parse(v.get("src").and_then(Value::as_str).unwrap_or("off"));
                        if which == audio::Audio::Mic && !params.media {
                            notice(&out, MEDIA_OFF);
                            continue;
                        }
                        eprintln!("Remote: sound {}", which.name());
                        *lock(&audio_want) = which;
                        ui::media(lock(&target).camera.is_some(), which == audio::Audio::Mic);
                    }
                    "monitor" => {
                        let mut t = lock(&target);
                        if let Some(i) = v.get("i").and_then(Value::as_u64).map(|i| i as usize).filter(|i| *i < t.monitors.len()) {
                            t.index = i;
                            t.camera = None;
                            t.version += 1;
                            let msg = info(&t, params, view_only);
                            drop(t);
                            let _ = send(&out, &proto::json_frame(&msg));
                        }
                    }
                    "stop" => break Ok(()),
                    _ => {}
                }
            }
            proto::K_INPUT => {
                let event = match Input::decode(body) {
                    Ok(e) => e,
                    Err(e) => {
                        eprintln!("Remote: {e}");
                        continue;
                    }
                };
                // Nothing to point at while the picture is a camera; the media keys still work.
                if view_only || (lock(&target).camera.is_some() && !matches!(event, Input::Media(_))) {
                    continue;
                }
                if locked.0.elapsed() > Duration::from_secs(2) {
                    locked = (Instant::now(), screen_locked());
                }
                // On the lock screen only the media keys work: nothing may type a password.
                if let Input::Media(m) = &event {
                    if let Err(e) = media::control(*m) {
                        eprintln!("Remote: {e}");
                    }
                    continue;
                }
                if locked.1 {
                    continue;
                }
                let Some(inj) = injector.as_mut() else { continue };
                inj.tidy();
                {
                    let t = lock(&target);
                    if t.version != seen_version {
                        seen_version = t.version;
                        let m = t.monitor();
                        inj.set_area(m.x, m.y, m.w, m.h);
                    }
                }
                apply(inj, event);
            }
            _ => {}
        }
    };
    session.stop();
    if let Some(Ok(handle)) = video {
        join_for(handle, "picture", Duration::from_secs(3));
    }
    if let Some(handle) = sound {
        join_for(handle, "sound", Duration::from_secs(3));
    }
    drop(injector);
    let _ = send(&out, &proto::json_frame(&json!({"t": "end"})));
    result
}

fn apply(inj: &mut inject::Injector, event: Input) {
    match event {
        Input::MoveAbs { x, y } => inj.move_abs(x, y),
        Input::MoveRel { dx, dy } => inj.move_rel(dx, dy),
        Input::Button { button, down } => inj.button(button, down),
        Input::Wheel { dx, dy } => inj.wheel(dx, dy),
        Input::Key { keysym, down } => inj.key(keysym, down),
        Input::Text(text) => inj.text(&text),
        Input::Media(_) => {}
    }
}

/// Whether the desktop's lock screen is up (logind's LockedHint).
fn screen_locked() -> bool {
    let id = std::env::var("XDG_SESSION_ID").unwrap_or_default();
    if id.is_empty() {
        return false;
    }
    std::process::Command::new("loginctl")
        .args(["show-session", &id, "-p", "LockedHint", "--value"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "yes")
        .unwrap_or(false)
}

#[allow(clippy::too_many_arguments)]
fn video_loop(
    session: &Session,
    out: &Mutex<Out>,
    target: &Mutex<Target>,
    want_key: &AtomicBool,
    on: &AtomicBool,
    kbps: &Mutex<u32>,
    display: &str,
    fps: u32,
    encoder: capture::Encoder,
) -> Result<()> {
    let make = |t: &Target, kbps: u32| -> Result<capture::Capture> { capture::Capture::start(&capture::Params { source: t.source(display), size: t.size(), fps, kbps, encoder }) };
    let (mut version, mut rate) = {
        let t = lock(target);
        (t.version, *lock(kbps))
    };
    let mut cap: Option<capture::Capture> = None;
    let mut need_key = true;
    let started = Instant::now();
    while !session.stopped() {
        if !on.load(Ordering::SeqCst) {
            if let Some(c) = cap.take() {
                c.stop();
            }
            std::thread::sleep(Duration::from_millis(100));
            continue;
        }
        {
            let t = lock(target);
            if cap.is_none() || t.version != version {
                version = t.version;
                if let Some(c) = cap.take() {
                    c.stop();
                }
                cap = Some(match make(&t, rate) {
                    Ok(c) => c,
                    // A camera that won't open (busy in another app, unplugged): back to the screen.
                    Err(e) if t.camera.is_some() => {
                        drop(t);
                        notice(out, &format!("The camera didn't open: {e}"));
                        let mut t = lock(target);
                        t.camera = None;
                        t.version += 1;
                        ui::media(false, false);
                        continue;
                    }
                    Err(e) => return Err(e),
                });
                need_key = true;
            }
        }
        let Some(c) = cap.as_ref() else { continue };
        let wanted = *lock(kbps);
        if wanted != rate {
            rate = wanted;
            c.set_bitrate(rate);
        }
        if want_key.swap(false, Ordering::SeqCst) {
            c.force_key();
            need_key = true;
        }
        let Some(unit) = c.pull(100)? else { continue };
        // After a slow send, frames were dropped on the way: wait for a keyframe, and ask for one.
        if need_key && !unit.key {
            c.force_key();
            continue;
        }
        if unit.key {
            need_key = false;
        }
        let pts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_micros() as u64).unwrap_or(0);
        let before = Instant::now();
        send(out, &proto::video_frame(unit.key, pts, &unit.data))?;
        if before.elapsed() > SLOW_SEND && started.elapsed() > Duration::from_secs(1) {
            need_key = true;
            c.force_key();
        }
    }
    if let Some(c) = cap {
        c.stop();
    }
    Ok(())
}

/// Sends the sound the phone asked for, switching sources as it asks, until the session ends.
fn audio_loop(session: &Session, out: &Mutex<Out>, want: &Mutex<audio::Audio>) {
    let mut current = audio::Audio::Off;
    let mut cap: Option<audio::Capture> = None;
    while !session.stopped() {
        let wanted = *lock(want);
        if wanted != current {
            if let Some(c) = cap.take() {
                c.stop();
            }
            current = wanted;
            if wanted != audio::Audio::Off {
                match audio::Capture::start(wanted, 64) {
                    Ok(c) => cap = Some(c),
                    Err(e) => {
                        notice(out, &e);
                        *lock(want) = audio::Audio::Off;
                        current = audio::Audio::Off;
                    }
                }
            }
            let _ = send(out, &proto::json_frame(&json!({"t": "audio", "src": current.name(), "codec": "opus", "rate": audio::RATE, "channels": audio::CHANNELS, "preSkip": audio::PRE_SKIP})));
        }
        let Some(c) = cap.as_ref() else {
            std::thread::sleep(Duration::from_millis(100));
            continue;
        };
        match c.pull(100) {
            Ok(Some(packet)) => {
                let pts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_micros() as u64).unwrap_or(0);
                if send(out, &proto::audio_frame(pts, &packet)).is_err() {
                    break;
                }
            }
            Ok(None) => {}
            Err(e) => {
                notice(out, &e);
                *lock(want) = audio::Audio::Off;
            }
        }
    }
    if let Some(c) = cap {
        c.stop();
    }
}

// ---- the page's commands --------------------------------------------------------------------

#[tauri::command]
pub fn remote_settings() -> store::Settings {
    store::get()
}

#[tauri::command]
pub async fn remote_set_settings(settings: store::Settings) -> Result<store::Settings> {
    tauri::async_runtime::spawn_blocking(move || {
        let saved = store::set(settings)?;
        if let Err(e) = anywhere::apply() {
            // It couldn't start: don't leave the switch on.
            if saved.internet {
                store::set(store::Settings { internet: false, ..saved })?;
                anywhere::stop();
            }
            return Err(e);
        }
        Ok(saved)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The internet path's state for Settings.
#[tauri::command]
pub fn remote_internet_status() -> anywhere::Status {
    anywhere::status()
}

/// Settings' Test: does the public hostname reach this PC?
#[tauri::command]
pub async fn remote_internet_test() -> Result<Value> {
    tauri::async_runtime::spawn_blocking(anywhere::check).await.map_err(|e| e.to_string())?
}

/// The window's answer to "<phone> wants to control this PC": "once", "always" or "deny".
#[tauri::command]
pub fn remote_answer(choice: String) {
    ui::answer(ui::Choice::parse(&choice));
}

/// The pill's Stop button.
#[tauri::command]
pub fn remote_stop() {
    if let Some(s) = lock(&ACTIVE).clone() {
        s.stop();
    }
}

#[tauri::command]
pub fn remote_ui_state() -> ui::UiState {
    ui::state()
}

/// Who is controlling this PC right now (the phone's id), if anyone.
#[tauri::command]
pub fn remote_active() -> Option<String> {
    lock(&ACTIVE).as_ref().map(|s| s.phone_id.clone())
}

// ---- this PC controlling the phone ------------------------------------------------------------

fn phone_link(phone_id: &str) -> Result<Arc<Link>> {
    crate::phonelink::hub().and_then(|h| h.link(phone_id)).ok_or_else(|| "The phone isn't connected.".to_string())
}

/// Asks the phone for control of its screen (its user answers on the phone; `control.start`).
#[tauri::command]
pub async fn phone_control_start(phone_id: String) -> Result<()> {
    tauri::async_runtime::spawn_blocking(move || {
        let name = crate::phonelink::hub().map(|h| h.pc_name()).unwrap_or_default();
        phone_link(&phone_id)?.request(json!({"t": "control.start", "name": name}), Duration::from_secs(75))?;
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// One control action for the phone (`ctl`: tap, swipe, key, text, global actions; docs/plans/remote.md).
#[tauri::command]
pub fn phone_control(phone_id: String, action: Value) -> Result<()> {
    let mut msg = match action {
        Value::Object(m) => m,
        _ => return Err("Not a control action.".into()),
    };
    msg.insert("t".into(), json!("ctl"));
    phone_link(&phone_id)?.send(Value::Object(msg))
}

#[tauri::command]
pub fn phone_control_stop(phone_id: String) -> Result<()> {
    phone_link(&phone_id)?.send(json!({"t": "control.stop"}))
}

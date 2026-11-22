//! Ctrl+Alt+A (answer) and Ctrl+Alt+D (decline) for a ringing call, from anywhere on an X11
//! desktop. The keys are grabbed only while a call rings, so the rest of the time other programs
//! keep them. Xlib is loaded at run time (x11-dl). On Wayland nothing is grabbed: the keys then
//! work while a Zazabor window has the focus (src/alerts/keys.ts and the alerts window).

use std::os::raw::{c_int, c_uint};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use x11_dl::xlib;

static STOP: Mutex<Option<Arc<AtomicBool>>> = Mutex::new(None);

fn libs() -> Option<&'static xlib::Xlib> {
    static LIBS: OnceLock<Option<xlib::Xlib>> = OnceLock::new();
    LIBS.get_or_init(|| xlib::Xlib::open().ok()).as_ref()
}

fn wayland() -> bool {
    std::env::var("XDG_SESSION_TYPE").is_ok_and(|t| t.eq_ignore_ascii_case("wayland")) || std::env::var_os("DISPLAY").is_none()
}

/// What a grabbed key asks for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Key {
    Answer,
    Decline,
}

/// Starts (true) or ends (false) the grab. `on_key` runs on the grab's thread.
pub fn set_active(on: bool, on_key: impl Fn(Key) + Send + 'static) {
    let mut slot = STOP.lock().unwrap_or_else(|e| e.into_inner());
    if on {
        if slot.is_some() || wayland() {
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        *slot = Some(stop.clone());
        std::thread::spawn(move || run(&stop, on_key));
    } else if let Some(stop) = slot.take() {
        stop.store(true, Ordering::Relaxed);
    }
}

extern "C" fn ignore_errors(_: *mut xlib::Display, _: *mut xlib::XErrorEvent) -> c_int {
    // Another program already holds the keys (BadAccess): then they stay its own.
    0
}

fn run(stop: &AtomicBool, on_key: impl Fn(Key)) {
    let Some(x) = libs() else { return };
    // SAFETY: a display of this thread's own, closed before returning; every call below uses it
    // only here.
    unsafe {
        let display = (x.XOpenDisplay)(ptr::null());
        if display.is_null() {
            return;
        }
        (x.XSetErrorHandler)(Some(ignore_errors));
        let root = (x.XDefaultRootWindow)(display);
        let code = |sym: u32| (x.XKeysymToKeycode)(display, sym as _) as c_uint;
        let (a, d) = (code(x11_dl::keysym::XK_a), code(x11_dl::keysym::XK_d));
        // Ctrl+Alt, with and without Caps Lock and Num Lock.
        let base = xlib::ControlMask | xlib::Mod1Mask;
        let extras = [0, xlib::LockMask, xlib::Mod2Mask, xlib::LockMask | xlib::Mod2Mask];
        for key in [a, d] {
            for extra in extras {
                (x.XGrabKey)(display, key as c_int, base | extra, root, xlib::False, xlib::GrabModeAsync, xlib::GrabModeAsync);
            }
        }
        (x.XSync)(display, xlib::False);
        while !stop.load(Ordering::Relaxed) {
            while (x.XPending)(display) > 0 {
                let mut event: xlib::XEvent = std::mem::zeroed();
                (x.XNextEvent)(display, &mut event);
                if event.get_type() == xlib::KeyPress {
                    let pressed = event.key.keycode;
                    if pressed == a {
                        on_key(Key::Answer);
                    } else if pressed == d {
                        on_key(Key::Decline);
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(40));
        }
        for key in [a, d] {
            for extra in extras {
                (x.XUngrabKey)(display, key as c_int, base | extra, root);
            }
        }
        (x.XSync)(display, xlib::False);
        (x.XCloseDisplay)(display);
    }
}

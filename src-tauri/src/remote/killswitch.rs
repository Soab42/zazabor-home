//! The kill switch: Ctrl+Alt+Esc ends a remote session at once, from the PC's own keyboard.
//! While a session runs, the key is grabbed on the root window, so no application sees it and
//! nothing the phone does can swallow it (the phone's own key events are injected after the
//! grab, and a grabbed combination still triggers on injected presses).

use std::os::raw::{c_int, c_uint, c_ulong};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use x11_dl::{keysym, xlib};

/// A running grab; `release` (or dropping it) lets go.
pub struct Guard {
    stop: Arc<AtomicBool>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Modifier combinations that still mean Ctrl+Alt: with Caps Lock and/or Num Lock on.
fn variants() -> [c_uint; 4] {
    let base = xlib::ControlMask | xlib::Mod1Mask;
    [base, base | xlib::LockMask, base | xlib::Mod2Mask, base | xlib::LockMask | xlib::Mod2Mask]
}

/// Grabs Ctrl+Alt+Esc on `display` and calls `on_trigger` (once) when it's pressed.
pub fn arm(display: &str, on_trigger: impl Fn() + Send + 'static) -> Result<Guard, String> {
    let x = xlib::Xlib::open().map_err(|_| "Xlib isn't available.".to_string())?;
    let name = std::ffi::CString::new(display).map_err(|_| "Bad display name.".to_string())?;
    // SAFETY: the connection is used by the thread below only, and closed there.
    let d = unsafe { (x.XOpenDisplay)(name.as_ptr()) };
    if d.is_null() {
        return Err("Couldn't open the display for the kill switch.".into());
    }
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    struct Conn(xlib::Xlib, *mut xlib::Display);
    // SAFETY: handed to exactly one thread.
    unsafe impl Send for Conn {}
    let conn = Conn(x, d);
    std::thread::Builder::new()
        .name("remote-kill".into())
        .spawn(move || {
            let conn = conn;
            let (x, d) = (&conn.0, conn.1);
            // SAFETY: `d` is this thread's own connection.
            unsafe {
                let root = (x.XDefaultRootWindow)(d);
                let key = (x.XKeysymToKeycode)(d, keysym::XK_Escape as c_ulong) as c_int;
                for m in variants() {
                    (x.XGrabKey)(d, key, m, root, xlib::True, xlib::GrabModeAsync, xlib::GrabModeAsync);
                }
                (x.XSync)(d, xlib::False);
                let mut fired = false;
                while !flag.load(Ordering::SeqCst) {
                    while (x.XPending)(d) > 0 {
                        let mut ev: xlib::XEvent = std::mem::zeroed();
                        (x.XNextEvent)(d, &mut ev);
                        if ev.get_type() == xlib::KeyPress && !fired {
                            fired = true;
                            on_trigger();
                        }
                    }
                    std::thread::sleep(Duration::from_millis(40));
                }
                for m in variants() {
                    (x.XUngrabKey)(d, key, m, root);
                }
                (x.XCloseDisplay)(d);
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(Guard { stop })
}

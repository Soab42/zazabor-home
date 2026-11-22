//! Pointer, keys and text into the X session through XTest (Xlib is loaded at run time, x11-dl).
//!
//! One `Injector` owns a connection of its own and is used from one thread. Positions arrive as
//! 0..65535 across the captured monitor and become pixels of that monitor's rectangle. Text is
//! typed through the keyboard map: a character on a key just presses it (with Shift for the
//! second level); any other (emoji, Bengali, …) goes through a spare keycode that is bound to
//! its keysym, the way `xdotool type` does. Applications read a key's keysym from the keyboard map
//! when they handle the event, so a scratch binding stays for a second after its last use (and a
//! few keycodes take turns), instead of being cleared the moment the key is released.

use std::collections::HashSet;
use std::os::raw::{c_int, c_ulong};
use std::time::{Duration, Instant};

use x11_dl::{keysym, xlib, xtest};

type Result<T> = std::result::Result<T, String>;

/// Where a keysym sits in the keyboard map: the keycode and the level (0 plain, 1 with Shift).
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct Place {
    pub keycode: u8,
    pub shift: bool,
}

/// The keysym a typed character is (Latin-1 as is, the rest as Unicode keysyms).
pub fn char_keysym(c: char) -> Option<u32> {
    match c {
        '\n' | '\r' => Some(keysym::XK_Return),
        '\t' => Some(keysym::XK_Tab),
        c if (c as u32) < 0x20 || c as u32 == 0x7f => None,
        c if (c as u32) <= 0xff => Some(c as u32),
        c => Some(0x0100_0000 | c as u32),
    }
}

/// The pixel a 0..65535 position is, along an axis that starts at `origin` and is `size` long.
pub fn to_pixel(v: u16, origin: i32, size: u32) -> i32 {
    origin + ((v as u64 * size.saturating_sub(1) as u64 + 32767) / 65535) as i32
}

/// Whole wheel clicks that `delta` eighths add up to, keeping the remainder in `acc`.
pub fn wheel_clicks(acc: &mut i32, delta: i16) -> i32 {
    *acc += delta as i32;
    let clicks = *acc / 8;
    *acc -= clicks * 8;
    clicks
}

/// The X button for a wheel click: 4 up, 5 down, 6 left, 7 right.
pub fn wheel_button(horizontal: bool, positive: bool) -> u32 {
    match (horizontal, positive) {
        (false, false) => 4,
        (false, true) => 5,
        (true, false) => 6,
        (true, true) => 7,
    }
}

/// Finds `target` in a keyboard map (`per` keysyms for each keycode from `min`), on the first
/// two levels. Plain beats shifted.
pub fn locate(map: &[c_ulong], per: usize, min: u8, target: u32) -> Option<Place> {
    if per == 0 {
        return None;
    }
    let mut shifted = None;
    for (i, row) in map.chunks(per).enumerate() {
        let keycode = u8::try_from(min as usize + i).ok()?;
        if row.first() == Some(&(target as c_ulong)) {
            return Some(Place { keycode, shift: false });
        }
        if shifted.is_none() && row.get(1) == Some(&(target as c_ulong)) {
            shifted = Some(Place { keycode, shift: true });
        }
    }
    shifted
}

/// Up to `n` keycodes that no keysym is bound to (the last one first, where nothing real lives).
pub fn free_keycodes(map: &[c_ulong], per: usize, min: u8, n: usize) -> Vec<u8> {
    if per == 0 {
        return Vec::new();
    }
    map.chunks(per).enumerate().rev().filter(|(_, row)| row.iter().all(|s| *s == 0)).filter_map(|(i, _)| u8::try_from(min as usize + i).ok()).take(n).collect()
}

/// How long a scratch keycode keeps its keysym after the last use.
const SCRATCH_KEEP: Duration = Duration::from_millis(1000);
/// How many scratch keycodes take turns.
const SCRATCH_SLOTS: usize = 8;

struct Libs {
    x: xlib::Xlib,
    tst: xtest::Xf86vmode,
}

pub struct Injector {
    libs: Libs,
    d: *mut xlib::Display,
    /// The captured monitor: x, y, width, height.
    area: (i32, i32, u32, u32),
    wheel_x: i32,
    wheel_y: i32,
    held_keys: HashSet<u8>,
    held_buttons: HashSet<u32>,
    /// Scratch keycodes and the keysym each holds, the next one to take, and when last used.
    scratch: Vec<(u8, c_ulong)>,
    next_scratch: usize,
    scratch_used: Option<Instant>,
}

// SAFETY: the display connection is only used by the thread that owns the injector.
unsafe impl Send for Injector {}

impl Injector {
    pub fn open(display: &str) -> Result<Injector> {
        let x = xlib::Xlib::open().map_err(|_| "Xlib isn't available.".to_string())?;
        let tst = xtest::Xf86vmode::open().map_err(|_| "XTest isn't available.".to_string())?;
        let name = std::ffi::CString::new(display).map_err(|_| "Bad display name.".to_string())?;
        // SAFETY: closed in Drop.
        let d = unsafe { (x.XOpenDisplay)(name.as_ptr()) };
        if d.is_null() {
            return Err(format!("Couldn't open the display {display}."));
        }
        Ok(Injector { libs: Libs { x, tst }, d, area: (0, 0, 1, 1), wheel_x: 0, wheel_y: 0, held_keys: HashSet::new(), held_buttons: HashSet::new(), scratch: Vec::new(), next_scratch: 0, scratch_used: None })
    }

    pub fn set_area(&mut self, x: i32, y: i32, w: u32, h: u32) {
        self.area = (x, y, w.max(1), h.max(1));
    }

    fn flush(&self) {
        // SAFETY: `d` is open.
        unsafe { (self.libs.x.XFlush)(self.d) };
    }

    pub fn move_abs(&mut self, x: u16, y: u16) {
        let (ax, ay, w, h) = self.area;
        let (px, py) = (to_pixel(x, ax, w), to_pixel(y, ay, h));
        // SAFETY: `d` is open.
        unsafe { (self.libs.tst.XTestFakeMotionEvent)(self.d, -1, px, py, 0) };
        self.flush();
    }

    pub fn move_rel(&mut self, dx: i16, dy: i16) {
        // SAFETY: `d` is open.
        unsafe { (self.libs.x.XWarpPointer)(self.d, 0, 0, 0, 0, 0, 0, dx as c_int, dy as c_int) };
        self.flush();
    }

    pub fn button(&mut self, button: u8, down: bool) {
        let b = button as u32;
        self.press_button(b, down);
        self.flush();
    }

    fn press_button(&mut self, b: u32, down: bool) {
        // SAFETY: `d` is open.
        unsafe { (self.libs.tst.XTestFakeButtonEvent)(self.d, b, down as c_int, 0) };
        if down {
            self.held_buttons.insert(b);
        } else {
            self.held_buttons.remove(&b);
        }
    }

    pub fn wheel(&mut self, dx: i16, dy: i16) {
        for (horizontal, delta) in [(false, dy), (true, dx)] {
            let acc = if horizontal { &mut self.wheel_x } else { &mut self.wheel_y };
            let clicks = wheel_clicks(acc, delta);
            let b = wheel_button(horizontal, clicks > 0);
            for _ in 0..clicks.unsigned_abs().min(20) {
                // SAFETY: `d` is open.
                unsafe {
                    (self.libs.tst.XTestFakeButtonEvent)(self.d, b, 1, 0);
                    (self.libs.tst.XTestFakeButtonEvent)(self.d, b, 0, 0);
                }
            }
        }
        self.flush();
    }

    /// The map as (keysyms, per keycode, first keycode).
    fn keymap(&self) -> Option<(Vec<c_ulong>, usize, u8)> {
        // SAFETY: `d` is open; the returned array is copied and freed.
        unsafe {
            let x = &self.libs.x;
            let (mut min, mut max) = (0, 0);
            (x.XDisplayKeycodes)(self.d, &mut min, &mut max);
            let count = max - min + 1;
            let mut per = 0;
            let list = (x.XGetKeyboardMapping)(self.d, min as u8, count, &mut per);
            if list.is_null() || per <= 0 {
                return None;
            }
            let map = std::slice::from_raw_parts(list, (count * per) as usize).to_vec();
            (x.XFree)(list.cast());
            Some((map, per as usize, min as u8))
        }
    }

    fn fake_key(&mut self, keycode: u8, down: bool) {
        // SAFETY: `d` is open.
        unsafe { (self.libs.tst.XTestFakeKeyEvent)(self.d, keycode as u32, down as c_int, 0) };
        if down {
            self.held_keys.insert(keycode);
        } else {
            self.held_keys.remove(&keycode);
        }
    }

    fn shift_code(&self) -> Option<u8> {
        // SAFETY: `d` is open.
        let kc = unsafe { (self.libs.x.XKeysymToKeycode)(self.d, keysym::XK_Shift_L as c_ulong) };
        (kc != 0).then_some(kc)
    }

    /// A key going down or up by keysym (shifted ones hold Shift around themselves).
    pub fn key(&mut self, sym: u32, down: bool) {
        let Some((map, per, min)) = self.keymap() else { return };
        let Some(place) = locate(&map, per, min, sym) else { return };
        let shift = if place.shift { self.shift_code() } else { None };
        if down {
            if let Some(s) = shift {
                self.fake_key(s, true);
            }
            self.fake_key(place.keycode, true);
        } else {
            self.fake_key(place.keycode, false);
            if let Some(s) = shift {
                self.fake_key(s, false);
            }
        }
        self.flush();
    }

    /// Types `text` as if from the keyboard.
    pub fn text(&mut self, text: &str) {
        let Some((map, per, min)) = self.keymap() else { return };
        if self.scratch.is_empty() {
            self.scratch = free_keycodes(&map, per, min, SCRATCH_SLOTS).into_iter().map(|k| (k, 0)).collect();
            self.next_scratch = 0;
        }
        for c in text.chars() {
            let Some(sym) = char_keysym(c) else { continue };
            if let Some(place) = locate(&map, per, min, sym) {
                let shift = if place.shift { self.shift_code() } else { None };
                if let Some(s) = shift {
                    self.fake_key(s, true);
                }
                self.fake_key(place.keycode, true);
                self.fake_key(place.keycode, false);
                if let Some(s) = shift {
                    self.fake_key(s, false);
                }
                self.flush();
            } else if let Some(slot) = self.scratch_for(sym as c_ulong) {
                self.fake_key(slot, true);
                self.fake_key(slot, false);
                self.flush();
                self.scratch_used = Some(Instant::now());
            }
        }
    }

    /// The scratch keycode bound to `sym`, binding the next free turn when none is (and waiting for
    /// applications to see the new mapping before the key goes in).
    fn scratch_for(&mut self, sym: c_ulong) -> Option<u8> {
        if let Some((k, _)) = self.scratch.iter().find(|(_, s)| *s == sym) {
            return Some(*k);
        }
        if self.scratch.is_empty() {
            return None;
        }
        let i = self.next_scratch % self.scratch.len();
        self.next_scratch += 1;
        let keycode = self.scratch[i].0;
        self.bind(keycode, sym);
        self.scratch[i].1 = sym;
        std::thread::sleep(Duration::from_millis(20));
        Some(keycode)
    }

    fn bind(&self, keycode: u8, sym: c_ulong) {
        let mut sym = sym;
        // SAFETY: `d` is open; one keycode with one keysym.
        unsafe {
            (self.libs.x.XChangeKeyboardMapping)(self.d, keycode as c_int, 1, &mut sym, 1);
            (self.libs.x.XSync)(self.d, xlib::False);
        }
    }

    /// Clears the scratch bindings once they have been idle for a while (call before each event).
    pub fn tidy(&mut self) {
        if self.scratch_used.is_some_and(|at| at.elapsed() > SCRATCH_KEEP) {
            self.unbind_scratch();
        }
    }

    fn unbind_scratch(&mut self) {
        for i in 0..self.scratch.len() {
            if self.scratch[i].1 != 0 {
                self.bind(self.scratch[i].0, 0);
                self.scratch[i].1 = 0;
            }
        }
        self.scratch_used = None;
    }

    /// Lets go of whatever is still held (a session ending mid-drag mustn't leave Ctrl pressed).
    pub fn release_all(&mut self) {
        for kc in self.held_keys.drain().collect::<Vec<_>>() {
            // SAFETY: `d` is open.
            unsafe { (self.libs.tst.XTestFakeKeyEvent)(self.d, kc as u32, 0, 0) };
        }
        for b in self.held_buttons.drain().collect::<Vec<_>>() {
            // SAFETY: `d` is open.
            unsafe { (self.libs.tst.XTestFakeButtonEvent)(self.d, b, 0, 0) };
        }
        self.unbind_scratch();
        self.flush();
    }
}

impl Drop for Injector {
    fn drop(&mut self) {
        self.release_all();
        // SAFETY: opened in `open`, closed once.
        unsafe { (self.libs.x.XCloseDisplay)(self.d) };
    }
}

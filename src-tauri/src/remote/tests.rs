use std::io::{BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use serde_json::json;

use super::capture::{description, Encoder, Params};
use super::inject::{char_keysym, free_keycodes, locate, to_pixel, wheel_button, wheel_clicks, Place};
use super::proto::*;
use super::{pick_monitor, MonitorInfo};
use crate::phonelink::link::{base64, read_line, unbase64, Cipher};

#[test]
fn input_events_round_trip() {
    let events = [
        Input::MoveAbs { x: 0, y: 65535 },
        Input::MoveAbs { x: 32768, y: 12345 },
        Input::MoveRel { dx: -5, dy: 300 },
        Input::Button { button: 1, down: true },
        Input::Button { button: 3, down: false },
        Input::Wheel { dx: -16, dy: 8 },
        Input::Key { keysym: 0xff1b, down: true },
        Input::Key { keysym: 0x0100_09b5, down: false },
        Input::Text("héllo বাংলা 😀".into()),
        Input::Media(Media::PlayPause),
        Input::Media(Media::SeekFwd),
    ];
    for e in events {
        let plain = e.encode();
        assert_eq!(plain[0], K_INPUT);
        assert_eq!(Input::decode(&plain[1..]).unwrap(), e, "{e:?}");
    }
}

#[test]
fn input_events_have_a_fixed_layout() {
    // The phone's Kotlin encoder must produce exactly these bytes.
    assert_eq!(Input::MoveAbs { x: 0x1234, y: 0xfffe }.encode(), vec![0x20, 1, 0x12, 0x34, 0xff, 0xfe]);
    assert_eq!(Input::MoveRel { dx: -2, dy: 3 }.encode(), vec![0x20, 2, 0xff, 0xfe, 0, 3]);
    assert_eq!(Input::Button { button: 3, down: true }.encode(), vec![0x20, 3, 3, 1]);
    assert_eq!(Input::Wheel { dx: 0, dy: -8 }.encode(), vec![0x20, 4, 0, 0, 0xff, 0xf8]);
    assert_eq!(Input::Key { keysym: 0xff0d, down: true }.encode(), vec![0x20, 5, 0, 0, 0xff, 0x0d, 1]);
    assert_eq!(Input::Text("ab".into()).encode(), vec![0x20, 6, 0, 2, b'a', b'b']);
    assert_eq!(Input::Media(Media::Mute).encode(), vec![0x20, 7, 6]);
}

#[test]
fn bad_input_events_are_refused() {
    for body in [
        &[][..],
        &[1, 0, 0][..],
        &[3, 4, 1][..],
        &[3, 1][..],
        &[5, 0, 0, 0, 1][..],
        &[6, 0, 5, b'a'][..],
        &[6, 0, 2, 0xff, 0xfe][..],
        &[7, 0][..],
        &[7, 99][..],
        &[200, 1, 2][..],
    ] {
        assert!(Input::decode(body).is_err(), "{body:?}");
    }
    // Text over the limit is cut at a character, not in the middle of one.
    let long = "é".repeat(MAX_TEXT);
    let Input::Text(t) = Input::decode(&Input::Text(long).encode()[1..]).unwrap() else { panic!() };
    assert!(t.len() <= MAX_TEXT && t.chars().all(|c| c == 'é'));
}

#[test]
fn video_frames_carry_their_flags_and_time() {
    let f = video_frame(true, 0x0102_0304_0506_0708, &[0, 0, 0, 1, 0x65]);
    assert_eq!(&f[..10], &[0x10, 1, 1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(&f[10..], &[0, 0, 0, 1, 0x65]);
    assert_eq!(video_frame(false, 0, &[])[1], 0);
}

#[test]
fn sessions_get_their_own_keys() {
    let key = [7u8; 32];
    let (a, b) = session_keys(&key, &[1; 16], &[2; 16]);
    assert_ne!(a, b);
    // Not the keys of the link itself, and not those of another session.
    let (link_up, _) = crate::phonelink::link::session_keys(&key, &[1; 16], &[2; 16]);
    assert_ne!(a, link_up);
    assert_ne!(session_keys(&key, &[1; 16], &[3; 16]).0, a);
}

#[test]
fn sealed_frames_round_trip_and_reject_tampering() {
    let key = [9u8; 32];
    let (mut tx, mut rx) = (Cipher::new(&key), Cipher::new(&key));
    let mut wire = Vec::new();
    write_sealed(&mut wire, &mut tx, &video_frame(true, 5, &[1, 2, 3])).unwrap();
    write_sealed(&mut wire, &mut tx, &json_frame(&json!({"t": "pong"}))).unwrap();
    let mut r = &wire[..];
    assert_eq!(read_sealed(&mut r, &mut rx).unwrap(), video_frame(true, 5, &[1, 2, 3]));
    assert_eq!(read_sealed(&mut r, &mut rx).unwrap(), json_frame(&json!({"t": "pong"})));
    let mut bad = wire.clone();
    bad[10] ^= 1;
    assert!(read_sealed(&mut &bad[..], &mut Cipher::new(&key)).is_err());
    // Too big is refused before it is read.
    let mut big = ((MAX_FRAME + 1000) as u32).to_be_bytes().to_vec();
    big.extend_from_slice(&[0; 32]);
    assert!(read_sealed(&mut &big[..], &mut Cipher::new(&key)).is_err());
}

fn expect() -> Expect {
    Expect { phone_id: "ab".repeat(16), token: [5; 16], link_key: [3; 32], pc_nonce: [8; 16] }
}

/// The phone's side of a dial, as the Link app does it.
fn phone_dial(port: u16, phone_id: &str, token: &[u8], link_key: &[u8; 32]) -> Option<(TcpStream, Cipher, Cipher)> {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let nonce = [4u8; 16];
    let hello = json!({"phoneId": phone_id, "token": base64(token), "nonce": base64(&nonce)});
    s.write_all(format!("{MAGIC}\n{hello}\n").as_bytes()).unwrap();
    let mut reader = BufReader::new(s.try_clone().unwrap());
    let line = read_line(&mut reader).ok()?;
    let answer: serde_json::Value = serde_json::from_str(&line).unwrap();
    if answer["ok"] != json!(true) {
        return None;
    }
    let pc_nonce = unbase64(answer["nonce"].as_str().unwrap()).unwrap();
    let (up, down) = session_keys(link_key, &nonce, &pc_nonce);
    let mut tx = Cipher::new(&up);
    write_sealed(&mut s, &mut tx, &json_frame(&json!({"t": "ready"}))).unwrap();
    Some((s, tx, Cipher::new(&down)))
}

#[test]
fn the_right_phone_with_the_token_gets_in_and_talks_sealed() {
    let e = expect();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (id, token, key) = (e.phone_id.clone(), e.token, e.link_key);
    let phone = std::thread::spawn(move || {
        let (mut s, mut tx, mut rx) = phone_dial(port, &id, &token, &key).expect("accepted");
        // The PC's first sealed frame opens with the phone's keys, and input goes the other way.
        let mut reader = s.try_clone().unwrap();
        let got = read_sealed(&mut reader, &mut rx).unwrap();
        write_sealed(&mut s, &mut tx, &Input::Button { button: 1, down: true }.encode()).unwrap();
        got
    });
    let mut d = accept_dial(&listener, &e, Duration::from_secs(5)).unwrap();
    write_sealed(&mut d.stream, &mut d.tx, &json_frame(&json!({"t": "info", "w": 1280}))).unwrap();
    let plain = read_sealed(&mut d.reader, &mut d.rx).unwrap();
    assert_eq!(Input::decode(&plain[1..]).unwrap(), Input::Button { button: 1, down: true });
    assert_eq!(phone.join().unwrap(), json_frame(&json!({"t": "info", "w": 1280})));
}

#[test]
fn a_wrong_token_or_phone_is_refused_and_does_not_use_up_the_port() {
    let e = expect();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (id, token, key) = (e.phone_id.clone(), e.token, e.link_key);
    let phone = std::thread::spawn(move || {
        assert!(phone_dial(port, &id, &[6; 16], &key).is_none(), "wrong token");
        assert!(phone_dial(port, &"cd".repeat(16), &token, &key).is_none(), "wrong phone");
        // A phone with the right token but the wrong link key can't produce a `ready` that opens.
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let hello = json!({"phoneId": id, "token": base64(&token), "nonce": base64(&[4u8; 16])});
        s.write_all(format!("{MAGIC}\n{hello}\n").as_bytes()).unwrap();
        let mut buf = [0u8; 64];
        let _ = s.read(&mut buf);
        let (up, _) = session_keys(&[0xee; 32], &[4; 16], &[8; 16]);
        write_sealed(&mut s, &mut Cipher::new(&up), &json_frame(&json!({"t": "ready"}))).unwrap();
        drop(s);
        // The real one still gets in afterwards.
        phone_dial(port, &id, &token, &key).is_some()
    });
    assert!(accept_dial(&listener, &e, Duration::from_secs(10)).is_ok());
    assert!(phone.join().unwrap());
}

#[test]
fn nobody_dialling_times_out() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    assert!(accept_dial(&listener, &expect(), Duration::from_millis(200)).is_err());
}

#[test]
fn keysyms_for_typed_characters() {
    assert_eq!(char_keysym('a'), Some(0x61));
    assert_eq!(char_keysym('Z'), Some(0x5a));
    assert_eq!(char_keysym('é'), Some(0xe9));
    assert_eq!(char_keysym('\n'), Some(0xff0d));
    assert_eq!(char_keysym('\t'), Some(0xff09));
    assert_eq!(char_keysym('\u{7}'), None);
    // Everything beyond Latin-1 is a Unicode keysym.
    assert_eq!(char_keysym('ক'), Some(0x0100_0995));
    assert_eq!(char_keysym('😀'), Some(0x0101_f600));
}

#[test]
fn the_keyboard_map_is_searched_plain_before_shifted() {
    // Keycodes 8.. with two levels each: a/A, 1/!, then b/B.
    let map: Vec<std::os::raw::c_ulong> = vec![0x61, 0x41, 0x31, 0x21, 0x62, 0x42, 0, 0, 0, 0];
    assert_eq!(locate(&map, 2, 8, 0x61), Some(Place { keycode: 8, shift: false }));
    assert_eq!(locate(&map, 2, 8, 0x41), Some(Place { keycode: 8, shift: true }));
    assert_eq!(locate(&map, 2, 8, 0x21), Some(Place { keycode: 9, shift: true }));
    assert_eq!(locate(&map, 2, 8, 0x42), Some(Place { keycode: 10, shift: true }));
    assert_eq!(locate(&map, 2, 8, 0x7a), None);
    // A symbol on both levels of two keys is found on the plain one.
    let twice: Vec<std::os::raw::c_ulong> = vec![0x41, 0x61, 0x61, 0x41];
    assert_eq!(locate(&twice, 2, 20, 0x61), Some(Place { keycode: 21, shift: false }));
    // The spare keycode for a character that isn't on the keyboard is the last empty one.
    assert_eq!(free_keycodes(&map, 2, 8, 3), vec![12, 11]);
    assert_eq!(free_keycodes(&map, 2, 8, 1), vec![12]);
    assert!(free_keycodes(&[1, 2], 2, 8, 3).is_empty());
    assert_eq!(locate(&[], 0, 8, 1), None);
}

#[test]
fn positions_map_onto_the_monitors_rectangle() {
    assert_eq!(to_pixel(0, 1920, 1920), 1920);
    assert_eq!(to_pixel(65535, 1920, 1920), 1920 + 1919);
    assert_eq!(to_pixel(32768, 0, 1001), 500);
    assert_eq!(to_pixel(65535, -100, 1), -100);
}

#[test]
fn the_wheel_adds_up_eighths_into_clicks() {
    let mut acc = 0;
    assert_eq!(wheel_clicks(&mut acc, 3), 0);
    assert_eq!(wheel_clicks(&mut acc, 6), 1);
    assert_eq!(acc, 1);
    assert_eq!(wheel_clicks(&mut acc, -17), -2);
    assert_eq!(acc, 0);
    assert_eq!(wheel_clicks(&mut acc, -3), 0);
    assert_eq!(wheel_clicks(&mut acc, -5), -1);
    assert_eq!(acc, 0);
    assert_eq!((wheel_button(false, false), wheel_button(false, true), wheel_button(true, false), wheel_button(true, true)), (4, 5, 6, 7));
}

fn monitor(name: &str, x: i32, w: u32, primary: bool) -> MonitorInfo {
    MonitorInfo { name: name.into(), x, y: 0, w, h: 1080, primary }
}

#[test]
fn the_monitor_asked_for_else_the_primary() {
    let two = [monitor("DP-1", 0, 1920, false), monitor("DP-2", 1920, 1920, true)];
    assert_eq!(pick_monitor(&two, Some(0)), Some(0));
    assert_eq!(pick_monitor(&two, None), Some(1));
    assert_eq!(pick_monitor(&two, Some(7)), Some(1));
    assert_eq!(pick_monitor(&[monitor("a", 0, 10, false)], None), Some(0));
    assert_eq!(pick_monitor(&[], None), None);
}

#[test]
fn the_capture_pipeline_describes_one_monitor() {
    let p = Params { display: ":0".into(), rect: (1920, 0, 1920, 1080), size: (1600, 900), fps: 30, kbps: 5000, encoder: Encoder::X264 };
    let d = description(&p);
    assert!(d.contains("startx=1920 starty=0 endx=3839 endy=1079"), "{d}");
    assert!(d.contains("video/x-raw,width=1600,height=900"));
    assert!(d.contains("x264enc name=enc tune=zerolatency speed-preset=ultrafast bitrate=5000 key-int-max=120 bframes=0"));
    assert!(d.contains("stream-format=byte-stream,alignment=au ! appsink name=v sync=false"));
    let v = description(&Params { encoder: Encoder::Vaapi, ..p });
    assert!(v.contains("vaapih264enc name=enc rate-control=cqp") && v.contains("format=NV12"));
    assert_eq!(Encoder::parse("vaapi"), Encoder::Vaapi);
    assert_eq!(Encoder::parse("anything"), Encoder::X264);
}

#[test]
fn media_actions_map_to_mpris_and_the_mixer() {
    use super::media::{mpris_method, volume_args};
    assert_eq!(mpris_method(Media::PlayPause), Some("PlayPause"));
    assert_eq!(mpris_method(Media::Prev), Some("Previous"));
    assert_eq!(mpris_method(Media::SeekFwd), Some("Seek"));
    assert_eq!(mpris_method(Media::VolUp), None);
    assert!(volume_args(Media::VolUp).is_some() && volume_args(Media::Next).is_none());
}

#[test]
fn settings_are_cleaned() {
    let s = super::store::Settings { enabled: true, always: vec!["x".into(), "ab".repeat(16), "ab".repeat(16)], view_only: false, encoder: "nvenc".into() }.cleaned();
    assert_eq!(s.always, vec!["ab".repeat(16)]);
    assert_eq!(s.encoder, "x264");
    let d: super::store::Settings = serde_json::from_str(r#"{"enabled":true}"#).unwrap();
    assert!(d.enabled && d.always.is_empty() && d.encoder == "x264");
    assert!(!super::store::Settings::default().enabled, "remote control is off until the user turns it on");
}

/// Phase 0 spike: the injector against a real X display (`ZREM_TEST_DISPLAY=:46`). A test window
/// listens for what XTest delivers: pointer position, buttons, wheel, keys and typed Unicode.
#[test]
#[ignore = "needs an X display: ZREM_TEST_DISPLAY=:46 cargo test … xtest_reaches_a_window -- --ignored"]
fn xtest_reaches_a_window() {
    use std::os::raw::c_ulong;
    use x11_dl::xlib;
    let display = std::env::var("ZREM_TEST_DISPLAY").expect("ZREM_TEST_DISPLAY");
    let x = xlib::Xlib::open().unwrap();
    let name = std::ffi::CString::new(display.clone()).unwrap();
    // SAFETY: one connection of this test, used on this thread only.
    unsafe {
        let d = (x.XOpenDisplay)(name.as_ptr());
        assert!(!d.is_null());
        let screen = (x.XDefaultScreen)(d);
        let root = (x.XRootWindow)(d, screen);
        let w = (x.XCreateSimpleWindow)(d, root, 100, 100, 600, 400, 0, 0, 0);
        (x.XSelectInput)(d, w, xlib::KeyPressMask | xlib::ButtonPressMask | xlib::ButtonReleaseMask | xlib::PointerMotionMask | xlib::StructureNotifyMask | xlib::FocusChangeMask);
        (x.XMapRaised)(d, w);
        (x.XSetInputFocus)(d, w, xlib::RevertToParent, xlib::CurrentTime);
        (x.XSync)(d, xlib::False);
        std::thread::sleep(Duration::from_millis(300));

        let mut inj = super::inject::Injector::open(&display).unwrap();
        inj.set_area(0, 0, 1920, 1080);
        // 0.25, 0.25 of a 1920x1080 area is about 480, 270: inside the window (100..700, 100..500).
        inj.move_abs(16384, 16384);
        inj.button(1, true);
        inj.button(1, false);
        inj.button(3, true);
        inj.button(3, false);
        inj.wheel(0, 16);
        inj.wheel(8, 0);
        inj.key(0xff0d, true);
        inj.key(0xff0d, false);
        inj.text("aB é€বা😀");

        std::thread::sleep(Duration::from_millis(400));
        let (mut buttons, mut motion) = (Vec::new(), None);
        let mut keys = Vec::new();
        let mut typed = String::new();
        while (x.XPending)(d) > 0 {
            let mut ev: xlib::XEvent = std::mem::zeroed();
            (x.XNextEvent)(d, &mut ev);
            match ev.get_type() {
                xlib::MotionNotify => motion = Some((ev.motion.x_root, ev.motion.y_root)),
                xlib::ButtonPress => buttons.push(ev.button.button),
                xlib::KeyPress => {
                    let mut buf = [0 as std::os::raw::c_char; 16];
                    let mut sym: c_ulong = 0;
                    let n = (x.XLookupString)(&mut ev.key, buf.as_mut_ptr(), 16, &mut sym, std::ptr::null_mut());
                    keys.push(sym);
                    if n > 0 && sym < 0x100 {
                        typed.push(buf[0] as u8 as char);
                    }
                }
                xlib::MappingNotify => {
                    (x.XRefreshKeyboardMapping)(&mut ev.mapping);
                }
                _ => {}
            }
        }
        inj.release_all();
        println!("pointer {motion:?} buttons {buttons:?} keysyms {keys:x?} typed {typed:?}");
        let (px, py) = motion.expect("the pointer moved");
        assert!((479..=481).contains(&px) && (269..=271).contains(&py), "the pointer went to the monitor position: {px},{py}");
        // Left, right, wheel down (5), wheel right (7).
        assert_eq!(buttons, vec![1, 3, 5, 5, 7]);
        assert_eq!(keys.first(), Some(&0xff0d), "Return");
        assert!(typed.contains("aB"), "typed ASCII with Shift: {typed:?}");
        // Return, then 2 + space + é + € + ব + া + 😀 presses: every character became a key press,
        // the ones off the keyboard through a spare keycode.
        assert_eq!(keys.len(), 1 + 9, "every character produced a key press: {keys:x?}");
        // …and the application sees the right keysym for each one, the off-keyboard ones included.
        assert_eq!(&keys[5..], &[0xe9, 0x0100_20ac, 0x0100_09ac, 0x0100_09be, 0x0101_f600], "{keys:x?}");
        (x.XCloseDisplay)(d);
    }
}

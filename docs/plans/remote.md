# Remote: control the PC from the phone (like Monect)

The user asked on 2026-11-22: "control full pc with screen from mobile like monect", then: "same
from pc to mobile with any network". So it goes **both ways**, over **any network**:

- **Phone → PC:** the phone sees and controls the PC (below).
- **PC → phone:** the PC sees and controls the phone (section "Controlling the phone from the PC").
- **Any network:** both directions, and all of Phone Link, work away from home through the user's
  own domain (section "Any network").

**Zazabor Link** (the phone app, already paired with the PC over the encrypted ZLNK link,
docs/plans/phone-link.md) gets a **Remote** screen. The PC side lives in Zazabor Home.

## What the user gets (phone)

| Mode | What it does |
| --- | --- |
| **Screen** | The PC's screen live on the phone: pinch to zoom, pan, and touch controls the pointer. Tap = click, two-finger tap = right-click, long-press = drag, two-finger scroll. Toggle between "direct touch" (tap where you want) and "trackpad" (a relative cursor over the picture). Pick a monitor (the user has 2 × 1920×1080). |
| **Touchpad** | Full-screen trackpad, no video (lowest battery): move, tap/2-finger/3-finger gestures, scroll, left/right buttons, adjustable speed and acceleration. |
| **Keyboard** | The phone keyboard types on the PC. Special keys bar: Esc, Tab, Ctrl, Alt, Super, arrows, F1–F12, Del, Home/End, PgUp/PgDn. Shortcut chips: Copy, Paste, Undo, Alt+Tab, Super, lock screen. |
| **Media** | Play/pause, previous/next, volume, mute, and seek ±10 s. Works with Zazabor's player and any MPRIS player. |
| **Presenter** | Next/previous slide, black screen, and a laser pointer drawn as an overlay on the PC. |
| **Power** | Lock, sleep, shut down and restart (each asks to confirm on the phone). |

Gamepad mode (a virtual controller for games) needs `/dev/uinput`, which is root-only here. That
is phase 3, with a one-time udev rule the user installs.

## Security (remote control is the most dangerous feature in the app)

- Only a phone already **paired** through Phone Link (numeric comparison, per-phone link key) can
  ask. The request travels sealed over ZLNK.
- **Each session needs a yes on the PC**: a prompt "Pixel 7 wants to control this PC: Allow /
  Deny / Always allow this phone". "Always allow" is a per-phone setting that can be revoked in
  Settings.
- While a session runs, the PC shows a **bright "Controlled from <phone>" pill** on every monitor,
  with a Stop button. A **kill switch** works from the keyboard: Ctrl+Alt+Esc (rebindable) stops
  the session at once.
- View-only sessions are possible ("Screen" with input off). Input injection only happens while
  the session is active and the PC's screen is unlocked. When the screen is locked, only Media and
  Power (with confirmation) work.
- Video is encrypted with the session keys. The listener accepts one connection with a
  single-use token, on the LAN only.

## How it works

PC (Rust, `src-tauri/src/remote/`):

- **Capture + encode:** a GStreamer pipeline (reusing `recorder/pipeline.rs` pieces):
  `ximagesrc use-damage=false` (X11; `pipewiresrc` through the ScreenCast portal on Wayland
  later) → videoconvert/scale → H.264 low-latency. VA-API `vaapih264enc` when present, else
  `x264enc tune=zerolatency speed-preset=ultrafast`. Keyframe on demand, an adaptive bitrate of
  2–12 Mbps, 30/60 fps, and the resolution capped to the phone's screen (e.g. 1920×1080 → 1600×900
  on a phone).
- **The cursor is drawn by the phone** (the PC sends its position and shape), so it stays sharp and
  moves at once.
- **Transport:** after `remote.start` over ZLNK, the PC opens a one-shot TCP port. The phone connects
  and presents the token. Frames are sealed with ChaCha20-Poly1305 using keys derived from the
  link key (the same scheme as ZLNK). Input events come back the other way on the same socket
  (small binary records), so there is no JSON per mouse move. Expected latency on Wi-Fi is
  60–120 ms.
- **Input:** X11 XTest (`x11rb` or the `x11` crate with the xtest feature) for pointer, buttons,
  wheel, keys, and text through keysym mapping, including Unicode via a temporary keysym remap.
  Media keys go through MPRIS on D-Bus. Power goes through logind (D-Bus). Lock goes through
  loginctl.
- **Multi-monitor:** capture one output (xrandr geometry). The phone's coordinates map into that
  output's rectangle.

Phone (Kotlin, `mobile-cast/`, Zazabor Link):

- The Remote screen with mode tabs. `MediaCodec` H.264 decoder → SurfaceView (hardware, low
  latency: `KEY_LOW_LATENCY`, and render as soon as decoded). A gesture layer maps touches to
  events, and keeps the screen awake. Landscape for Screen mode.
- The link capability `remote` is advertised. The PC refuses it unless enabled in its settings.

## Controlling the phone from the PC

- **Seeing the phone's screen:** the existing mirroring (ZMIR → `cast::mirror` → the detached
  portrait mirror window). Over any network it runs through the same sealed WebSocket as below,
  instead of the LAN port.
- **Input without root:** Zazabor Link adds an **Accessibility service** (`RemoteInputService`),
  the approach TeamViewer QuickSupport and AirDroid use:
  - `dispatchGesture` for tap, long-press, swipe, drag and pinch
  - `performGlobalAction` for Back, Home, Recents, notifications, quick settings, lock screen
    and screenshot
  - typed text goes to the focused field via `ACTION_SET_TEXT` / `ACTION_PASTE` (a
    clipboard-backed paste for long text); Enter as the IME action
  - the volume keys through AudioManager
- The user turns it on once in Android Settings → Accessibility → Zazabor Link (restricted settings
  are already allowed on their Xiaomi). The app explains what it can do, and stops when the session
  ends.
- **PC side:** the mirror window gets control. Mouse click = tap, drag = swipe, wheel = scroll,
  right-click = Back, middle-click = Home, and the PC keyboard types into the phone. A toolbar has
  Back / Home / Recents / Notifications / Volume / Power-menu / Rotate / Screenshot.
- **Consent on the phone:** each session shows a system-style prompt ("<PC> wants to control this
  phone"). While control is on, a persistent notification and an on-screen border stay visible,
  with a Stop button. "Always allow this PC" is a setting.
- **Limits** (Android rules, not ours):
  - secure screens (banking apps, password fields, the lock screen) show black and can't be
    driven
  - screen capture needs Android's own "Start recording or casting" prompt each session, unless the
    user picks "Entire screen" with "don't ask again" where the OS offers it

## Any network

- The PC already runs `cloudflared` for Meet. A **second public hostname** on the same named tunnel,
  e.g. `link.syfuddhin.com` → `http://localhost:47214`, serves a WebSocket endpoint
  (`/ws/link`). It carries the ZLNK protocol unchanged inside WebSocket binary messages, plus
  the remote video/input channels. The quick tunnel has no fixed address, so this needs the user's
  own domain. (The user has syfuddhin.com on Cloudflare, docs/plans/meet.md.)
- The phone tries the LAN first (mDNS / saved address). Away from home it dials
  `wss://link.syfuddhin.com/ws/link`. The PC tells the phone this address while paired, over a
  sealed message, so nothing needs typing.
- **End-to-end encryption:** the ZLNK handshake and sealing (per-phone link key, ChaCha20-Poly1305)
  run inside the WebSocket, so Cloudflare only ever sees ciphertext. An unpaired client gets
  nothing.
- Hardening:
  - rate-limit handshakes per IP (from `Cf-Connecting-Ip`)
  - an optional Cloudflare Access service token as a second lock
  - a "Remote over the internet" switch, off by default
- **Bandwidth:** adaptive bitrate (1–6 Mbps over the internet), 720p default on mobile data, and
  pause video when the screen is off.
- **Latency:** typically 80–200 ms through Cloudflare. Fine for control, not for gaming.
- All of Phone Link (texts, notifications, files, photos, calls through the app) works over this
  path too, not only Remote.

## Phases

0. **Spike (one day), done 2026-11-22 (see "Spike results"):**
   - Accessibility `dispatchGesture` on the emulator
   - ZLNK over WebSocket through a quick tunnel
   - capture + encode + decode latency, on the emulator and the user's phone, only with the user present
   - XTest injection on the nested display
   - CPU cost at 1080p30 with VA-API vs x264
1. **Core, done 2026-11-22 (see "Status"):**
   - Phone → PC: Screen, Touchpad and Keyboard
   - PC → phone: control in the mirror window
   - consent prompts, indicators and kill switches on both sides
   - LAN
2. **Any network:** `link.<domain>` WebSocket path, end-to-end sealed, for Phone Link and Remote.
3. **Media, Presenter, Power, monitor picker, adaptive bitrate, quality settings.**
4. **Later:** gamepad (uinput rule), Wayland (portal), and a relay for users without their own domain.

## Status (2026-11-22)

Phases 0 and 1 are built, LAN only. The user's own phone is **untested**: everything below ran on the
Android emulator against the real app on a nested display.

- **PC, `src-tauri/src/remote/`:** `proto.rs` (ZREM/1, input events, the one-shot dial; wire format
  in docs/plans/phone-link.md §1b), `capture.rs` (ximagesrc → x264/VA-API → appsink), `inject.rs`
  (XTest through `x11-dl`, which Zazabor already had: pointer, buttons, wheel, keys, Unicode text),
  `media.rs` (MPRIS on the session bus, volume through wpctl/pactl), `killswitch.rs` (Ctrl+Alt+Esc
  grabbed while a session runs), `ui.rs` (the question and the pill, one small window per monitor),
  `store.rs` (Settings → Phone → Remote control), `mod.rs` (sessions, `remote.start`, the commands).
- **Phone, `mobile-cast/` (Zazabor Link 0.5.1):** `RemoteActivity` (Screen, Touchpad, Keyboard,
  Media), `RemoteClient`, `RemoteDecoder` (MediaCodec, low latency, into a SurfaceView),
  `RemoteGestures` (the touch → mouse engine and the zoom/pan viewport), `RemoteProto`; and for PC →
  phone `RemoteInputService` (the accessibility service), `RemoteControl` (the `ctl` actions).
- **PC → phone:** the mirror window gets a "Control this phone" button (`src/modules/phone/control.ts`):
  click = tap, drag = swipe, long press, wheel = scroll, right-click = Back, middle-click = Home, typing
  goes into the focused field, and a toolbar has Back/Home/Recents/Notifications/Quick settings/Volume/
  Screenshot/Lock/Power menu.
- **Not built yet:** Presenter and Power modes (phase 3), the monitor picker's preview, adaptive bitrate
  from measured loss (the phone can already send `quality` and `bitrate`; nothing chooses them
  automatically), the cursor drawn by the phone (the PC's cursor is in the picture), Wayland, and the
  any-network path.

### Decisions made while building

- **x264 is the default encoder, VA-API the option.** On this TigerLake iGPU `vaapih264enc` only
  works with constant QP (`rate-control=cbr|vbr|icq|qvbr` all fail to negotiate), so its bitrate is
  whatever the picture costs and can't follow a network. x264 `zerolatency` holds a bitrate, which the
  any-network phase needs.
- **A separate channel, not frames on the ZLNK socket.** Video must not queue behind a file transfer
  or a ping, and a stalled phone must not stall the link. The token is single use and the keys are
  derived from the link key with their own label, so a remote session never reuses a link key.
- **Consent is a window of our own,** not the Alerts queue: it needs to be on every monitor, stay
  until answered, and become the "Controlled from…" pill, which the Alerts window's cards don't do.
- **Sticky scratch keycodes for text.** A key that isn't on the keyboard (é, ক, 😀) is typed through
  a spare keycode; clearing it the moment the key is released makes applications that read the map
  late see no symbol (the spike below caught it), so eight spare keycodes take turns and are cleared
  a second after the last use.
- **Text into the phone is `ACTION_SET_TEXT`** at the cursor of the focused field, so it works in
  the apps that allow it and not in ones that don't (password fields, some games).

## What the any-network phase needs

- The `link.<domain>` hostname on the existing named tunnel, and a `/ws/link` WebSocket endpoint in
  the PC's HTTP server that speaks ZLNK unchanged inside binary messages. Add `remote.start` to the
  same socket: over WebSocket there is no second port, so ZREM frames must ride the same WebSocket as
  a multiplexed kind (`10`, `20` are already free of ZLNK's kinds `1` and `2`).
- The ZREM dial (`token`, `nonce`, `ready`) stays as it is; only the transport under it changes, so
  `proto.rs` takes `Read + Write` instead of a `TcpStream` (it already does for the frame functions).
- Adaptive bitrate: the phone should measure its receive rate and the ping and send `quality`
  (already handled by the PC), because over the internet 1–6 Mbps is the range and VA-API can't
  hold it (use x264).
- Per-IP handshake rate limit from `Cf-Connecting-Ip`, the "Remote over the internet" switch, and
  a pill that says which network the session runs on.
- PC → phone over the internet needs ZMIR (mirroring) on the same WebSocket too: today it dials the
  PC's LAN address.
- The phone's Link needs the PC's public address while paired (sealed message), and a fallback order
  LAN → WSS.

## Spike results

Measured on 2026-11-22 on the user's PC (Intel TigerLake iGPU, 12 cores, shared with other agents,
so the load average was 10 to 30 during the runs) with the Android emulator (Pixel 7, API 35, 2
cores, software GL) as the phone. The user's own phone was not used. Treat every phone-side number
as a bound, not a result.

**(a) Capture, encode, send, decode**

- **Encoders.** `vaapih264enc` works on this iGPU in constant-QP mode only: `rate-control=cbr`,
  `vbr`, `qvbr` and `icq` all fail to negotiate (`not-negotiated`). x264 `zerolatency` holds a
  bitrate. So x264 is the default and VA-API an option (Settings → Phone → Remote control).
- **CPU of the pipeline alone** (real `ximagesrc` of a 1920×1080 display at 30 fps, scaled, a mostly
  static desktop, percent of one core): x264 ultrafast 26 % at 1280×720, 41 % at 1600×900, 40 % at
  1920×1080; VA-API 14 % at 1280×720 and 21 % at 1920×1080. Test clips (`videotestsrc`) cost
  23 % (720p30) and 47 % (1080p30) with x264, 13 % and 32 % with VA-API. In the running app, the
  whole Zazabor process (UI, capture, encoding, sealing, sending) used about 65 % of a core at 720p,
  85 % at 900p and 84 % at 1080p with a window open on the screen.
- **PC-side latency** (GStreamer's own `latency` tracer, `ximagesrc` → encoded access unit):
  mean 4.3 ms at 720p, 7.3 ms at 900p, 8.8 ms at 1080p (worst frame 15, 23 and 84 ms).
- **On the emulator:** 20 to 30 fps at 900p, 24 fps at 720p, 17 fps at 1080p (the emulator's
  software GL and decoder are the limit, not the PC), 1.1 to 3.2 Mbps for a mostly still desktop.
  The decoder's own time from `queueInputBuffer` to the screen was **12 ms** (the stats line shows it;
  it needs no clock agreement with the PC). The ping through the sealed channel went from 10 to 250
  ms with the host's load.
- **Glass to glass** could not be measured honestly here. The method was an on-screen millisecond
  clock on the PC (in the picture) and one on the phone, read from a single `adb screencap`. The
  emulator's clock disagreed with the host's by −159 ms and later by −510 ms (it drifts), and its
  compositor queues frames, so the readings came out between 0.3 s and 4.5 s: useless. The estimate
  for a LAN instead is the sum of its parts: about 17 ms waiting for the next capture at 30 fps, 7 ms
  to encode, 2 to 5 ms on Wi-Fi, 12 ms to decode and 16 to 33 ms for the display, so roughly
  **55 to 75 ms**, inside the plan's 60 to 120 ms. **It must be measured on the real phone with the
  user present** (the Remote screen has the tool: tap the stats line for a live clock over the
  picture, put a millisecond clock on the PC's screen, and read both in one photograph of the phone).

**(b) X11 XTest injection** (ignored test `remote::tests::xtest_reaches_a_window`, which opens a
window on `ZREM_TEST_DISPLAY` and reads what XTest delivers; also seen live with `xev`): the pointer
lands on the monitor position, left and right buttons, wheel down and right (buttons 5 and 7),
Return, `aB` with Shift, `é`, `€`, two Bengali letters and an emoji all arrive. One finding: the
first version cleared the scratch keycode right after each use, and the window then read `NoSymbol`
for the characters that aren't on the keyboard. Eight scratch keycodes that keep their keysym for a
second fixed it, and the test now pins the keysyms.

**(c) Accessibility on the emulator** (`settings put secure enabled_accessibility_services …`, the
emulator only): `dispatchGesture` taps, swipes and long drags worked from the mirror window (a click
opened Photos, a drag opened the app drawer), Back (right-click) and Home (toolbar) worked, and the
red border shows (it first failed silently: `addView` from a background thread). Not exercised: text
into a field, scroll, long press, Recents, notifications, quick settings, lock, screenshot, power
menu and volume. The consent notification posts correctly (`dumpsys notification` shows its title,
text and three actions), but the emulator's shade showed only the collapsed public version, so its
buttons couldn't be tapped; for the control test "Always allow this PC" was written into the app's
preferences. Android turns an accessibility service off when its app is force-stopped or replaced,
so it has to be switched on again after each `adb install`.

**Not spiked:** ZLNK over a WebSocket through a quick tunnel (the any-network phase).

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

0. **Spike (one day):**
   - Accessibility `dispatchGesture` on the emulator
   - ZLNK over WebSocket through a quick tunnel
   - capture + encode + decode latency, on the emulator and the user's phone, only with the user present
   - XTest injection on the nested display
   - CPU cost at 1080p30 with VA-API vs x264
1. **Core:**
   - Phone → PC: Screen, Touchpad and Keyboard
   - PC → phone: control in the mirror window
   - consent prompts, indicators and kill switches on both sides
   - LAN
2. **Any network:** `link.<domain>` WebSocket path, end-to-end sealed, for Phone Link and Remote.
3. **Media, Presenter, Power, monitor picker, adaptive bitrate, quality settings.**
4. **Later:** gamepad (uinput rule), Wayland (portal), and a relay for users without their own domain.

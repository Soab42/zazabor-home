# Messages: every chat in one place

A new **Messages** section puts the user's chat services in one window, like Ferdium, Rambox
or Beeper: WhatsApp, Messenger, Microsoft Teams, Telegram and more. Each account runs the
service's **official web app** inside Zazabor with its own login. The phone's texts from Phone
Link sit in the same list.

User decisions (2026-11-22):

- One place for "Messenger, WhatsApp, Microsoft Teams" and other services.
- WhatsApp and Messenger offer no API for personal chats (only WhatsApp Business / Messenger for
  Pages), and unofficial reverse-engineered libraries break their terms and get accounts banned.
  So each service runs its official web app. Nothing is scraped or re-implemented.

## What the user gets

- **A sidebar of accounts:** service icon, name, unread badge, and a mute dot. Drag to reorder.
  More than one account per service, e.g. two WhatsApp numbers or work and personal Teams.
- **Add account:** pick a service (or "Custom web app" with any URL), name it, sign in inside
  the panel. WhatsApp shows its QR code to scan with the phone, as on the web.
- **One unread total** on the Messages rail icon. Desktop notifications per account, with the
  sender, a preview and the account name; clicking one opens that account and chat. Per-account
  mute, do-not-disturb (with a schedule), and "show previews" on or off.
- **Phone texts** (Phone Link) as a built-in entry at the top: the existing Messages view of the
  Phone section, embedded.
- **Calls:** voice and video calls can't run inside Zazabor's web view (WebKitGTK has no WebRTC
  plugins here; docs/plans/meet.md). Zazabor catches a call's new window and opens it in the
  system browser, with a hint to sign in there once. Everything else (text, voice notes, photos,
  files, reactions) works in the panel.
- **Per account:** reload, zoom, "open in browser", hibernate (unload until opened; saves
  RAM), sign out and forget (deletes its data folder).
- **Files:** downloads go to ~/Downloads (asked first for executables). Uploads use the system
  file picker. Links open in the default browser, except the service's own pages.

## Services (first set)

| Service | URL | Notes |
| --- | --- | --- |
| WhatsApp | https://web.whatsapp.com | QR login; needs a recent Safari UA or it shows "update browser" |
| Messenger | https://www.messenger.com | Facebook login |
| Microsoft Teams | https://teams.microsoft.com (work/school), https://teams.live.com (personal) | Teams v2 supports Safari; picker asks which account type |
| Telegram | https://web.telegram.org/a/ | |
| Instagram DMs | https://www.instagram.com/direct/inbox/ | |
| Slack | https://app.slack.com/client | |
| Discord | https://discord.com/app | |
| Google Chat | https://chat.google.com | |
| Google Messages | https://messages.google.com/web | QR pairing with the phone's Google Messages |
| LinkedIn | https://www.linkedin.com/messaging/ | |
| Custom | any https URL | |

Each catalogue entry names: URL, URL patterns that stay inside the panel (everything else opens
in the browser), the user agent to present, how its unread count is read (title pattern or DOM
badge), and the icon (bundled SVG; no fetching logos at run time).

## How it works

### Panels are child web views of the main window

- Each account gets one plain WebKitGTK web view inside the main window (not a Tauri child
  webview: see the spike results), placed exactly over the Messages content area. The page
  reports that rectangle whenever the layout changes (resize, sidebar fold, DPI), and Rust moves
  or resizes the child. Only the shown account's web view is visible. The others are hidden (not
  destroyed) so they keep receiving messages, unless hibernated.
- Leaving the Messages section, or any sheet or menu opening over it, hides the visible child,
  because native web views draw above the page. The section shows a still snapshot under the
  sheet instead.
- **Separate logins:** each panel gets its own `WebsiteDataManager` + `WebContext` rooted at
  `<app data>/messages/<account id>/`, so cookies, local storage and IndexedDB never mix between
  accounts or with Zazabor's own pages.
- **User agent:** a current Safari-on-macOS string for services that check, set per service.

### No Zazabor powers for remote pages

- The remote pages get **no** Tauri IPC: no capability lists their URLs, so
  `window.__TAURI__` and `invoke` don't exist there.
- They talk to Zazabor only through an initialization script that does two things:
  - wraps `window.Notification` / `ServiceWorkerRegistration.showNotification`, and watches
    `document.title` and the service's unread badge;
  - reports by posting a JSON string to WebKit's script message handler `zmsg` (not a cancelled
    navigation: see the spike results). Payloads are size-capped and validated in Rust.
- `on_new_window`: links outside the service open in the default browser; call windows open in
  the system browser; the service's own pop-ups (e.g. Microsoft login) open as a small child
  window that shares the account's data directory.
- Downloads go through WebKit's download signal into ~/Downloads, with a toast.
- Camera, mic and screen permission requests are refused (calls go to the browser). Location is
  refused. Notification permission is answered "granted" by the shim.

### Code

| Part | Where |
| --- | --- |
| Accounts (id, service, name, order, muted, hibernate, zoom) | `src-tauri/src/messages/store.rs` (JSON in the app data dir) |
| Child web views: create, place, show/hide, reload, zoom, destroy + wipe data | `src-tauri/src/messages/panels.rs` |
| Injected script (notification shim, unread watcher, event bridge) | `src-tauri/src/messages/inject.js` (`include_str!`) |
| Event bridge (`zazabor-msg://`), validation, desktop notifications via `desktop_notify` | `src-tauri/src/messages/events.rs` |
| Service catalogue | `src-tauri/src/messages/services.rs` + icons in `src/modules/messages/icons/` |
| Section UI (sidebar, add-account sheet, settings, placeholder/snapshot) | `src/modules/messages/` |
| Rail badge total | `src/shell/registry.ts` status hook (like Phone/Mail) |

Commands: `msg_accounts`, `msg_add {service, name, url?}`, `msg_update`, `msg_remove {id, wipe}`,
`msg_show {id, rect}`, `msg_hide`, `msg_rect {rect}`, `msg_reload {id}`, `msg_zoom {id, factor}`,
`msg_hibernate {id, on}`, `msg_open_external {id}`. Events: `msg-unread {id, count}`,
`msg-notified {id, title}`, `msg-state {id, loading|ready|error}`.

## Risks to check first (phase 0 spike, before building the UI)

1. `add_child` with several children in the WebKitGTK main window: z-order, resizing, hiding,
   focus and keyboard input, under X11 and XWayland.
2. Per-account `data_directory` really isolates sessions on this wry version. Two WhatsApp
   accounts side by side is the test.
3. WhatsApp Web, Messenger and Teams load and log in in WebKitGTK 2.50 with a Safari UA:
   - WhatsApp reaches the QR screen
   - Teams personal reaches sign-in
   - Messenger's login page renders.
4. RAM: each account is a WebKit web process (about 150–400 MB). Hibernate after N idle minutes
   by default for accounts not marked "always on". Measure 4 accounts.
5. Notifications from the service worker path (WhatsApp, Teams) reach the shim.

If (1) fails badly, the fallback is one separate top-level window per account, docked next to the
main window. It costs more, but uses the same isolation and bridge.

## Phases

0. **Spike (half a day):** the five checks above, with a throwaway profile on a nested display.
   Real logins only with the user present, never with the user's own accounts unattended.
1. **Core:** catalogue, add/remove, panels, isolation, unread badges, notifications, hibernate,
   external links and calls, Phone texts entry, rail badge.
2. **Polish:** do-not-disturb schedule, per-account zoom, drag-reorder, quick switcher
   (Ctrl+1…9, Ctrl+K search across accounts), snapshot under sheets, light theme, settings page.
3. **Later (optional):** Matrix bridges for a truly unified inbox (one list of chats across
   services), and the Microsoft Graph API for Teams once the Azure client ID exists
   ([[mail-next]]). Neither is needed for phase 1.

## Spike results (2026-11-22)

Phase 0 was run on a nested Xephyr display (1920x1080, X11), WebKitGTK 2.50 (webkit2gtk-4.1),
Tauri 2.12.4, a throwaway profile, and a driver (`src-tauri/src/messages/spike.rs`, throwaway) that
feeds the real `panels.rs` commands from a control file. Screenshots are in the session scratchpad
`/tmp/claude-1000/-home-soab-working-personal-zazabor-home/26cfe356-9b60-41e7-98e7-627ef3222f28/scratchpad/`
as `msg-*.png` (called `$S2/` below). Nothing was signed into and no QR code was scanned.

### Change to the plan: panels are plain WebKitGTK views, not Tauri child webviews

`Window::add_child` (Tauri `unstable`) does work, but on Linux it only packs the child into the
window's GtkBox beside the main web view, and, worse, once a window has a second webview
`app.get_webview_window("main")` returns None (Tauri's `is_webview_window` needs every webview
to share the window's label; the spike's `resize` failed with "no main window"). The app calls
`get_webview_window("main")` in lib.rs, mail, cast, phonelink, clipboard and notes, so every one of
those would break the moment a panel exists. So `panels.rs` builds the views itself with the
`webkit2gtk` crate (already a dependency): the main web view is moved into a `GtkOverlay` the first
time, and each panel is an overlay child positioned by the page's rectangle. No `unstable` feature.
`add_child` plus the same overlay trick was also tried first and behaved the same visually
(`$S2/msg-01..06-*.png`).

### 1. Several panels in the main window: works

- Three panels placed over a page-reported rectangle, exact to the pixel (`msg-10-raw-three.png`).
- Resize and move through `msg_rect`: works. Main window shrunk 1400x900 to 1000x700: panels are
  clipped by the window and never escape (`msg-13-raw-window-shrunk.png`, `msg-14-raw-after-shrink.png`).
  The page has to resend the rectangle (a ResizeObserver does it); nothing follows automatically.
- Hide and show: works (`msg-04`/`msg-13`). Z-order: panels draw above the main page, as expected.
- Keyboard focus and typing in a panel (click, then xdotool typing), mouse-wheel scrolling and the
  WebKit right-click menu (Back, Forward, Reload, Inspect Element in debug builds) all work
  (`msg-02-typing-scroll.png`, `msg-03-rightclick.png`; the raw-view run printed the typed text
  back through the bridge: "hello A", "bee B").
- The main window's own UI keeps working around the panels: clicking the Files and Photos rail
  icons while panels were up navigated the main page (`msg-04`, `msg-12`/`msg-13`).
- Caveat: a pop-up menu from a panel (right-click) is a separate X window, fine. Panels are native
  widgets, so anything of the page's own (menus, sheets, toasts) over the Messages area is hidden
  behind them, as the plan already says (hide the panel or show a snapshot).
- Fallback (one top-level window per account) was not needed.

### 2. Session isolation: works

- Two panels of the same origin (127.0.0.1:8377) with different data directories each kept their
  own cookie and localStorage value, also after a reload (log lines `STATE cookie=ck=ck-set-by-A`
  vs `...-by-B`; `msg-01`, `msg-10`).
- Two `web.whatsapp.com` panels show two different QR codes with different IndexedDB sets
  (`msg-42-two-whatsapp.png`): separate sessions.
- Cookies persist on disk per account (`cookies.sqlite`, `localstorage`, `databases`,
  `serviceworkers`, `WebKitCache` under `messages/<id>/`).

### 3. Service compatibility in WebKitGTK 2.50: works (with one trap)

| Service | Result (both with the Safari 26.1 macOS UA, and with WebKitGTK's own UA) |
| --- | --- |
| web.whatsapp.com | QR screen, no "update browser" (`msg-20-wa-safari.png`, `msg-20-wa-plain.png`). WebKitGTK already presents a macOS Safari UA to WhatsApp through its built-in site quirks, so the override is not needed. |
| teams.live.com | Landing page, then the Microsoft sign-in page after one click on "Sign in" (`msg-30`, `msg-31-teamslive-signin.png`); plain UA identical (`msg-35-sheet-plain.png`) |
| teams.microsoft.com | Redirects to login.microsoftonline.com sign-in (`msg-33-sheet.png`, `msg-35`) |
| www.messenger.com | Login page renders (`msg-33`, `msg-35`) |
| web.telegram.org/a | QR login screen (`msg-33`) |
| app.slack.com | "Sign in to your workspace" (`msg-33`) |
| discord.com/login | Login and QR (`msg-33`) |
| messages.google.com/web | Welcome page with Sign in (`msg-33`) |

All eight reach their login or QR screen with either UA, so the user agent can stay the default
and a per-service override remains just a catalogue field. The trap is the bridge, below.

### 4. RAM and CPU: works with caveats

Process trees measured from /proc (RSS and PSS summed over the app's WebKit processes):

| State | RSS | PSS | CPU, idle |
| --- | --- | --- | --- |
| App alone (main web view only) | 542 MB | 305 MB | 0.2% of a core |
| + 4 panels (WhatsApp, Messenger, Teams, Telegram at login screens), all visible | 2447 MB | 1328 MB | 2.1% |
| same, 3 of them hidden (plus two test pages) | 2880 MB | 1434 MB | 3.5% (test pages tick) |

So about 475 MB RSS (255 MB PSS) per account: roughly 375 MB web process + 75 MB network
process, because every data directory gets its own WebKitNetworkProcess (5 of them for 4
accounts). Hiding does not free anything; only destroying the view does (hibernate). After
destroying everything the process count and RSS returned to the 548 MB baseline: the processes
exit and no leftovers remain.

Hidden panels keep running JS, throttled: a 1 s timer in a hidden panel ran about 0.6 times a second
(visibility "hidden"), full speed when merely parked off-screen (visibility "visible"). So network
connections, timers and in-page notifications keep working while hidden, with up to roughly a second
or two of delay, which is fine for chat. Parking off-screen is the alternative if a service
misbehaves hidden, at full CPU cost.

Plan change: hibernate (destroy the view, keep the data folder, reload on open) is the only
RAM saver; the default "hibernate after N idle minutes" stays, and the catalogue should offer
"always on" for the accounts the user wants notifications from.

### 5. Notifications path: works, with a transport change

- `new Notification(...)`, `registration.showNotification(...)` and a `document.title = "(3) ..."`
  change were each delivered to Rust (log lines `notification via page`, `via sw`, `title ... ->
  unread 3`), and the first two reached the desktop notification service
  (org.freedesktop.Notifications.Notify with the title "Ada (A) - acca", the body, a default
  action and category im.received; seen with dbus-monitor on a private session bus).
- Payloads are capped in the page (title 200, body 500, tag 100, whole report 6000) and again in
  Rust (`events.rs`: 8 KB, control characters dropped, unknown types refused, 12 notifications per
  minute per account), with unit tests (`cargo test --offline --lib messages::`: 5 pass).
- Remote pages have no Tauri access: `typeof window.__TAURI__` and `__TAURI_INTERNALS__` are both
  `undefined` in a panel, and a call to `__TAURI_INTERNALS__.invoke` throws. (The panels are not
  Tauri webviews, so no capability applies.)
- Limit: a notification raised inside the service worker itself (a push event) can't be hooked,
  as an init script doesn't run in workers; WhatsApp, Telegram, Messenger and Teams raise theirs from
  the page (`new Notification` or `registration.showNotification`), which are covered.
- Transport: the planned cancelled navigation to `zazabor-msg://event?...` was built and tested.
  It breaks pages that are still loading: Teams stayed blank (no `<body>`) whenever the title
  watcher fired during load (bisected: bridge alone fine, title shim off fine, WebKitGTK's own
  MiniBrowser fine, `msg-20-tl-safari.png` vs `msg-25-tl-noinject.png`). A hidden iframe report never
  arrived from Teams. The shipped transport is WebKit's script message handler `zmsg`
  (`window.webkit.messageHandlers.zmsg.postMessage(json)`), read in Rust with
  `connect_script_message_received`; with it Teams renders (`msg-30-teamslive-safari.png`). It
  needs the `javascriptcore-rs` crate as a direct dependency (already in the lock file).

### Panels destroyed and data wiped: works

`msg_destroy {wipe: true}` removed the view and the whole `messages/<id>/` folder (up to 53 MB for
WhatsApp's login page); without `wipe` the folder stays. The WebKit processes for it exit.

### Left for phase 1 (needs the user present or more code)

- Real logins: WhatsApp pairing, Messenger, Teams (work and personal), Telegram; whether sessions
  survive an app restart, and whether the services' notifications arrive as the shim expects
  (the sender and preview text in particular, which differs per service).
- Calls: no WebRTC was tested here; new-window requests are routed to the system browser in `panels.rs` but that
  path was not exercised; the service's own pop-ups (Microsoft login) still need the
  "stay inside" handling.
- Downloads and uploads (file chooser), zoom, and the page that reports the rectangle
  (ResizeObserver, hide on sheets). `msg_*` commands exist and are registered but nothing calls them
  yet; `spike.rs` is a throwaway driver (control file in `ZAZABOR_MSG_CTL`) to delete once the page
  drives them. `ZAZABOR_MSG_OFF` in `panels.rs` and the `dbg` report type are spike aids as well.


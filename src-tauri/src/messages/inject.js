// Injected into every Messages panel (a remote chat web app) before its own scripts run.
// It is the panel's only line to Zazabor: the page gets no Tauri IPC. Each report is a small JSON
// string posted to WebKit's script message handler "zmsg"; Rust (messages/events.rs) caps and
// validates it. (A navigation to a zazabor-msg:// address, which on_navigation cancels, was tried
// first: it works on quiet pages but stops a page that is still loading, as Teams' is, and an
// iframe report never arrived from Teams.)
(() => {
  if (window.__zmsg) return;
  const MAX = 6000;
  const send = (o) => {
    let s;
    try { s = JSON.stringify(o); } catch (_) { return; }
    if (s.length > MAX) return;
    try { window.webkit.messageHandlers.zmsg.postMessage(s); } catch (_) {}
  };
  window.__zmsg = { send };

  // The colour scheme the account asked for (Rust sets __zmsgScheme before this script). Zazabor's
  // web engine reports the desktop's dark theme to every page, so WhatsApp's "System default" and
  // Teams' "Follow OS theme" went dark; "light" and "dark" answer their checks the account's way.
  const scheme = window.__zmsgScheme;
  if (scheme === "light" || scheme === "dark") {
    const dark = scheme === "dark";
    const realMatch = window.matchMedia.bind(window);
    const fixed = (query, matches) => {
      const listeners = new Set();
      return {
        matches, media: query, onchange: null,
        addListener(f) { listeners.add(f); }, removeListener(f) { listeners.delete(f); },
        addEventListener(_t, f) { listeners.add(f); }, removeEventListener(_t, f) { listeners.delete(f); },
        dispatchEvent() { return false; },
      };
    };
    window.matchMedia = function (query) {
      const q = String(query);
      if (/prefers-color-scheme\s*:\s*dark/i.test(q)) return fixed(q, dark);
      if (/prefers-color-scheme\s*:\s*light/i.test(q)) return fixed(q, !dark);
      return realMatch(query);
    };
    // Built-in controls (scrollbars, inputs) and pages that only read the CSS property.
    const setScheme = () => { try { document.documentElement.style.colorScheme = scheme; } catch (_) {} };
    if (document.documentElement) setScheme(); else document.addEventListener("readystatechange", setScheme, { once: true });
  }

  const str = (v, n) => (v == null ? "" : String(v).slice(0, n));

  // Page notifications: `new Notification(...)`. Permission always reads "granted".
  class ZNotification extends EventTarget {
    constructor(title, opts) {
      super();
      opts = opts || {};
      this.title = str(title, 200);
      this.body = str(opts.body, 500);
      this.tag = str(opts.tag, 100);
      this.onclick = this.onclose = this.onerror = this.onshow = null;
      send({ t: "notify", via: "page", title: this.title, body: this.body, tag: this.tag });
    }
    close() { send({ t: "notify-close", title: this.title, tag: this.tag }); }
    static get permission() { return "granted"; }
    static requestPermission(cb) { if (cb) cb("granted"); return Promise.resolve("granted"); }
    static get maxActions() { return 0; }
  }
  try { Object.defineProperty(window, "Notification", { value: ZNotification, configurable: true, writable: true }); } catch (_) {}

  // Notifications through the service worker registration.
  try {
    if (window.ServiceWorkerRegistration) {
      ServiceWorkerRegistration.prototype.showNotification = function (title, opts) {
        opts = opts || {};
        send({ t: "notify", via: "sw", title: str(title, 200), body: str(opts.body, 500), tag: str(opts.tag, 100) });
        return Promise.resolve();
      };
      ServiceWorkerRegistration.prototype.getNotifications = () => Promise.resolve([]);
    }
  } catch (_) {}

  // The unread count the services put in the tab title: "(3) WhatsApp".
  let lastTitle = null;
  let timer = 0;
  const reportTitle = () => {
    timer = 0;
    if (document.title !== lastTitle) {
      lastTitle = document.title;
      send({ t: "title", title: str(lastTitle, 200) });
    }
  };
  const watchTitle = () => {
    const el = document.querySelector("title");
    if (!el) return false;
    new MutationObserver(() => { if (!timer) timer = setTimeout(reportTitle, 300); })
      .observe(el, { childList: true, characterData: true, subtree: true });
    reportTitle();
    return true;
  };
  // Shortcuts: keys typed in a panel never reach Zazabor's own page, so the few it owns are
  // forwarded. Ctrl+1..9 switch account, Ctrl+Shift+[ / ] previous / next, Ctrl+R reload,
  // Ctrl+Shift+K the quick switcher (plain Ctrl+K stays the service's own search).
  window.addEventListener("keydown", (e) => {
    if (!e.ctrlKey || e.altKey || e.metaKey) return;
    let key = null;
    if (!e.shiftKey && /^[1-9]$/.test(e.key)) key = e.key;
    else if (e.shiftKey && (e.code === "BracketLeft" || e.key === "{")) key = "prev";
    else if (e.shiftKey && (e.code === "BracketRight" || e.key === "}")) key = "next";
    else if (!e.shiftKey && (e.key === "r" || e.key === "R")) key = "reload";
    else if (e.shiftKey && (e.key === "k" || e.key === "K")) key = "switcher";
    if (!key) return;
    e.preventDefault();
    e.stopPropagation();
    send({ t: "key", key });
  }, true);
  // Incoming calls: the page's own call dialog (a notification may not come at all, or may stay
  // after the call ended). One table per service, then a generic fallback: a dialog whose text says
  // "incoming ... call" / "is calling you" and that holds a Decline-like button. Conservative: an
  // element must be a dialog (role or test id), short, and have such a button. The Rust side
  // (alerts/calls.rs) has the matching table for notification text.
  const CALL_DOM = {
    whatsapp: { dialogs: ['[data-testid*="call"]', '[data-animate-modal-popup="true"]'] },
    teams: { dialogs: ['[data-tid*="call-toast"]', '[data-tid*="incoming"]', '[data-tid*="calling"]'] },
    messenger: { dialogs: ['[aria-label*="ncoming"]', '[aria-label*="alling"]'] },
  };
  const GENERIC_DIALOGS = ['[role="dialog"]', '[role="alertdialog"]', '[role="alert"]'];
  const RING_TEXT = /(incoming (voice |video |audio |group )*(call|chat))|(is (video )?calling you)/i;
  const DECLINE = /^(decline|reject|dismiss|ignore|not now)\b/i;
  const ANSWER = /^(accept|answer|join|pick up)\b/i;
  const labelOf = (el) => (el.getAttribute("aria-label") || el.getAttribute("title") || el.textContent || "").trim();
  const service = () => {
    const h = location.hostname;
    if (/whatsapp\.com$/.test(h)) return "whatsapp";
    if (/(teams\.microsoft|teams\.live)\.com$/.test(h)) return "teams";
    if (/(messenger|facebook)\.com$/.test(h)) return "messenger";
    if (/discord\.com$/.test(h)) return "discord";
    if (/slack\.com$/.test(h)) return "slack";
    if (/chat\.google\.com$|mail\.google\.com$/.test(h)) return "googlechat";
    return "";
  };
  const findCall = () => {
    const sels = (CALL_DOM[service()] ? CALL_DOM[service()].dialogs : []).concat(GENERIC_DIALOGS);
    for (const sel of sels) {
      let nodes;
      try { nodes = document.querySelectorAll(sel); } catch (_) { continue; }
      for (const el of nodes) {
        const text = (el.innerText || el.textContent || "").trim();
        if (!text || text.length > 400 || !RING_TEXT.test(text)) continue;
        const buttons = Array.from(el.querySelectorAll('button,[role="button"]'));
        const decline = buttons.find((b) => DECLINE.test(labelOf(b)));
        if (!decline) continue;
        const answer = buttons.find((b) => ANSWER.test(labelOf(b)));
        const line = text.split(/\n+/).map((l) => l.trim()).find((l) => l && !RING_TEXT.test(l) && !DECLINE.test(l) && !ANSWER.test(l)) || "";
        return { name: line.slice(0, 80), video: /video/i.test(text), decline, answer };
      }
    }
    return null;
  };
  let ringing = false;
  let callTimer = 0;
  const checkCall = () => {
    callTimer = 0;
    const found = findCall();
    if (found && !ringing) { ringing = true; send({ t: "call", state: "ringing", name: found.name, video: found.video }); }
    else if (!found && ringing) { ringing = false; send({ t: "call", state: "ended" }); }
  };
  // The alerts' Decline / Answer: click the dialog's own button (best effort).
  window.__zmsg.callAction = (what) => {
    const found = findCall();
    const button = found && (what === "answer" ? found.answer : found.decline);
    if (button) button.click();
    return !!button;
  };
  const watchCalls = () => {
    new MutationObserver(() => { if (!callTimer) callTimer = setTimeout(checkCall, 400); })
      .observe(document.documentElement, { childList: true, subtree: true });
  };
  if (document.documentElement) watchCalls();
  else document.addEventListener("DOMContentLoaded", watchCalls, { once: true });
  // Outgoing calls: this web engine has no WebRTC, so a service's own call button ends in "your
  // browser doesn't support calling". Two signals tell Zazabor to offer the account's browser
  // window: a click on the service's call button, and the service's unsupported-browser notice
  // showing up. One table per service (conservative: exact labels, not substrings of chat text).
  const CALL_BUTTONS = {
    whatsapp: { icons: ["video-call", "voice-call", "audio-call", "call", "videocall", "voicecall"], labels: /^(video|voice|audio) call$|^call$/i },
    // Teams: the call buttons, and the meeting pre-join screen ("Join now").
    teams: { tids: /^(call-video|call-audio|calling-.*(video|audio)|(video|audio)-call.*|.*call-button|prejoin-join-button|.*prejoin.*join.*)$/i, labels: /^(video|audio|voice) call$|^call$|^(start|make) (a )?(video |audio |voice )?call$|^join now$/i },
    // Discord: starting a call in a DM, joining a voice channel, turning the camera on.
    discord: { labels: /^(start (a )?(voice|video) call|join voice|join video|start video call|turn on camera|share your screen)$/i },
    messenger: { labels: /^(start|make) (a )?(video|voice|audio) call$|^(video|voice|audio) call$/i },
    slack: { labels: /^(start|join) (a )?huddle$|^huddle$|^(start|make) (a )?call$/i },
    googlechat: { labels: /^(start|make) (a )?(video |audio |voice )?(call|meeting)$|^(video|audio|voice) call$/i },
  };
  const NOTICE = /(browser (doesn'?t|does not|isn'?t|is not|can'?t|cannot) (support|make|place|start))|(update (your|the) browser)|(calls? (aren'?t|are not|isn'?t|is not|can'?t be|cannot be) (supported|made|available))|(not supported (in|on|by) (this|your) browser)|(your browser (is not|isn'?t) supported)|(use (a )?(supported|different|another) browser)|(calling (isn'?t|is not) (supported|available))|(video (calling )?(isn'?t|is not|not) supported)|(not supported.{0,40}video)|(video.{0,40}not supported)|(webrtc)/i;
  const NOTICE_HOLDERS = '[role="dialog"],[role="alertdialog"],[role="alert"],[role="status"],[data-animate-modal-popup="true"],[data-testid*="popup"],[data-tid*="toast"],[data-tid*="notification"]';
  let needsSent = 0;
  const needsBrowser = (via) => {
    const now = Date.now();
    if (now - needsSent < 10000) return;
    needsSent = now;
    send({ t: "call-needs-browser", via });
  };
  const isCallButton = (el) => {
    const table = CALL_BUTTONS[service()];
    if (!table) return false;
    for (let n = el, depth = 0; n && n.nodeType === 1 && depth < 6; n = n.parentElement, depth++) {
      const label = (n.getAttribute("aria-label") || n.getAttribute("title") || "").trim();
      if (label && table.labels && table.labels.test(label)) return true;
      const tid = n.getAttribute("data-tid") || "";
      if (tid && table.tids && table.tids.test(tid)) return true;
      const icon = n.getAttribute("data-icon") || (n.querySelector && n.querySelector("[data-icon]") ? n.querySelector("[data-icon]").getAttribute("data-icon") : "");
      if (icon && table.icons && table.icons.includes(icon)) return true;
    }
    return false;
  };
  window.addEventListener("click", (e) => {
    try { if (e.isTrusted && isCallButton(e.target)) needsBrowser("click"); } catch (_) {}
  }, true);
  let noticeTimer = 0;
  const checkNotice = () => {
    noticeTimer = 0;
    let nodes;
    try { nodes = document.querySelectorAll(NOTICE_HOLDERS); } catch (_) { return; }
    for (const el of nodes) {
      const text = (el.innerText || el.textContent || "").trim();
      if (text && text.length <= 400 && /call|video|meeting|voice/i.test(text) && NOTICE.test(text)) { needsBrowser("notice"); return; }
    }
  };
  const watchNotices = () => {
    new MutationObserver(() => { if (!noticeTimer) noticeTimer = setTimeout(checkNotice, 800); })
      .observe(document.documentElement, { childList: true, subtree: true });
  };
  if (document.documentElement) watchNotices();
  else document.addEventListener("DOMContentLoaded", watchNotices, { once: true });
  if (!watchTitle()) {
    new MutationObserver((_, ob) => { if (watchTitle()) ob.disconnect(); })
      .observe(document.documentElement || document, { childList: true, subtree: true });
  }
})();

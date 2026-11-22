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
  if (!watchTitle()) {
    new MutationObserver((_, ob) => { if (watchTitle()) ob.disconnect(); })
      .observe(document.documentElement || document, { childList: true, subtree: true });
  }
})();

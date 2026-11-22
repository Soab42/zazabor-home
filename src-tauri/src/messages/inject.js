// Injected into every Messages panel (a remote chat web app) before its own scripts run.
// It is the panel's only line to Zazabor: the page gets no Tauri IPC. Each report is a small JSON
// string posted to WebKit's script message handler "zmsg"; Rust (messages/events.rs) caps and
// validates it. (A navigation to a zazabor-msg:// address, which on_navigation cancels, was tried
// first: it works on quiet pages but stops a page that is still loading, as Teams' is, and an
// iframe report never arrived from Teams.)
(() => {
  if (window.__zmsg) return;
  const MAX = 6000;
  // Spike switch: services whose shims to leave out ("notify,sw,title").
  const off = "__OFF__";
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
    close() {}
    static get permission() { return "granted"; }
    static requestPermission(cb) { if (cb) cb("granted"); return Promise.resolve("granted"); }
    static get maxActions() { return 0; }
  }
  if (!off.includes("notify")) try { Object.defineProperty(window, "Notification", { value: ZNotification, configurable: true, writable: true }); } catch (_) {}

  // Notifications through the service worker registration.
  try {
    if (window.ServiceWorkerRegistration && !off.includes("sw")) {
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
  // Spike diagnostics: script errors in the page (Rust drops these unless ZAZABOR_MSG_SPIKE is set).
  window.addEventListener("error", (e) => send({ t: "dbg", msg: "page error: " + str(e.message, 150) + " @" + str(e.filename, 60) + ":" + e.lineno }));
  window.addEventListener("unhandledrejection", (e) => send({ t: "dbg", msg: "rejection: " + str(e.reason && (e.reason.stack || e.reason), 200) }));
  if (off.includes("title")) return;
  if (!watchTitle()) {
    new MutationObserver((_, ob) => { if (watchTitle()) ob.disconnect(); })
      .observe(document.documentElement || document, { childList: true, subtree: true });
  }
})();

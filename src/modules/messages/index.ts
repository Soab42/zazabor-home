import type { AppModule, ModuleFactory } from "../../shell/module";
import { isMainWindow } from "../../shell/windows";
import { h } from "../../ui/dom";
import { icon } from "../../ui/icons";
import { openMenu, type MenuItem } from "../../ui/menu";
import "../files/ops/ops.css";
import { nameSheet } from "../files/ops/sheets";
import { showToast } from "../files/ops/toast";
import { errorText, msg, onMsg, type Account, type Rect } from "./api";
import { serviceIcon, serviceInfo, SERVICES } from "./catalogue";
import { onOpenRequest, takePendingOpen } from "./global";
import "./messages.css";
import { addAccountSheet, quickSwitcher, removeAccountSheet } from "./sheets";
import { accounts, account, refresh, settings, startStore, subscribe } from "./store";

const PHONE = "phone";
const SELECTED_KEY = "zazabor.messages.selected";
const FOLDED_KEY = "zazabor.messages.folded";
const ZOOM_STEP = 0.1;

const remembered = (key: string): string | null => {
  try { return localStorage.getItem(key); } catch { return null; }
};
const remember = (key: string, value: string) => {
  try { localStorage.setItem(key, value); } catch { /* private mode */ }
};

const sameRect = (a: Rect | null, b: Rect | null) => !!a && !!b && a.x === b.x && a.y === b.y && a.w === b.w && a.h === b.h;

/**
 * Messages: the chat services as their official web apps (docs/plans/messages.md). A sidebar of
 * accounts, a slim bar, and the selected account's panel over the content area. The panel is a
 * native web view that Rust places at the rectangle this page reports; native views draw above the
 * page, so the panel hides whenever the section is left or a sheet, menu or toast covers the
 * area (a still snapshot stands in while a sheet is open).
 */
export const createMessagesModule: ModuleFactory = (shell): AppModule => {
  if (!isMainWindow) return mainWindowOnly();
  void startStore();

  let selected: string | null = remembered(SELECTED_KEY);
  let visible = false;
  let covered = false;
  /** The account whose panel the user put to sleep while it was selected: it stays asleep until woken. */
  let napping: string | null = null;
  /** What Rust shows now, as far as this page knows. */
  let shownId: string | null = null;
  let lastRect: Rect | null = null;
  let phoneModule: { el: HTMLElement; show(force: boolean): void; phone: () => unknown } | null = null;
  let phoneLoading = false;

  const toast = showToast;
  const fail = (what: string) => (err: unknown) => toast(`${what}: ${errorText(err)}`);

  // ---- elements

  const foldButton = h("button", { class: "icon-btn small msg-fold", type: "button" }, icon("sidebar", 16));
  const dndButton = h("button", { class: "icon-btn small", type: "button" }, icon("bell", 16));
  const list = h("div", { class: "msg-list", role: "listbox", "aria-label": "Accounts" });
  const addButton = h("button", { class: "msg-add", type: "button", title: "Add an account" }, icon("plus", 16), h("span", null, "Add account"));
  const side = h("aside", { class: "msg-side" },
    h("div", { class: "msg-side-head" }, h("h1", { class: "msg-side-title" }, "Messages"), dndButton, foldButton), list, addButton);

  const barTitle = h("div", { class: "msg-bar-title" });
  const reloadButton = h("button", { class: "icon-btn small", type: "button", title: "Reload (Ctrl+R)", "aria-label": "Reload" }, icon("refresh", 16));
  const zoomOut = h("button", { class: "icon-btn small", type: "button", title: "Zoom out", "aria-label": "Zoom out" }, icon("zoomOut", 16));
  const zoomLabel = h("button", { class: "msg-zoom", type: "button", title: "Reset the zoom" });
  const zoomIn = h("button", { class: "icon-btn small", type: "button", title: "Zoom in", "aria-label": "Zoom in" }, icon("zoomIn", 16));
  const browserButton = h("button", { class: "icon-btn small", type: "button", title: "Open in your browser", "aria-label": "Open in your browser" }, icon("external", 16));
  const sleepButton = h("button", { class: "icon-btn small", type: "button", title: "Hibernate: unload it to free memory", "aria-label": "Hibernate" }, icon("hourglass", 16));
  const bar = h("header", { class: "msg-bar" }, barTitle, reloadButton, h("span", { class: "sep" }), zoomOut, zoomLabel, zoomIn, h("span", { class: "sep" }), browserButton, sleepButton);

  const stage = h("div", { class: "msg-stage" });
  const snap = h("img", { class: "msg-snap", alt: "", hidden: true });
  const placeholder = h("div", { class: "msg-placeholder" });
  const phoneWrap = h("div", { class: "msg-phonewrap phone-module", hidden: true });
  stage.append(placeholder, phoneWrap, snap);
  const main = h("div", { class: "msg-main" }, bar, stage);
  const el = h("section", { class: "messages-module" }, side, main);

  const folded = () => remembered(FOLDED_KEY) === "1";
  function showFolded() {
    el.classList.toggle("folded", folded());
    foldButton.title = folded() ? "Show the sidebar" : "Fold the sidebar";
    foldButton.setAttribute("aria-label", foldButton.title);
  }
  foldButton.addEventListener("click", () => {
    remember(FOLDED_KEY, folded() ? "0" : "1");
    showFolded();
  });
  showFolded();

  // ---- keeping the panel over the content area

  const rectNow = (): Rect | null => {
    const r = stage.getBoundingClientRect();
    if (r.width < 8 || r.height < 8) return null;
    return { x: Math.round(r.left), y: Math.round(r.top), w: Math.round(r.width), h: Math.round(r.height) };
  };

  /** The account the panel should show now, or null when none should. */
  function wanted(): string | null {
    // Settings and the guide hide the section without deactivating it: no area, no panel.
    if (!visible || covered || !selected || selected === PHONE || selected === napping || !rectNow()) return null;
    return account(selected)?.mode === "browser" ? null : account(selected) ? selected : null;
  }

  let busy = false;
  let again = false;
  async function apply() {
    const want = wanted();
    try {
      if (want) {
        const r = rectNow();
        if (!r) return;
        if (shownId !== want) {
          await msg.show(want, r);
          shownId = want;
          lastRect = r;
        } else if (!sameRect(r, lastRect)) {
          lastRect = r;
          await msg.rect(r);
        }
        snap.hidden = true;
      } else if (shownId) {
        const id = shownId;
        shownId = null;
        lastRect = null;
        if (visible && covered && !document.body.classList.contains("app-locked")) {
          // A sheet covers the area: keep a still picture of the panel under it.
          await msg.snapshot(id).then((url) => {
            snap.src = url;
            snap.hidden = false;
          }, () => (snap.hidden = true));
        }
        await msg.hide();
      }
    } catch (err) {
      console.warn("Messages:", err);
    }
  }
  /** Makes the panel match the page; runs one at a time, and once more if something changed meanwhile. */
  async function sync() {
    if (busy) {
      again = true;
      return;
    }
    busy = true;
    do {
      again = false;
      await apply();
    } while (again);
    busy = false;
  }

  // A native view draws above the page: when a sheet, menu or toast overlaps the content area the
  // panel steps aside (see `apply`).
  function computeCovered(): boolean {
    if (document.body.classList.contains("app-locked")) return true;
    const area = stage.getBoundingClientRect();
    for (const child of document.body.children) {
      if (!(child instanceof HTMLElement) || child.id === "app" || child.hidden || ["SCRIPT", "STYLE", "LINK"].includes(child.tagName)) continue;
      const style = getComputedStyle(child);
      if (style.display === "none" || style.visibility === "hidden") continue;
      const box = child.getBoundingClientRect();
      if (box.width > 0 && box.height > 0 && box.left < area.right && box.right > area.left && box.top < area.bottom && box.bottom > area.top) return true;
    }
    return false;
  }
  const watchCover = () => {
    const now = computeCovered();
    if (now === covered) return;
    covered = now;
    void sync();
  };
  const coverObserver = new MutationObserver(watchCover);
  coverObserver.observe(document.body, { childList: true });
  new ResizeObserver(() => {
    el.classList.toggle("compact", el.clientWidth < 720);
    if (shownId) void sync();
  }).observe(el);
  new ResizeObserver(() => shownId && void sync()).observe(stage);
  window.addEventListener("resize", () => shownId && void sync());
  // Anything that moves the area without resizing it (a bar that appears) is caught by looking now and then.
  let poll = 0;

  // ---- the list

  const phoneOn = () => shell.config.features.phone;
  function phoneUnread(): number {
    return phoneStore?.getState()?.phones.reduce((sum, p) => sum + (p.unreadSms || 0), 0) ?? 0;
  }
  let phoneStore: typeof import("../phone/store") | null = null;

  const badge = (n: number) => (n > 0 ? h("span", { class: "msg-badge" }, n > 99 ? "99+" : String(n)) : null);

  function itemFor(a: Account): HTMLElement {
    const item = h("button", { class: `msg-item${a.id === selected ? " active" : ""}${a.muted ? " muted" : ""}${a.awake || a.mode === "browser" ? "" : " asleep"}`, type: "button", role: "option", "aria-selected": String(a.id === selected), "data-id": a.id,
      title: `${a.name}${a.unread ? ` (${a.unread} unread)` : ""}${a.awake ? "" : "\nSleeping: opens when you do"}${a.muted ? "\nMuted" : ""}` },
      h("span", { class: "msg-item-icon" }, serviceIcon(a.service, 28)),
      a.mode === "browser" ? h("span", { class: "msg-item-name" }, a.name, h("small", { class: "msg-item-sub" }, "Opens in browser")) : h("span", { class: "msg-item-name" }, a.name),
      a.muted ? h("span", { class: "msg-item-mute", title: "Muted" }, icon("mute", 14)) : null,
      badge(a.unread));
    item.addEventListener("click", () => select(a.id, true));
    item.addEventListener("contextmenu", (e) => {
      e.preventDefault();
      accountMenu(a, e.clientX, e.clientY);
    });
    item.addEventListener("pointerdown", (e) => startDrag(e, item, a.id));
    return item;
  }

  function drawList() {
    const items: HTMLElement[] = [];
    if (phoneOn()) {
      const tile = h("span", { class: "msg-phone-tile" }, icon("phone", 16));
      const phoneItem = h("button", { class: `msg-item${selected === PHONE ? " active" : ""}`, type: "button", role: "option", "aria-selected": String(selected === PHONE), title: "Your phone's texts (Phone Link)" },
        h("span", { class: "msg-item-icon" }, tile), h("span", { class: "msg-item-name" }, "Phone"), badge(phoneUnread()));
      phoneItem.addEventListener("click", () => select(PHONE));
      items.push(phoneItem);
      if (accounts().length) items.push(h("div", { class: "msg-rule" }));
    }
    items.push(...accounts().map(itemFor));
    list.replaceChildren(...items);
    const dnd = settings()?.dnd ?? false;
    dndButton.classList.toggle("on", dnd);
    dndButton.replaceChildren(icon(dnd ? "moon" : "bell", 16));
    dndButton.title = dnd ? "Do not disturb is on: no desktop notifications. Click to turn it off." : "Do not disturb: silence the desktop notifications";
    dndButton.setAttribute("aria-label", dndButton.title);
    dndButton.setAttribute("aria-pressed", String(dnd));
  }

  // Dragging an account up or down puts the accounts in your order.
  let dragged = false;
  function startDrag(e: PointerEvent, item: HTMLElement, id: string) {
    if (e.button !== 0) return;
    const startY = e.clientY;
    let dragging = false;
    let target = -1;
    const others = () => [...list.querySelectorAll<HTMLElement>(".msg-item[data-id]")].filter((i) => i !== item);
    const move = (ev: PointerEvent) => {
      if (!dragging && Math.abs(ev.clientY - startY) > 6) {
        dragging = true;
        item.classList.add("dragging");
      }
      if (!dragging) return;
      item.style.transform = `translateY(${ev.clientY - startY}px)`;
      const rest = others();
      target = rest.findIndex((i) => ev.clientY < i.getBoundingClientRect().top + i.offsetHeight / 2);
      if (target < 0) target = rest.length;
      rest.forEach((i, n) => {
        i.classList.toggle("drop-before", n === target);
        i.classList.toggle("drop-after", target === rest.length && n === rest.length - 1);
      });
    };
    const finish = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", finish);
      window.removeEventListener("pointercancel", finish);
      if (!dragging) return;
      dragged = true;
      window.setTimeout(() => (dragged = false), 0);
      const ids = others().map((i) => i.dataset.id!);
      ids.splice(Math.max(0, target), 0, id);
      item.classList.remove("dragging");
      item.style.transform = "";
      others().forEach((i) => i.classList.remove("drop-before", "drop-after"));
      // Show the new order at once; the backend's change event then redraws it for real.
      const byId = new Map(accounts().map((a) => [a.id, a]));
      accounts().splice(0, accounts().length, ...ids.map((i) => byId.get(i)!).filter(Boolean));
      drawList();
      void msg.reorder(ids).catch(fail("Couldn't save the order"));
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", finish);
    window.addEventListener("pointercancel", finish);
  }
  // A drag that ends over its own item must not also select it.
  list.addEventListener("click", (e) => {
    if (dragged) e.stopImmediatePropagation();
  }, true);

  // ---- the stage

  function drawBar() {
    const a = selected && selected !== PHONE ? account(selected) : undefined;
    bar.hidden = !a;
    if (!a) return;
    const web = a.mode === "browser";
    for (const b of [reloadButton, zoomOut, zoomLabel, zoomIn, sleepButton]) b.hidden = web;
    for (const s of bar.querySelectorAll(".sep")) (s as HTMLElement).hidden = web;
    barTitle.replaceChildren(h("strong", { title: a.name }, a.name), h("span", null, serviceInfo(a.service).name));
    zoomLabel.textContent = `${Math.round(a.zoom * 100)}%`;
    sleepButton.disabled = !a.awake;
  }

  /** Launches (or brings up) the account's own browser window. */
  function openWindow(a: Account) {
    void msg.openBrowserWindow(a.id).catch(fail("Couldn't open the browser"));
  }

  function placeholderFor(a: Account | undefined, kind: "empty" | "asleep" | "waiting" | "none") {
    placeholder.className = `msg-placeholder${kind === "waiting" ? " waiting" : ""}`;
    if (kind === "empty") {
      placeholder.replaceChildren(
        h("div", { class: "msg-empty-icons" }, ...SERVICES.slice(0, 6).map((s) => serviceIcon(s.id, 40))),
        h("strong", null, "All your chats in one place"),
        h("p", null, "Add WhatsApp, Messenger, Microsoft Teams, Telegram and more. Each account signs in on the service's own page and keeps its own login."),
        h("button", { class: "btn primary", type: "button", onclick: () => void addAccount() }, icon("plus", 15), "Add an account"));
    } else if (kind === "asleep" && a) {
      placeholder.replaceChildren(serviceIcon(a.service, 48), h("strong", null, `${a.name} is asleep`),
        h("p", null, "Its page is unloaded to free memory. You stay signed in."),
        h("button", { class: "btn primary", type: "button", onclick: () => wake(a) }, "Wake up"));
    } else if (kind === "waiting" && a) {
      placeholder.replaceChildren(serviceIcon(a.service, 48), h("strong", null, `Opening ${a.name}…`));
    } else {
      placeholder.replaceChildren();
    }
  }

  function drawStage() {
    drawBar();
    const a = selected && selected !== PHONE ? account(selected) : undefined;
    const isPhone = selected === PHONE && phoneOn();
    phoneWrap.hidden = !isPhone;
    placeholder.hidden = isPhone;
    if (isPhone) {
      snap.hidden = true;
      void showPhone();
    } else if (!a) {
      placeholderFor(undefined, accounts().length ? "none" : "empty");
    } else if (a.mode === "browser") {
      placeholder.className = "msg-placeholder";
      placeholder.replaceChildren(serviceIcon(a.service, 48), h("strong", null, `${a.name} opens in a browser window`),
        h("p", null, "It has its own window and its own login, and calls work there. Zazabor shows no unread counts or notifications for it: the browser shows its own."),
        h("button", { class: "btn primary", type: "button", onclick: () => openWindow(a) }, icon("external", 15), "Open"),
        h("button", { class: "btn ghost", type: "button", onclick: () => void switchMode(a, "panel") }, "Open inside Zazabor instead"));
    } else if (napping === a.id) {
      placeholderFor(a, "asleep");
    } else {
      placeholderFor(a, "waiting");
    }
  }

  function draw() {
    // An account that is gone can't stay selected.
    if (selected !== PHONE && selected && !account(selected)) selected = null;
    if (selected === PHONE && !phoneOn()) selected = null;
    if (!selected) selected = accounts()[0]?.id ?? null;
    drawList();
    drawStage();
    void sync();
  }

  function select(id: string, launch = false) {
    if (id === PHONE ? !phoneOn() : !account(id)) return;
    if (launch && account(id)?.mode === "browser") openWindow(account(id)!);
    selected = id;
    if (napping === id) napping = null;
    remember(SELECTED_KEY, id);
    snap.hidden = true;
    draw();
  }

  /** Switches by position in the list (Ctrl+1 is the first account), or by step. */
  const nth = (n: number) => {
    const a = accounts()[n];
    if (a) select(a.id);
  };
  const step = (by: number) => {
    const ids = [...(phoneOn() ? [PHONE] : []), ...accounts().map((a) => a.id)];
    if (!ids.length) return;
    const at = ids.indexOf(selected ?? "");
    select(ids[(at + by + ids.length) % ids.length]);
  };

  // ---- Phone texts: the Phone section's Messages view, embedded

  async function showPhone() {
    if (!phoneModule && !phoneLoading) {
      phoneLoading = true;
      try {
        const [{ messagesView }, store, { startPhoneService }] = await Promise.all([
          import("../phone/views/messages"), import("../phone/store"), import("../phone/global"), import("../phone/phone.css"),
        ]);
        phoneStore = store;
        startPhoneService(shell);
        const current = () => store.getState()?.phones.find((p) => p.phoneId === shell.config.phone.active) ?? store.getState()?.phones[0] ?? null;
        const view = messagesView({
          shell, phone: current,
          showTab: () => shell.showModule("phone"),
          messageTo: () => shell.showModule("phone"),
          downloadDir: () => shell.config.phone.downloadDir,
          showTransfers: () => shell.showModule("phone"),
        });
        const none = h("div", { class: "msg-placeholder" },
          icon("phone", 34), h("strong", null, "No phone linked"), h("p", null, "Link your Android phone in the Phone section to read and send its texts here."),
          h("button", { class: "btn primary", type: "button", onclick: () => shell.showModule("phone") }, "Open Phone"));
        const wrap = h("div", { class: "pl-main" }, view.el);
        phoneWrap.replaceChildren(wrap, none);
        new ResizeObserver(() => phoneWrap.classList.toggle("narrow", phoneWrap.clientWidth < 740)).observe(phoneWrap);
        phoneModule = {
          el: phoneWrap, phone: current,
          show(force) {
            const has = !!current();
            wrap.hidden = !has;
            none.hidden = has;
            if (has) view.show(force);
          },
        };
        store.subscribe("state", () => {
          drawList();
          if (selected === PHONE && visible) phoneModule?.show(true);
        });
        await store.startStore();
        drawList();
      } catch (err) {
        console.warn("Messages: the phone view didn't load:", err);
      }
      phoneLoading = false;
    }
    if (selected === PHONE) phoneModule?.show(false);
  }

  // ---- actions

  async function addAccount() {
    const answer = await addAccountSheet(accounts());
    if (!answer) return;
    try {
      const a = await msg.add(answer.service, answer.name, answer.url, answer.mode);
      await refresh();
      select(a.id, a.mode === "browser");
    } catch (err) {
      fail("Couldn't add the account")(err);
    }
  }

  const patch = (a: Account, change: Parameters<typeof msg.update>[1], what = "Couldn't change the account") => msg.update(a.id, change).then(() => refresh(), fail(what));
  const setZoom = (a: Account, zoom: number) => patch(a, { zoom: Math.round(Math.min(3, Math.max(0.5, zoom)) * 100) / 100 });

  function wake(a: Account) {
    if (napping === a.id) napping = null;
    select(a.id);
  }

  function hibernate(a: Account) {
    if (a.id === selected) napping = a.id;
    msg.hibernate(a.id).then(() => refresh(), fail("Couldn't hibernate")).finally(() => draw());
    // Show the sleeping page at once; the backend's answer redraws it.
    if (a.id === selected) {
      shownId = null;
      drawStage();
    }
  }

  async function switchMode(a: Account, mode: "panel" | "browser") {
    await patch(a, { mode });
    toast(mode === "browser" ? "Now opens in a browser window. Its login doesn't carry over: sign in there." : "Now opens inside Zazabor. Its login doesn't carry over: sign in again.", undefined, 7000);
    if (mode === "browser" && a.id === selected) openWindow({ ...a, mode });
  }

  async function rename(a: Account) {
    await nameSheet({ title: "Rename account", name: a.name, isDir: true, submitLabel: "Rename", taken: new Set(), apply: async (name) => { await msg.update(a.id, { name }); await refresh(); } });
  }

  async function remove(a: Account) {
    const answer = await removeAccountSheet(a);
    if (!answer) return;
    try {
      if (a.id === selected) {
        selected = null;
        napping = null;
      }
      await msg.remove(a.id, answer.wipe);
      await refresh();
      toast(answer.wipe ? `${a.name} was removed and its data deleted` : `${a.name} was removed`);
    } catch (err) {
      fail("Couldn't remove the account")(err);
    }
  }

  function accountMenu(a: Account, x: number, y: number) {
    const items: MenuItem[] = [
      ...((a.mode === "browser" ? [
        { label: "Open Browser Window", icon: "external", action: () => openWindow(a) } as MenuItem,
        { label: "Open Inside Zazabor Instead", icon: "sync", action: () => void switchMode(a, "panel") } as MenuItem,
        "separator" as const,
        { label: "Rename…", icon: "pencil", action: () => void rename(a) } as MenuItem,
        { label: "Remove…", icon: "trash", danger: true, action: () => void remove(a) } as MenuItem,
      ] : [
      { label: "Reload", icon: "refresh", shortcut: "ctrl+r", disabled: !a.awake, action: () => void msg.reload(a.id).catch(fail("Couldn't reload")) },
      { label: "Zoom In", icon: "zoomIn", action: () => void setZoom(a, a.zoom + ZOOM_STEP) },
      { label: "Zoom Out", icon: "zoomOut", action: () => void setZoom(a, a.zoom - ZOOM_STEP) },
      { label: "Actual Size", icon: "fit", disabled: a.zoom === 1, action: () => void setZoom(a, 1) },
      "separator",
      { label: a.muted ? "Unmute Notifications" : "Mute Notifications", icon: a.muted ? "bell" : "mute", action: () => void patch(a, { muted: !a.muted }) },
      { label: "Hibernate Now", icon: "hourglass", disabled: !a.awake, action: () => hibernate(a) },
      { label: a.sleepAfter === null ? `Sleep After ${settings()?.defaultSleep ?? 30} Minutes Unused` : "Keep Always On", icon: "clock", action: () => void patch(a, { sleepAfter: a.sleepAfter === null ? settings()?.defaultSleep ?? 30 : null }) },
      "separator",
      { label: "Open in Browser", icon: "external", action: () => void msg.openExternal(a.id).catch(fail("Couldn't open the browser")) },
      { label: "Rename…", icon: "pencil", action: () => void rename(a) },
      { label: "Open in Browser Instead", icon: "sync", action: () => void switchMode(a, "browser") },
      { label: "Remove…", icon: "trash", danger: true, action: () => void remove(a) },
      ]) as MenuItem[]),
    ];
    openMenu(x, y, items);
  }

  async function switcher() {
    const entries = accounts().map((a) => ({ id: a.id, name: a.name, detail: serviceInfo(a.service).name, service: a.service, unread: a.unread }));
    if (!entries.length) return;
    const id = await quickSwitcher(entries);
    if (id) select(id);
  }

  // ---- wiring

  addButton.addEventListener("click", () => void addAccount());
  dndButton.addEventListener("click", () => {
    const on = !(settings()?.dnd ?? false);
    void msg.settings({ dnd: on }).then(() => refresh(), fail("Couldn't change Do Not Disturb"));
    toast(on ? "Do not disturb is on: no desktop notifications" : "Do not disturb is off");
  });
  const current = () => (selected && selected !== PHONE ? account(selected) : undefined);
  reloadButton.addEventListener("click", () => current() && void msg.reload(current()!.id).catch(fail("Couldn't reload")));
  zoomOut.addEventListener("click", () => current() && void setZoom(current()!, current()!.zoom - ZOOM_STEP));
  zoomIn.addEventListener("click", () => current() && void setZoom(current()!, current()!.zoom + ZOOM_STEP));
  zoomLabel.addEventListener("click", () => current() && void setZoom(current()!, 1));
  browserButton.addEventListener("click", () => current() && void msg.openExternal(current()!.id).catch(fail("Couldn't open the browser")));
  sleepButton.addEventListener("click", () => current() && hibernate(current()!));

  subscribe(() => {
    draw();
  });
  // Keys the panel forwarded (it has the keyboard focus, so the page never sees them).
  void onMsg("msg-key", ({ key }) => {
    if (!visible || covered) return;
    if (/^[1-9]$/.test(key)) nth(Number(key) - 1);
    else if (key === "prev") step(-1);
    else if (key === "next") step(1);
    else if (key === "reload") current() && void msg.reload(current()!.id).catch(() => undefined);
    else if (key === "switcher") void switcher();
  });
  onOpenRequest((id) => account(id) && select(id));

  // The same keys when the page itself has the focus. Ctrl+1…9 would switch sections in the rail, so
  // this listener comes first, and only while Messages shows with nothing covering it.
  window.addEventListener("keydown", (e) => {
    if (!visible || covered || e.altKey || e.metaKey || !e.ctrlKey) return;
    let handled = true;
    if (!e.shiftKey && /^[1-9]$/.test(e.key)) nth(Number(e.key) - 1);
    else if (e.shiftKey && (e.code === "BracketLeft" || e.key === "{")) step(-1);
    else if (e.shiftKey && (e.code === "BracketRight" || e.key === "}")) step(1);
    else if (!e.shiftKey && e.key.toLowerCase() === "r") current() && void msg.reload(current()!.id).catch(() => undefined);
    else if (!e.shiftKey && e.key.toLowerCase() === "k") void switcher();
    else if (e.shiftKey && e.key.toLowerCase() === "k") void switcher();
    else handled = false;
    if (handled) {
      e.preventDefault();
      e.stopImmediatePropagation();
    }
  }, true);

  return {
    id: "messages",
    title: "Messages",
    icon: "message",
    el,
    activate() {
      visible = true;
      covered = computeCovered();
      const asked = takePendingOpen();
      if (asked && account(asked)) {
        selected = asked;
        napping = napping === asked ? null : napping;
      }
      void refresh().then(() => {
        draw();
      });
      draw();
      window.clearInterval(poll);
      poll = window.setInterval(() => {
        watchCover();
        if (shownId) void sync();
      }, 250);
    },
    deactivate() {
      visible = false;
      window.clearInterval(poll);
      void sync();
    },
    onLock() {
      covered = true;
      return sync();
    },
    lockChanged() {
      watchCover();
    },
    handleKey: () => false,
    guideTopic: () => "messages",
    dispose() {
      visible = false;
      window.clearInterval(poll);
      coverObserver.disconnect();
      void msg.hide().catch(() => undefined);
    },
  };
};

/** A window other than the main one can't hold the panels (they live in the main window). */
function mainWindowOnly(): AppModule {
  const el = h("section", { class: "messages-module" }, h("div", { class: "msg-placeholder", style: { position: "static", flex: "1" } },
    icon("message", 34), h("strong", null, "Messages lives in the main window"), h("p", null, "The chat panels float over the main window, so they can't open in a window of their own.")));
  return { id: "messages", title: "Messages", icon: "message", el, activate() {}, deactivate() {}, handleKey: () => false };
}

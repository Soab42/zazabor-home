import { applyAppearance } from "../appearance";
import { adoptConfig, CONFIG_SAVED, parseConfig, type Config } from "../config";
import { serviceIcon, SERVICES } from "../modules/messages/catalogue";
import { contactFor, loadContacts, startStore } from "../modules/phone/store";
import { avatar } from "../modules/phone/ui";
import "../modules/phone/phone.css";
import { onOthers } from "../shell/windows";
import { h } from "../ui/dom";
import { icon } from "../ui/icons";
import { errorText, alertsApi, onAlertsState, type Alert, type AlertsState } from "./api";
import { mountAlertKeys } from "./keys";
import { startRing, stopRing } from "./ringtone";
import "./alerts.css";

/** How a source is named on its cards. */
const SOURCE_NAMES: Record<string, string> = { phone: "Phone", sms: "Phone · Messages" };
const sourceName = (a: Alert) => SOURCE_NAMES[a.service] ?? (a.service === "custom" ? a.accountName : SERVICES.find((s) => s.id === a.service)?.name) ?? a.accountName ?? "Messages";

/** The little tile that says where an alert comes from. */
function sourceIcon(service: string, size: number): HTMLElement {
  if (service === "phone" || service === "sms") {
    const tile = h("span", { class: `al-tile ${service}`, "aria-hidden": "true" }, icon(service === "phone" ? "call" : "message", Math.round(size * 0.58)));
    tile.style.width = tile.style.height = `${size}px`;
    return tile;
  }
  return serviceIcon(service, size);
}

const clock = (at: number) => new Date(at).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });

interface Card {
  el: HTMLElement;
  update(alert: Alert): void;
}

/**
 * The alerts window: transparent and sized to its cards, which the backend places in the corner of
 * the screen. It only draws; what is shown and what the buttons do lives in the backend, so
 * alerts keep coming with the main window hidden or gone.
 */
export async function startAlertsWindow(config: Config) {
  document.body.classList.add("alerts-window");
  applyAppearance(config.appearance);
  void onOthers<string>(CONFIG_SAVED, (json) => {
    try {
      adoptConfig(config, parseConfig(json));
    } catch {
      return;
    }
    applyAppearance(config.appearance);
  });
  mountAlertKeys();
  void startStore().catch(() => undefined);

  const stack = h("div", { class: "al-stack", role: "region", "aria-label": "Alerts", "aria-live": "polite" });
  const more = h("button", { class: "al-more", type: "button", title: "Dismiss these to see the older ones" });
  const root = h("div", { class: "al-root" }, stack, more);
  document.getElementById("app")!.append(root);

  const cards = new Map<number, Card>();
  let state: AlertsState | null = null;
  const fail = (a: Alert, err: unknown) => cards.get(a.id) && showNote(a.id, errorText(err));
  const notes = new Map<number, HTMLElement>();
  const showNote = (id: number, text: string) => {
    const n = notes.get(id);
    if (n) n.textContent = text;
  };
  const act = (a: Alert, task: Promise<unknown>, button?: HTMLButtonElement) => {
    if (button) button.disabled = true;
    task.catch((err) => fail(a, err)).finally(() => {
      if (button) button.disabled = false;
    });
  };

  // ---- cards ----------------------------------------------------------------------------------

  const hold = (el: HTMLElement, a: Alert) => {
    el.addEventListener("pointerenter", () => void alertsApi.hover(a.id, true).catch(() => undefined));
    el.addEventListener("pointerleave", () => {
      if (!el.contains(document.activeElement)) void alertsApi.hover(a.id, false).catch(() => undefined);
    });
    el.addEventListener("focusin", () => void alertsApi.hover(a.id, true).catch(() => undefined));
    el.addEventListener("focusout", () => {
      if (!el.matches(":hover")) void alertsApi.hover(a.id, false).catch(() => undefined);
    });
  };

  const face = (a: Alert, size: number): HTMLElement => {
    const who = a.kind === "call" && a.service === "phone" || a.service === "sms" ? contactFor(a.phoneId, a.number ?? "") : null;
    const wrap = h("span", { class: "al-face" }, avatar(a.title, size, a.phoneId, who));
    wrap.append(h("span", { class: "al-badge" }, sourceIcon(a.service, 16)));
    if (a.phoneId) void loadContacts(a.phoneId).then(() => {
      const found = contactFor(a.phoneId, a.number ?? "");
      if (found?.hasPhoto) wrap.replaceChildren(avatar(a.title, size, a.phoneId, found), h("span", { class: "al-badge" }, sourceIcon(a.service, 16)));
    });
    return wrap;
  };

  function callCard(a0: Alert): Card {
    let a = a0;
    const label = h("span", { class: "al-label" });
    const name = h("strong", { class: "al-name" });
    const sub = h("span", { class: "al-sub" });
    const note = h("p", { class: "al-note", role: "status" });
    notes.set(a.id, note);
    const faceHost = h("span", { class: "al-ring" });
    const round = (glyph: "call" | "callEnd" | "message" | "video", text: string, tone: string, run: (b: HTMLButtonElement) => void) => {
      const b = h("button", { class: `al-round ${tone}`, type: "button", title: text, "aria-label": text }, icon(glyph, 20));
      b.addEventListener("click", () => run(b));
      return h("div", { class: "al-round-wrap" }, b, h("span", null, text));
    };
    const answer = round(a.video ? "video" : "call", a.via === "web" ? "Answer in call window" : "Answer", "accept", (b) => act(a, alertsApi.answer(a.id), b));
    const decline = round("callEnd", "Decline", "end", (b) => act(a, alertsApi.decline(a.id), b));
    const messages = h("div", { class: "al-quick", hidden: true });
    const message = round("message", "Message", "neutral", () => {
      if (a.via === "web") return act(a, alertsApi.callMessage(a.id, ""));
      messages.hidden = !messages.hidden;
      if (!messages.hidden) custom.focus();
    });
    const custom = h("input", { class: "al-input", type: "text", placeholder: "Custom message…", "aria-label": "Custom message", maxLength: 160 });
    const sendCustom = () => {
      const text = custom.value.trim();
      if (text) act(a, alertsApi.callMessage(a.id, text));
    };
    custom.addEventListener("keydown", (e) => {
      if (e.key === "Enter") sendCustom();
    });
    const fill = () => {
      messages.replaceChildren(
        ...(state?.settings.quickReplies ?? []).map((text) => h("button", { class: "al-chip", type: "button", onclick: () => act(a, alertsApi.callMessage(a.id, text)) }, text)),
        h("div", { class: "al-custom" }, custom, h("button", { class: "al-send", type: "button", title: "Send and decline", "aria-label": "Send and decline", onclick: sendCustom }, icon("send", 16))));
    };
    const el = h("div", { class: "al-card call", role: "alertdialog", "aria-label": `Incoming call from ${a.title}` },
      h("div", { class: "al-call-head" }, sourceIcon(a.service, 18), label, h("span", { class: "al-keys", title: "Keyboard shortcuts" }, "Ctrl+Alt+A / D")),
      h("div", { class: "al-call-who" }, faceHost, h("div", { class: "al-call-text" }, name, sub)),
      note,
      h("div", { class: "al-call-actions" }, decline, a.canReply ? message : null, answer),
      messages);
    const update = (next: Alert) => {
      a = next;
      label.textContent = `${a.body}${a.video && !a.body.includes("video") ? " (video)" : ""} · ${sourceName(a)}`;
      name.textContent = a.title;
      sub.textContent = a.number && a.number !== a.title ? a.number : a.accountName && a.accountName !== a.title ? a.accountName : "";
      note.textContent = a.note ?? "";
      note.hidden = !a.note;
      faceHost.replaceChildren(face(a, 52));
      fill();
    };
    update(a);
    return { el, update };
  }

  function messageCard(a0: Alert): Card {
    let a = a0;
    let built = false;
    const name = h("strong", { class: "al-name" });
    const meta = h("span", { class: "al-meta" });
    const body = h("p", { class: "al-body" });
    const note = h("p", { class: "al-note", role: "status", hidden: true });
    notes.set(a.id, note);
    const faceHost = h("span", { class: "al-face-host" });
    const close = h("button", { class: "al-x", type: "button", title: "Dismiss", "aria-label": "Dismiss", onclick: () => void alertsApi.dismiss(a.id) }, icon("close", 13));
    const head = h("button", { class: "al-head", type: "button", title: "Open the conversation" }, faceHost, h("span", { class: "al-head-text" }, name, meta));
    head.addEventListener("click", () => act(a, alertsApi.open(a.id)));
    body.addEventListener("click", () => act(a, alertsApi.open(a.id)));

    const actions = h("div", { class: "al-actions" });
    const input = h("input", { class: "al-input", type: "text", placeholder: "Reply…", "aria-label": `Reply to ${a.title}`, maxLength: 480 });
    const send = h("button", { class: "al-send", type: "button", title: "Send", "aria-label": "Send", onclick: () => submit() }, icon("send", 16));
    const submit = () => {
      const text = input.value.trim();
      if (!text) return;
      input.disabled = send.disabled = true;
      note.hidden = false;
      note.textContent = "Sending…";
      alertsApi.reply(a.id, text).then(
        () => {
          note.textContent = "Sent ✓";
          note.classList.add("ok");
          window.setTimeout(() => void alertsApi.dismiss(a.id).catch(() => undefined), 1600);
        },
        (err) => {
          note.textContent = errorText(err);
          input.disabled = send.disabled = false;
        });
    };
    input.addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        e.preventDefault();
        submit();
      } else if (e.key === "Escape") input.blur();
    });
    const button = (glyph: "call" | "check" | "reply", text: string, run: () => void) =>
      h("button", { class: "al-btn", type: "button", title: text, onclick: run }, icon(glyph, 14), text);
    const el = h("div", { class: "al-card toast", role: "status" }, h("div", { class: "al-top" }, head, close), body, note);
    const update = (next: Alert) => {
      a = next;
      name.textContent = a.title;
      meta.textContent = `${sourceName(a)} · ${clock(a.at)}`;
      body.textContent = a.body;
      body.hidden = !a.body;
      faceHost.replaceChildren(face(a, 38));
      if (!built) {
        built = true;
        // SMS answers inline, with Call and Mark read beside the field; a chat can only be opened (a
        // web app can't be typed into from here).
        const iconButton = (glyph: "call" | "check", text: string, run: () => void) =>
          h("button", { class: "al-ico", type: "button", title: text, "aria-label": text, onclick: run }, icon(glyph, 16));
        if (a.canReply && !a.accountId) {
          el.append(h("div", { class: "al-reply" }, input, send,
            a.canCall ? iconButton("call", "Call", () => act(a, alertsApi.call(a.id))) : null,
            a.canRead ? iconButton("check", "Mark read", () => act(a, alertsApi.read(a.id))) : null));
          return;
        }
        const row: HTMLElement[] = [];
        if (a.canReply && a.accountId) row.push(button("reply", "Reply", () => act(a, alertsApi.open(a.id))));
        if (a.canCall) row.push(button("call", "Call", () => act(a, alertsApi.call(a.id))));
        if (a.canRead) row.push(button("check", "Mark read", () => act(a, alertsApi.read(a.id))));
        actions.append(...row);
        if (row.length) el.append(actions);
      }
    };
    update(a);
    return { el, update };
  }

  // ---- drawing --------------------------------------------------------------------------------

  const draw = (next: AlertsState) => {
    state = next;
    root.dataset.position = next.settings.position;
    const ids = new Set(next.alerts.map((a) => a.id));
    for (const [id, card] of cards) {
      if (ids.has(id)) continue;
      card.el.remove();
      cards.delete(id);
      notes.delete(id);
    }
    next.alerts.forEach((a, index) => {
      let card = cards.get(a.id);
      if (!card) {
        card = a.kind === "call" ? callCard(a) : messageCard(a);
        hold(card.el, a);
        cards.set(a.id, card);
      } else {
        card.update(a);
      }
      // Cards keep the backend's order (oldest first).
      if (stack.children[index] !== card.el) stack.insertBefore(card.el, stack.children[index] ?? null);
    });
    more.hidden = next.hidden === 0;
    more.textContent = `+${next.hidden} more`;
    const ringing = next.alerts.some((a) => a.kind === "call" && !a.silent) && next.settings.ringtone;
    if (ringing) startRing(next.settings.volume);
    else stopRing();
    report();
  };

  more.addEventListener("click", () => {
    for (const a of state?.alerts ?? []) if (a.kind === "message") void alertsApi.dismiss(a.id).catch(() => undefined);
  });

  let measured = 0;
  const report = () => {
    // A timer, not an animation frame: a hidden window never gets frames, and it is what shows it.
    window.setTimeout(() => {
      const height = Math.ceil(Math.max(root.getBoundingClientRect().height, root.scrollHeight));
      if (height !== measured && cards.size) {
        measured = height;
        void alertsApi.resize(height).catch(() => undefined);
      }
    }, 30);
  };
  new ResizeObserver(report).observe(root);
  await onAlertsState((s) => {
    measured = 0;
    draw(s);
  });
  await alertsApi.ready();
  draw(await alertsApi.state());
}

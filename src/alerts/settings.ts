import type { SettingsRow } from "../appearance";
import type { Config } from "../config";
import type { Category } from "../shell/settings";
import { h } from "../ui/dom";
import { alertsApi, errorText, onAlertsSettings, type AlertSettings, type Position } from "./api";

/**
 * Settings → Alerts: where the call cards and message toasts appear, how long toasts stay, the
 * ringtone, Do not disturb, and which sources show alerts (off: the desktop's own notification).
 */
export function alertsCategory(_host: { readonly config: Config }): Category {
  let current: AlertSettings | null = null;
  const problem = h("p", { class: "sf-hint pl-settings-error", hidden: true });
  const save = (patch: Partial<AlertSettings>) => {
    if (!current) return;
    current = { ...current, ...patch };
    void alertsApi.setSettings(current).then((s) => ((current = s), (problem.hidden = true)), (err) => {
      problem.hidden = false;
      problem.textContent = errorText(err);
    });
  };

  const updates: (() => void)[] = [];
  const toggle = (label: string, hint: string, key: "ringtone" | "dnd" | "previews" | "phoneCalls" | "sms" | "whatsapp" | "teams" | "messenger" | "otherMessages" | "callKeys"): SettingsRow => {
    const input = h("input", { type: "checkbox", class: "switch", role: "switch" });
    input.addEventListener("change", () => save({ [key]: input.checked }));
    updates.push(() => (input.checked = !!current?.[key]));
    return { label, el: h("label", { class: "sf-row sf-toggle" }, h("div", { class: "st-text" }, h("span", { class: "sf-label" }, label), h("p", { class: "sf-hint" }, hint)), input) };
  };
  const select = (label: string, hint: string, options: [string, string][], get: (s: AlertSettings) => string, set: (v: string) => Partial<AlertSettings>): SettingsRow => {
    const el = h("select", { class: "ops-input", "aria-label": label });
    for (const [value, text] of options) el.append(h("option", { value }, text));
    el.addEventListener("change", () => save(set(el.value)));
    updates.push(() => current && (el.value = get(current)));
    return { label, el: h("div", { class: "sf-row plain" }, h("div", { class: "st-text" }, h("span", { class: "sf-label" }, label), h("p", { class: "sf-hint" }, hint)), el) };
  };

  const volume = h("input", { type: "range", min: 0, max: 100, step: 5, "aria-label": "Ringtone volume" });
  volume.addEventListener("change", () => save({ volume: Number(volume.value) }));
  updates.push(() => current && (volume.value = String(current.volume)));
  const volumeRow: SettingsRow = { label: "Ringtone volume", el: h("div", { class: "sf-row plain" }, h("span", { class: "sf-label" }, "Ringtone volume"), volume) };

  const replies = h("textarea", { class: "ops-input", rows: 4, "aria-label": "Quick replies", style: { width: "100%", resize: "vertical" } });
  replies.addEventListener("change", () => save({ quickReplies: replies.value.split("\n").map((l) => l.trim()).filter(Boolean) }));
  updates.push(() => current && (replies.value = current.quickReplies.join("\n")));
  const repliesRow: SettingsRow = { label: "Quick replies", el: h("div", { class: "sf-row wide" }, h("div", { class: "st-text" }, h("span", { class: "sf-label" }, "Quick replies"), h("p", { class: "sf-hint" }, "Offered as “Message” on a ringing phone call: one per line. Sending one texts the caller and declines the call.")), replies) };

  const redraw = () => updates.forEach((u) => u());
  void alertsApi.settings().then((s) => ((current = s), redraw()), () => undefined);
  void onAlertsSettings((s) => ((current = s), redraw()));

  const off = "Off: your desktop's own notification is used instead.";
  return {
    id: "alerts", title: "Alerts", icon: "bell", section: "App",
    intro: problem,
    groups: [
      {
        title: "Where",
        rows: [
          select("Position", "Calls and messages stack in this corner of the screen, even when Zazabor Home's window is hidden.",
            [["bottom-right", "Bottom right"], ["bottom-left", "Bottom left"], ["top-right", "Top right"], ["top-left", "Top left"]], (s) => s.position, (v) => ({ position: v as Position })),
          select("Message toasts stay", "A toast stays while the pointer is on it or you are typing a reply.",
            [["4", "4 seconds"], ["8", "8 seconds"], ["15", "15 seconds"], ["30", "30 seconds"]], (s) => String(s.toastSecs), (v) => ({ toastSecs: Number(v) })),
          toggle("Show message text", "Toasts show who wrote and what. Off: only “New message”.", "previews"),
        ],
      },
      {
        title: "Calls",
        rows: [
          toggle("Ringtone", "A ring sounds from this PC while a call is ringing.", "ringtone"),
          volumeRow,
          toggle("Keyboard shortcuts", "Ctrl+Alt+A answers and Ctrl+Alt+D declines the ringing call, from any program (on Wayland: while Zazabor Home has the focus).", "callKeys"),
          repliesRow,
          toggle("Do not disturb", "No message toasts, and calls ring without a sound. Missed calls stay in each app.", "dnd"),
        ],
      },
      {
        title: "Show alerts for",
        rows: [
          toggle("Phone calls", `Incoming calls on your linked phone. ${off}`, "phoneCalls"),
          toggle("Phone texts", `New SMS, with reply, call and mark read. ${off}`, "sms"),
          toggle("WhatsApp", `Messages and calls. ${off}`, "whatsapp"),
          toggle("Teams", `Messages and calls. ${off}`, "teams"),
          toggle("Messenger", `Messages and calls. ${off}`, "messenger"),
          toggle("Other Messages accounts", `Telegram, Slack and the rest. ${off}`, "otherMessages"),
        ],
      },
    ],
    refresh() {
      problem.hidden = true;
      void alertsApi.settings().then((s) => ((current = s), redraw()), () => undefined);
    },
  };
}

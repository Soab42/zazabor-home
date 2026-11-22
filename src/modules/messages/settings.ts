import "../files/ops/ops.css";
import "./messages.css";
import type { SettingsRow } from "../../appearance";
import type { Config } from "../../config";
import type { Category } from "../../shell/settings";
import { formatBytes, h } from "../../ui/dom";
import { confirmSheet } from "../files/ops/sheets";
import { showToast } from "../files/ops/toast";
import { errorText, msg } from "./api";
import { serviceIcon, serviceInfo } from "./catalogue";
import { accounts, refresh, settings, startStore, subscribe } from "./store";

/**
 * Settings → Messages: the accounts (muted, always on or sleeping), do not disturb, whether
 * notifications show previews, how long a new account is left alone before it sleeps, and the
 * data the accounts keep on this PC.
 */
export function messagesCategory(_host: { readonly config: Config }): Category {
  const problem = h("p", { class: "sf-hint pl-settings-error", hidden: true });
  const fail = (err: unknown) => {
    problem.hidden = false;
    problem.textContent = errorText(err);
  };
  const change = (patch: Parameters<typeof msg.settings>[0]) => void msg.settings(patch).then(() => refresh(), fail);

  const toggle = (label: string, hint: string, get: () => boolean, set: (on: boolean) => void) => {
    const input = h("input", { type: "checkbox", class: "switch", role: "switch" });
    input.addEventListener("change", () => set(input.checked));
    const row: SettingsRow = { label, el: h("label", { class: "sf-row sf-toggle" }, h("div", { class: "st-text" }, h("span", { class: "sf-label" }, label), h("p", { class: "sf-hint" }, hint)), input) };
    return { row, update: () => (input.checked = get()) };
  };

  const dnd = toggle("Do not disturb", "No desktop notifications from any account. Unread counts still add up.", () => settings()?.dnd ?? false, (on) => change({ dnd: on }));
  const previews = toggle("Show previews", "Notifications show who wrote and what. Off: only “New message” and the account's name.", () => settings()?.previews ?? true, (on) => change({ previews: on }));

  const sleepSelect = h("select", { class: "ops-input msg-set-select", "aria-label": "New accounts sleep after" });
  for (const m of [5, 15, 30, 60, 120, 240]) sleepSelect.append(h("option", { value: String(m) }, m < 60 ? `${m} minutes` : `${m / 60} ${m === 60 ? "hour" : "hours"}`));
  sleepSelect.addEventListener("change", () => change({ defaultSleep: Number(sleepSelect.value) }));
  const sleepRow: SettingsRow = {
    label: "Sleep after",
    el: h("div", { class: "sf-row plain" }, h("div", { class: "st-text" }, h("span", { class: "sf-label" }, "New accounts sleep after"),
      h("p", { class: "sf-hint" }, "An account nobody looks at is unloaded after this long, which gives its memory back; opening it wakes it and you stay signed in. WhatsApp and Teams start as Always on so their notifications keep coming. Change one account in its menu in Messages.")), sleepSelect),
  };

  const list = h("div", { class: "msg-set-accounts" });
  const accountsRow: SettingsRow = { label: "Accounts", el: h("div", { class: "sf-row wide" }, h("span", { class: "sf-label" }, "Accounts"), list) };
  const drawAccounts = () => {
    list.replaceChildren(...(accounts().length ? accounts().map((a) => {
      const mute = h("input", { type: "checkbox", class: "switch", role: "switch", "aria-label": `Notifications from ${a.name}` });
      mute.checked = !a.muted;
      mute.addEventListener("change", () => void msg.update(a.id, { muted: !mute.checked }).then(() => refresh(), fail));
      const always = h("select", { class: "ops-input msg-set-select", "aria-label": `${a.name}: keep running` },
        h("option", { value: "always" }, "Always on"), h("option", { value: "sleep" }, `Sleep after ${a.sleepAfter ?? settings()?.defaultSleep ?? 30} min`));
      always.value = a.sleepAfter === null ? "always" : "sleep";
      always.hidden = a.mode === "browser";
      always.addEventListener("change", () => void msg.update(a.id, { sleepAfter: always.value === "always" ? null : settings()?.defaultSleep ?? 30 }).then(() => refresh(), fail));
      return h("div", { class: "msg-set-account" }, serviceIcon(a.service, 26),
        h("div", { class: "st-text" }, h("span", { class: "sf-label" }, a.name), h("p", { class: "sf-hint" }, a.mode === "browser" ? `${serviceInfo(a.service).name} · opens in a browser window, no notifications from Zazabor` : serviceInfo(a.service).name)),
        always, a.mode === "browser" ? null : h("span", { class: "sf-hint", title: "Notifications" }, "Notify"), a.mode === "browser" ? null : mute);
    }) : [h("p", { class: "msg-set-note" }, "No accounts yet. Add one in Messages.")]));
  };

  const size = h("span", { class: "st-value" }, "—");
  const clear = h("button", { class: "btn small danger", type: "button" }, "Clear All Data…");
  clear.addEventListener("click", () => {
    void confirmSheet({ title: "Sign out of every account?", message: "Every account's login, cache and stored data on this PC is deleted. The accounts stay in the list, and you sign in to each again.", confirmLabel: "Clear All Data", destructive: true }).then(async (ok) => {
      if (!ok) return;
      try {
        await msg.clearData();
        showToast("All Messages data was deleted");
      } catch (err) {
        fail(err);
      }
      void readSize();
    });
  });
  const dataRow: SettingsRow = { label: "Data on this PC", el: h("div", { class: "sf-row plain" }, h("div", { class: "st-text" }, h("span", { class: "sf-label" }, "Data on this PC"), h("p", { class: "sf-hint" }, "Logins, cookies and caches of all accounts.")), h("div", { class: "pl-settings-buttons" }, size, clear)) };
  const readSize = () => msg.dataSize().then((n) => (size.textContent = formatBytes(n)), () => (size.textContent = "—"));

  const redraw = () => {
    dnd.update();
    previews.update();
    sleepSelect.value = String(settings()?.defaultSleep ?? 30);
    if (![...sleepSelect.options].some((o) => o.value === sleepSelect.value)) sleepSelect.append(h("option", { value: sleepSelect.value }, `${sleepSelect.value} minutes`)), (sleepSelect.value = String(settings()?.defaultSleep ?? 30));
    drawAccounts();
  };
  subscribe(redraw);
  void startStore().then(redraw, () => undefined);

  return {
    id: "messages", title: "Messages", icon: "message", section: "Modules",
    intro: problem,
    groups: [
      { title: "Notifications", rows: [dnd.row, previews.row] },
      { title: "Accounts", rows: [accountsRow, sleepRow] },
      { title: "Data", rows: [dataRow] },
    ],
    refresh() {
      problem.hidden = true;
      void refresh().then(redraw);
      void readSize();
    },
  };
}

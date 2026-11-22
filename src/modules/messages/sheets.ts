import { h } from "../../ui/dom";
import { icon } from "../../ui/icons";
import { serviceIcon, serviceInfo, SERVICES } from "./catalogue";
import type { Account } from "./api";

/** The sheets of the Messages section: Add account, Remove and the quick switcher. */

/** Opens a glass sheet over the app; keys inside never reach the shortcuts behind it. */
function openSheet(card: HTMLElement, onEscape: () => void): () => void {
  const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  const overlay = h("div", { class: "modal-overlay ops-overlay msg-overlay" }, card);
  overlay.addEventListener("keydown", (e) => {
    e.stopPropagation();
    if (e.key === "Escape") {
      e.preventDefault();
      onEscape();
    }
  });
  overlay.addEventListener("pointerdown", (e) => {
    if (e.target === overlay) onEscape();
  });
  document.body.append(overlay);
  return () => {
    overlay.remove();
    if (previous?.isConnected) previous.focus({ preventScroll: true });
  };
}

export interface NewAccount {
  service: string;
  name: string;
  url?: string;
  mode: "panel" | "browser";
}

/** A custom address: https, or http on this computer (a page you run yourself). */
export function customUrlProblem(text: string): string | null {
  const value = text.trim();
  if (!value) return "Type the address of the web app.";
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    return "That isn't a web address. Start with https://";
  }
  if (url.username || url.password) return "Leave the user name and password out of the address.";
  if (url.protocol === "https:") return null;
  if (url.protocol === "http:" && ["127.0.0.1", "localhost", "[::1]"].includes(url.hostname)) return null;
  return "The address must start with https://";
}

/** Picks a service and a name. Resolves null when dismissed. */
export function addAccountSheet(existing: Account[]): Promise<NewAccount | null> {
  return new Promise((resolve) => {
    let close = () => {};
    const finish = (value: NewAccount | null) => {
      close();
      resolve(value);
    };
    let service = SERVICES[0].id;
    let choice = 0;
    let nameEdited = false;
    const tiles = new Map<string, HTMLButtonElement>();
    const name = h("input", { class: "ops-input", type: "text", maxlength: 40, spellcheck: false, autocomplete: "off", "aria-label": "Account name" });
    const url = h("input", { class: "ops-input", type: "url", placeholder: "https://chat.example.com", spellcheck: false, autocomplete: "off", "aria-label": "Web address" });
    const problem = h("p", { class: "ops-field-msg", role: "alert" });
    const choices = h("div", { class: "msg-choices", role: "radiogroup", "aria-label": "Account type" });
    const urlField = h("label", { class: "msg-field" }, h("span", { class: "msg-field-label" }, "Web address (https)"), url);
    const viaBrowser = h("input", { type: "checkbox" });
    const browserNote = h("span", { class: "msg-field-hint" });
    const browserRow = h("label", { class: "msg-check msg-browser-opt" }, viaBrowser,
      h("span", null, h("strong", null, "Sign in using your browser instead"), browserNote));
    const add = h("button", { class: "btn primary", type: "button" }, "Add");

    const suggested = (id: string) => {
      const base = serviceInfo(id).name;
      const same = existing.filter((a) => a.service === id).length;
      return same ? `${base} ${same + 1}` : base;
    };
    const draw = () => {
      const info = serviceInfo(service);
      for (const [id, tile] of tiles) {
        tile.classList.toggle("on", id === service);
        tile.setAttribute("aria-checked", String(id === service));
      }
      if (!nameEdited) name.value = suggested(service);
      choices.hidden = !info.choices;
      choices.replaceChildren(...(info.choices ?? []).map((c, i) => {
        const input = h("input", { type: "radio", name: "msg-choice", checked: i === choice });
        input.addEventListener("change", () => (choice = i));
        return h("label", { class: "msg-choice" }, input, h("span", null, h("strong", null, c.label), h("small", null, c.hint)));
      }));
      urlField.hidden = service !== "custom";
      const rec = !!info.browserRecommended;
      viaBrowser.checked = rec;
      browserNote.textContent = rec
        ? "Recommended: this service often refuses sign-in inside an app. It opens in its own browser window; Zazabor shows no unread counts or notifications for it."
        : "Opens in its own browser window with a login of its own (calls work there). Zazabor then shows no unread counts or notifications for it.";
      problem.textContent = "";
      check();
    };
    const check = () => {
      add.disabled = !name.value.trim() || (service === "custom" && !url.value.trim());
    };
    const submit = () => {
      const label = name.value.trim();
      if (!label) return;
      if (service === "custom") {
        const bad = customUrlProblem(url.value);
        if (bad) {
          problem.textContent = bad;
          url.focus();
          return;
        }
        return finish({ service, name: label, url: new URL(url.value.trim()).href, mode: viaBrowser.checked ? "browser" : "panel" });
      }
      const info = serviceInfo(service);
      finish({ service, name: label, url: info.choices?.[choice]?.url, mode: viaBrowser.checked ? "browser" : "panel" });
    };

    const grid = h("div", { class: "msg-grid", role: "radiogroup", "aria-label": "Service" }, ...SERVICES.map((s) => {
      const tile = h("button", { class: "msg-tile", type: "button", role: "radio", title: s.blurb }, serviceIcon(s.id, 34), h("strong", null, s.name), h("small", null, s.blurb));
      tile.addEventListener("click", () => {
        service = s.id;
        choice = 0;
        draw();
        (s.id === "custom" ? url : name).focus();
      });
      tiles.set(s.id, tile);
      return tile;
    }));
    name.addEventListener("input", () => {
      nameEdited = true;
      check();
    });
    url.addEventListener("input", () => {
      problem.textContent = "";
      check();
    });
    for (const input of [name, url]) {
      input.addEventListener("keydown", (e) => {
        if (e.key === "Enter" && !e.isComposing) {
          e.preventDefault();
          submit();
        }
      });
    }
    add.addEventListener("click", submit);

    const card = h("div", { class: "modal ops-sheet msg-sheet", role: "dialog", "aria-modal": "true", "aria-label": "Add account" },
      h("div", { class: "msg-sheet-head" }, h("h2", null, "Add an account"), h("p", null, "Each account signs in on the service's own page and keeps its own login.")),
      h("div", { class: "msg-sheet-body" }, grid, choices,
        h("label", { class: "msg-field" }, h("span", { class: "msg-field-label" }, "Name"), name), urlField, browserRow, problem),
      h("div", { class: "modal-foot ops-sheet-foot" }, h("span", { class: "ops-spacer" }),
        h("button", { class: "btn ghost", type: "button", onclick: () => finish(null) }, "Cancel"), add));
    close = openSheet(card, () => finish(null));
    draw();
    name.focus();
    name.select();
  });
}

/** Asks before removing an account; `wipe` is whether to also sign out and delete its data. */
export function removeAccountSheet(account: Account): Promise<{ wipe: boolean } | null> {
  return new Promise((resolve) => {
    let close = () => {};
    const finish = (value: { wipe: boolean } | null) => {
      close();
      resolve(value);
    };
    const wipe = h("input", { type: "checkbox", checked: true });
    const cancel = h("button", { class: "btn ghost", type: "button", onclick: () => finish(null) }, "Cancel");
    const confirm = h("button", { class: "btn danger", type: "button", onclick: () => finish({ wipe: wipe.checked }) }, "Remove");
    const card = h("div", { class: "modal ops-sheet", role: "dialog", "aria-modal": "true", "aria-label": "Remove account" },
      h("div", { class: "ops-sheet-body" },
        h("div", { class: "ops-sheet-icon danger" }, icon("trash", 20)),
        h("div", { class: "ops-sheet-text" },
          h("h2", null, `Remove ${account.name}?`),
          h("p", { class: "ops-sheet-msg" }, "The account leaves the list. Your messages stay with the service."),
          h("label", { class: "msg-check" }, wipe, h("span", null, "Also sign out and delete its data on this PC")))),
      h("div", { class: "modal-foot ops-sheet-foot" }, h("span", { class: "ops-spacer" }), cancel, confirm));
    close = openSheet(card, () => finish(null));
    cancel.focus();
  });
}

export interface SwitchEntry {
  id: string;
  name: string;
  detail: string;
  service: string;
  unread: number;
}

/** Quick switcher: type to narrow the accounts, Enter opens one. Resolves its id, or null. */
export function quickSwitcher(entries: SwitchEntry[]): Promise<string | null> {
  return new Promise((resolve) => {
    let close = () => {};
    let active = 0;
    let shown: SwitchEntry[] = entries;
    const finish = (value: string | null) => {
      close();
      resolve(value);
    };
    const input = h("input", { class: "ops-input msg-switch-input", type: "text", placeholder: "Go to an account…", spellcheck: false, autocomplete: "off", "aria-label": "Find an account" });
    const list = h("div", { class: "msg-switch-list", role: "listbox" });
    const draw = () => {
      const q = input.value.trim().toLowerCase();
      shown = entries.filter((e) => !q || e.name.toLowerCase().includes(q) || e.detail.toLowerCase().includes(q));
      active = Math.min(active, Math.max(0, shown.length - 1));
      list.replaceChildren(...(shown.length ? shown.map((e, i) => {
        const row = h("button", { class: `msg-switch-row${i === active ? " active" : ""}`, type: "button", role: "option" },
          serviceIcon(e.service, 24), h("span", { class: "msg-switch-name" }, e.name), h("small", null, e.detail),
          e.unread ? h("span", { class: "msg-badge" }, e.unread > 99 ? "99+" : String(e.unread)) : null);
        row.addEventListener("click", () => finish(e.id));
        return row;
      }) : [h("p", { class: "msg-switch-none" }, "No account matches.")]));
      list.querySelector(".active")?.scrollIntoView({ block: "nearest" });
    };
    input.addEventListener("input", () => {
      active = 0;
      draw();
    });
    input.addEventListener("keydown", (e) => {
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        if (shown.length) active = (active + (e.key === "ArrowDown" ? 1 : shown.length - 1)) % shown.length;
        draw();
      } else if (e.key === "Enter" && !e.isComposing) {
        e.preventDefault();
        if (shown[active]) finish(shown[active].id);
      }
    });
    const card = h("div", { class: "modal ops-sheet msg-switch", role: "dialog", "aria-modal": "true", "aria-label": "Quick switcher" }, input, list);
    close = openSheet(card, () => finish(null));
    draw();
    input.focus();
  });
}

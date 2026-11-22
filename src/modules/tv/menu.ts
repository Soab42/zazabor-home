import { h } from "../../ui/dom";
import { icon, type IconName } from "../../ui/icons";
import { isPhone } from "./phone";

export interface MenuEntry {
  label: string;
  glyph?: IconName;
  /** Shown as switched on (a list the title is on). */
  on?: boolean;
  danger?: boolean;
  /** A line of grey text under the label. */
  hint?: string;
  run: () => void;
}

let open: { close: () => void } | null = null;

/**
 * A menu of actions: a popover next to `at` (a point or an element) on the PC, a bottom sheet on
 * the phone. It closes on a choice, Escape or a click outside, and restores the focus.
 */
export function showMenu(title: string, entries: MenuEntry[], at: { x: number; y: number } | HTMLElement) {
  open?.close();
  const back = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  const buttons = entries.map((e) => {
    const b = h("button", { class: `tv-menu-item${e.on ? " on" : ""}${e.danger ? " danger" : ""}`, type: "button", role: "menuitem" },
      e.glyph ? icon(e.glyph, 18) : h("span", { class: "tv-menu-gap" }),
      h("span", { class: "tv-menu-text" }, h("span", null, e.label), e.hint ? h("small", null, e.hint) : null),
      e.on ? icon("check", 16) : null);
    b.addEventListener("click", () => {
      close();
      e.run();
    });
    return b;
  });
  const list = h("div", { class: "tv-menu-list", role: "menu", "aria-label": title }, ...buttons);
  const root = isPhone
    ? h("div", { class: "tv-sheet-wrap" }, h("div", { class: "tv-sheet", role: "dialog", "aria-label": title }, h("div", { class: "tv-sheet-grab" }), h("h2", { class: "tv-sheet-title" }, title), list))
    : h("div", { class: "tv-menu glass-strong" }, list);
  const keys = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      close();
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      e.stopPropagation();
      const i = buttons.indexOf(document.activeElement as HTMLButtonElement);
      const next = (i + (e.key === "ArrowDown" ? 1 : -1) + buttons.length) % buttons.length;
      buttons[i < 0 ? 0 : next].focus();
    }
  };
  const away = (e: Event) => {
    if (!root.contains(e.target as Node) || (isPhone && e.target === root)) close();
  };
  function close() {
    document.removeEventListener("keydown", keys, true);
    document.removeEventListener("pointerdown", away, true);
    root.classList.remove("in");
    window.setTimeout(() => root.remove(), 160);
    if (open?.close === close) open = null;
    back?.isConnected && !isPhone && back.focus({ preventScroll: true });
  }
  open = { close };
  document.body.append(root);
  if (!isPhone) {
    const w = root.offsetWidth;
    const hgt = root.offsetHeight;
    const r = at instanceof HTMLElement ? at.getBoundingClientRect() : null;
    let x = r ? r.left : (at as { x: number }).x;
    let y = r ? r.bottom + 6 : (at as { y: number }).y;
    if (x + w > innerWidth - 8) x = Math.max(8, innerWidth - w - 8);
    if (y + hgt > innerHeight - 8) y = Math.max(8, (r ? r.top - 6 : y) - hgt);
    root.style.left = `${x}px`;
    root.style.top = `${y}px`;
  }
  requestAnimationFrame(() => {
    root.classList.add("in");
    if (!isPhone) buttons[0]?.focus({ preventScroll: true });
  });
  document.addEventListener("keydown", keys, true);
  document.addEventListener("pointerdown", away, true);
}

export const closeMenu = () => open?.close();

export const menuIsOpen = () => open !== null;

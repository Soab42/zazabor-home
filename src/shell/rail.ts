import { clear, h } from "../ui/dom";
import { icon, type IconName } from "../ui/icons";
import { closeMenu, openMenu } from "../ui/menu";
import type { ModuleId, RailStatus } from "./module";
import { MODULES } from "./registry";

export interface RailHandlers {
  select(id: ModuleId): void;
  settings(): void;
  /** The user guide (the ? above Settings). */
  help(): void;
  /** Shows or hides the terminal drawer (the button above the ?). */
  drawer(): void;
  /** Opens the module in a new window (right-click → Open in New Window, or a middle click). */
  openWindow(id: ModuleId): void;
  /** Moves the section to this place among the visible ones (a drag, or Alt+Up/Down). */
  reorder(id: ModuleId, to: number): void;
  /** Puts the sections back in their built-in order. */
  resetOrder(): void;
  /** Turns the section's feature off (right-click → Hide from sidebar). */
  hide(id: ModuleId): void;
  /** The fold button: icons only, or names too. */
  toggleExpanded(): void;
}

/** A pointer must move this far before a press on a section becomes a drag. */
const DRAG_THRESHOLD = 5;

/**
 * The glass rail on the far left: one button per enabled module, Settings at the bottom. It is
 * icons only (64 px) or, expanded, icons with the names (the fold button at its foot).
 */
export class Rail {
  readonly el = h("nav", { class: "rail", "aria-label": "Modules" });
  private items = h("div", { class: "rail-items" });
  private buttons = new Map<ModuleId, HTMLButtonElement>();
  private statuses = new Map<ModuleId, RailStatus>();
  private expanded = false;
  private drawerOpen = false;
  private drawerNotice: "ok" | "failed" | null = null;
  private settingsButton = this.fixedButton("Settings", "settings", "Settings (Ctrl+,)");
  private helpButton = this.fixedButton("Help & Guide", "help", "Help & Guide (F1)", "rail-help");
  private drawerButton = this.fixedButton("Terminal Drawer", "terminal", "Terminal Drawer (Ctrl+`)");
  private foldButton = this.fixedButton("Expand", "sidebar", "Show names (Ctrl+Alt+B)", "rail-fold");
  /** The line that shows where a dragged section would land. */
  private dropLine = h("div", { class: "rail-drop", hidden: true });
  private drag: { id: ModuleId; button: HTMLButtonElement; startY: number; started: boolean; target: number } | null = null;
  /** A drag just ended on a button: the click that follows it is not a choice of that section. */
  private dragged = false;

  constructor(private readonly handlers: RailHandlers) {
    const mark = h(
      "div",
      { class: "rail-mark", title: "Zazabor Home" },
      h("img", { src: "/logo.svg", alt: "Zazabor Home", width: 30, height: 30, draggable: false }),
      h("span", { class: "rail-mark-name" }, "Zazabor Home"),
    );
    this.settingsButton.append(h("span", { class: "rail-pill" }));
    this.settingsButton.addEventListener("click", () => handlers.settings());
    this.helpButton.addEventListener("click", () => handlers.help());
    this.drawerButton.addEventListener("click", () => handlers.drawer());
    this.foldButton.addEventListener("click", () => handlers.toggleExpanded());
    this.el.append(mark, this.items, h("div", { class: "spacer" }), this.drawerButton, this.helpButton, this.settingsButton, this.foldButton);
    this.setExpanded(false);
  }

  /** One of the buttons at the bottom: an icon and, when the rail is wide, a name. */
  private fixedButton(name: string, glyph: IconName, title: string, extra = "") {
    const button = h("button", { class: `rail-btn ${extra}`.trim(), type: "button", "aria-label": name }, icon(glyph, 20), h("span", { class: "rail-label" }, name));
    button.dataset.title = title;
    button.title = title;
    return button;
  }

  /** Shows the names beside the icons; tooltips are for the icons-only rail. */
  setExpanded(on: boolean) {
    this.expanded = on;
    this.el.classList.toggle("expanded", on);
    this.foldButton.setAttribute("aria-expanded", String(on));
    this.foldButton.querySelector(".rail-label")!.textContent = on ? "Collapse" : "Expand";
    this.foldButton.setAttribute("aria-label", on ? "Collapse sidebar" : "Expand sidebar");
    this.foldButton.dataset.title = on ? "Show icons only (Ctrl+Alt+B)" : "Show names (Ctrl+Alt+B)";
    for (const button of [this.settingsButton, this.helpButton, this.foldButton]) button.title = on ? "" : button.dataset.title ?? "";
    this.setDrawer(this.drawerOpen, this.drawerNotice);
    for (const id of this.buttons.keys()) this.showStatus(id);
  }

  /** Rebuilds the buttons for the enabled modules, in order (Ctrl+1 is the first). */
  setItems(ids: ModuleId[]) {
    const focused = [...this.buttons].find(([, button]) => button === document.activeElement)?.[0];
    clear(this.items);
    this.buttons.clear();
    this.items.append(this.dropLine);
    ids.forEach((id) => {
      const meta = MODULES[id];
      const button = h(
        "button",
        { class: "rail-btn", type: "button", "aria-label": meta.title },
        icon(meta.icon, 20),
        h("span", { class: "rail-label" }, meta.title),
        h("span", { class: "rail-pill" }),
      );
      button.addEventListener("click", () => {
        // The click that ends a drag is not a choice.
        if (this.dragged) this.dragged = false;
        else this.handlers.select(id);
      });
      button.addEventListener("pointerdown", (e) => this.beginDrag(e, id, button, ids));
      button.addEventListener("keydown", (e) => {
        if (!e.altKey || e.ctrlKey || e.metaKey || (e.key !== "ArrowUp" && e.key !== "ArrowDown")) return;
        e.preventDefault();
        const at = ids.indexOf(id) + (e.key === "ArrowUp" ? -1 : 1);
        if (at >= 0 && at < ids.length) this.handlers.reorder(id, at);
      });
      button.addEventListener("contextmenu", (e) => {
        e.preventDefault();
        const at = ids.indexOf(id);
        openMenu(e.clientX, e.clientY, [
          { label: meta.newWindow?.label ?? "Open in New Window", icon: meta.newWindow ? "plus" : "external", action: () => this.handlers.openWindow(id) },
          "separator",
          { label: "Move Up", icon: "chevronUp", disabled: at <= 0, action: () => this.handlers.reorder(id, at - 1) },
          { label: "Move Down", icon: "chevronDown", disabled: at >= ids.length - 1, action: () => this.handlers.reorder(id, at + 1) },
          { label: "Reset Order", icon: "refresh", action: () => this.handlers.resetOrder() },
          "separator",
          { label: "Hide from Sidebar", icon: "eyeOff", disabled: ids.length <= 1, action: () => this.handlers.hide(id) },
        ]);
      });
      button.addEventListener("auxclick", (e) => {
        if (e.button === 1) this.handlers.openWindow(id);
      });
      this.buttons.set(id, button);
      this.items.append(button);
      this.showStatus(id);
    });
    if (focused) this.buttons.get(focused)?.focus();
  }

  // ---- reordering by dragging

  private beginDrag(e: PointerEvent, id: ModuleId, button: HTMLButtonElement, ids: ModuleId[]) {
    if (e.button !== 0 || ids.length < 2) return;
    this.drag = { id, button, startY: e.clientY, started: false, target: ids.indexOf(id) };
    const move = (ev: PointerEvent) => this.dragMove(ev);
    const finish = (commit: boolean) => {
      window.removeEventListener("pointermove", move, true);
      window.removeEventListener("pointerup", up, true);
      window.removeEventListener("pointercancel", cancel, true);
      window.removeEventListener("keydown", key, true);
      const drag = this.drag;
      this.drag = null;
      if (!drag?.started) return;
      this.dragged = true;
      setTimeout(() => (this.dragged = false), 0);
      drag.button.classList.remove("dragging");
      drag.button.style.transform = "";
      this.items.classList.remove("dragging");
      this.dropLine.hidden = true;
      if (commit && drag.target !== ids.indexOf(drag.id)) this.handlers.reorder(drag.id, drag.target);
    };
    const up = () => finish(true);
    const cancel = () => finish(false);
    const key = (ev: KeyboardEvent) => {
      if (ev.key !== "Escape") return;
      ev.stopPropagation();
      finish(false);
    };
    window.addEventListener("pointermove", move, true);
    window.addEventListener("pointerup", up, true);
    window.addEventListener("pointercancel", cancel, true);
    window.addEventListener("keydown", key, true);
  }

  private dragMove(e: PointerEvent) {
    const drag = this.drag;
    if (!drag) return;
    const dy = e.clientY - drag.startY;
    if (!drag.started) {
      if (Math.abs(dy) < DRAG_THRESHOLD) return;
      drag.started = true;
      drag.button.classList.add("dragging");
      this.items.classList.add("dragging");
      closeMenu();
    }
    drag.button.style.transform = `translateY(${dy}px)`;
    // Where it would land: after every other section whose middle is above the pointer.
    const others = [...this.buttons].filter(([id]) => id !== drag.id).map(([, button]) => button);
    const above = others.filter((b) => {
      const r = b.getBoundingClientRect();
      return r.top + r.height / 2 < e.clientY;
    }).length;
    drag.target = above;
    const box = this.items.getBoundingClientRect();
    const edge = above < others.length ? others[above].getBoundingClientRect().top - 3 : (others.at(-1)?.getBoundingClientRect().bottom ?? box.top) + 3;
    this.dropLine.style.top = `${edge - box.top + this.items.scrollTop - 1}px`;
    this.dropLine.hidden = false;
  }

  /** Puts a dot on a module's button (or takes it off), with a line for its tooltip. */
  setStatus(id: ModuleId, status: RailStatus | null) {
    // The same status again changes nothing (and must not rebuild the dot, which restarts its pulse).
    const before = this.statuses.get(id);
    if (before?.tone === status?.tone && before?.title === status?.title && before?.count === status?.count) return;
    if (status) this.statuses.set(id, status);
    else this.statuses.delete(id);
    this.showStatus(id);
  }

  private showStatus(id: ModuleId) {
    const button = this.buttons.get(id);
    if (!button) return;
    const status = this.statuses.get(id);
    const index = [...this.buttons.keys()].indexOf(id);
    // Names are shown when the rail is wide, so the tooltip is for the icons-only rail (and for a status line).
    const lines = [this.expanded ? "" : `${MODULES[id].title}  Ctrl+${index + 1}`, status?.title ?? ""].filter(Boolean);
    button.title = lines.join("\n");
    button.querySelector(".rail-dot, .rail-count")?.remove();
    if (status?.count) button.append(h("span", { class: "rail-count" }, status.count > 99 ? "99+" : String(status.count)));
    else if (status) button.append(h("span", { class: `rail-dot ${status.tone}` }));
  }

  /** Marks the module whose workspace is showing; `null` while Settings has the workspace. */
  setActive(id: ModuleId | null) {
    for (const [key, button] of this.buttons) button.classList.toggle("active", key === id);
  }

  setSettingsActive(on: boolean) {
    this.settingsButton.classList.toggle("active", on);
  }

  setHelpActive(on: boolean) {
    this.helpButton.classList.toggle("active", on);
  }

  /** The drawer's button: lit while it shows, with a dot when a command finished out of sight. */
  setDrawer(open: boolean, notice: "ok" | "failed" | null) {
    this.drawerOpen = open;
    this.drawerNotice = notice;
    this.drawerButton.classList.toggle("on", open);
    this.drawerButton.querySelector(".rail-dot")?.remove();
    if (notice) this.drawerButton.append(h("span", { class: `rail-dot ${notice === "failed" ? "failed" : "done"}` }));
    const line = notice ? `A command ${notice === "failed" ? "failed" : "finished"} in the drawer` : "";
    this.drawerButton.title = [this.expanded ? "" : "Terminal Drawer (Ctrl+`)", line].filter(Boolean).join("\n");
  }
}


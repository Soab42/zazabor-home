import { h } from "../../../ui/dom";

export interface ToastAction {
  label: string;
  run: () => void;
}

let current: { el: HTMLElement; timer: number } | null = null;
/** Where toasts go instead of the bottom of the window (setToastAnchor). */
let anchor: HTMLElement | null = null;

/**
 * Shows toasts inside `el` (null: the bottom of the window again). Messages uses its account bar:
 * a native chat panel draws above the page, so a toast at the bottom would sit under it, and
 * hiding the panel for each toast made the chat look frozen for seconds.
 */
export function setToastAnchor(el: HTMLElement | null) {
  anchor = el;
}

function dismiss(toast: { el: HTMLElement; timer: number }) {
  clearTimeout(toast.timer);
  toast.el.classList.add("leaving");
  const remove = () => toast.el.remove();
  toast.el.addEventListener("animationend", remove, { once: true });
  setTimeout(remove, 400);
  if (current === toast) current = null;
}

/** A brief glass message at the bottom of the window, with an optional action ("Undo"). */
export function showToast(message: string, action?: ToastAction, ms = 5000) {
  if (current) dismiss(current);
  const el = h("div", { class: "ops-toast", role: "status" }, h("span", { class: "ops-toast-text" }, message));
  const toast = { el, timer: 0 };
  const arm = () => {
    clearTimeout(toast.timer);
    toast.timer = window.setTimeout(() => dismiss(toast), ms);
  };
  if (action) {
    el.append(h("button", {
      class: "ops-toast-action", type: "button",
      onclick: () => {
        dismiss(toast);
        action.run();
      },
    }, action.label));
  }
  // Resting the pointer on a toast with a button holds it (to reach the button). A plain message
  // always goes on its own: it can cover a native panel, which stays hidden while it shows.
  if (action) {
    el.addEventListener("pointerenter", () => clearTimeout(toast.timer));
    el.addEventListener("pointerleave", arm);
  }
  if (anchor?.isConnected && anchor.offsetParent !== null) {
    el.classList.add("anchored");
    anchor.append(el);
  } else {
    document.body.append(el);
  }
  current = toast;
  arm();
}

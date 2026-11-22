import { alertsApi } from "./api";

let mounted = false;

/**
 * Ctrl+Alt+A answers and Ctrl+Alt+D declines the ringing call while a Zazabor window has the
 * keyboard. (On X11 the backend also grabs them system-wide while a call rings, and then these
 * never arrive; on Wayland this is all there is.) The backend ignores them when nothing rings.
 */
export function mountAlertKeys() {
  if (mounted) return;
  mounted = true;
  window.addEventListener("keydown", (e) => {
    if (!e.ctrlKey || !e.altKey || e.shiftKey || e.metaKey) return;
    const key = e.key.toLowerCase();
    if (key !== "a" && key !== "d") return;
    e.preventDefault();
    void alertsApi.hotkey(key === "a" ? "answer" : "decline").catch(() => undefined);
  });
}

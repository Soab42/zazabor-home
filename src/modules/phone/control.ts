import { listen } from "@tauri-apps/api/event";
import { h } from "../../ui/dom";
import { icon, type IconName } from "../../ui/icons";
import { errorText, phoneApi, type ControlAction } from "./api";

/**
 * Controlling the phone from its mirror window (docs/plans/remote.md): once the phone's user
 * allowed it (Zazabor Link's accessibility service does the touching), a click is a tap, a drag a
 * swipe, a long press a long press, the wheel scrolls, right-click is Back and middle-click Home,
 * typing goes into the focused field, and a small toolbar has the phone's buttons. Every position
 * is a fraction of the phone's screen, so the window can be any size.
 */

/** Where the picture sits inside the stage, in the stage's pixels. */
export interface Fit {
  left: number;
  top: number;
  width: number;
  height: number;
}

/** The picture letterboxed into a box of `boxW`×`boxH`. */
export function fitPicture(boxW: number, boxH: number, picW: number, picH: number): Fit {
  if (!picW || !picH || !boxW || !boxH) return { left: 0, top: 0, width: boxW, height: boxH };
  const s = Math.min(boxW / picW, boxH / picH);
  const width = picW * s;
  const height = picH * s;
  return { left: (boxW - width) / 2, top: (boxH - height) / 2, width, height };
}

/** A point in the stage as a fraction of the picture (null outside it). */
export function toFraction(fit: Fit, x: number, y: number): [number, number] | null {
  const u = (x - fit.left) / fit.width;
  const v = (y - fit.top) / fit.height;
  if (u < 0 || u > 1 || v < 0 || v > 1) return null;
  return [u, v];
}

/** At most `max` points of a path, always keeping the first and the last. */
export function thin<T>(points: T[], max: number): T[] {
  if (points.length <= max) return points;
  const out: T[] = [];
  for (let i = 0; i < max; i++) out.push(points[Math.round((i * (points.length - 1)) / (max - 1))]);
  return out;
}

/** What one finished mouse gesture means for the phone. */
export function gestureAction(path: [number, number][], ms: number, travelPx: number): ControlAction {
  const [x, y] = path[0];
  if (travelPx < 8) return ms >= 500 ? { k: "long", x, y } : { k: "tap", x, y };
  return { k: "swipe", p: thin(path, 48).map(([px, py]) => [Math.round(px * 10000) / 10000, Math.round(py * 10000) / 10000] as [number, number]), ms: Math.min(5000, Math.max(80, Math.round(ms))) };
}

/** The wheel as a scroll: 100 px of wheel is a quarter of the phone's screen. */
export function wheelAction(at: [number, number], deltaX: number, deltaY: number): ControlAction {
  return { k: "scroll", x: at[0], y: at[1], dx: Math.max(-1, Math.min(1, deltaX / 400)), dy: Math.max(-1, Math.min(1, deltaY / 400)) };
}

const TOOLS: { icon: IconName; title: string; action: ControlAction }[] = [
  { icon: "back", title: "Back", action: { k: "global", a: "back" } },
  { icon: "home", title: "Home", action: { k: "global", a: "home" } },
  { icon: "grid", title: "Recent apps", action: { k: "global", a: "recents" } },
  { icon: "bell", title: "Notifications", action: { k: "global", a: "notifications" } },
  { icon: "settings", title: "Quick settings", action: { k: "global", a: "quick" } },
  { icon: "minus", title: "Volume down", action: { k: "volume", d: "down" } },
  { icon: "plus", title: "Volume up", action: { k: "volume", d: "up" } },
  { icon: "image", title: "Screenshot", action: { k: "global", a: "screenshot" } },
  { icon: "lock", title: "Lock the phone", action: { k: "global", a: "lock" } },
  { icon: "bolt", title: "Power menu", action: { k: "global", a: "power" } },
];

export interface PhoneControl {
  dispose(): void;
}

/**
 * Adds the "Control" button to the mirror window's tools and, once the phone says yes, the
 * overlay and the toolbar. `picture()` says how big the phone's picture is (0 until it plays).
 */
export function mountPhoneControl(stage: HTMLElement, tools: HTMLElement, phoneId: string, picture: () => { width: number; height: number }, notify: (text: string) => void): PhoneControl {
  let on = false;
  let overlay: HTMLElement | null = null;
  let bar: HTMLElement | null = null;
  let typed = "";
  let typedTimer = 0;
  const unlisten: (() => void)[] = [];

  const send = (action: ControlAction) => void phoneApi.control(phoneId, action).catch((err) => notify(errorText(err)));
  const button = h("button", { class: "icon-btn", type: "button", title: "Control this phone from here", "aria-label": "Control this phone" }, icon("hand", 16));
  tools.prepend(button);

  const fit = () => {
    const box = stage.getBoundingClientRect();
    const pic = picture();
    return fitPicture(box.width, box.height, pic.width, pic.height);
  };
  const at = (e: { clientX: number; clientY: number }) => {
    const box = stage.getBoundingClientRect();
    return toFraction(fit(), e.clientX - box.left, e.clientY - box.top);
  };

  const flushText = () => {
    if (typed) send({ k: "text", s: typed });
    typed = "";
  };
  const type = (s: string) => {
    typed += s;
    clearTimeout(typedTimer);
    typedTimer = window.setTimeout(flushText, 40);
  };

  const onKey = (e: KeyboardEvent) => {
    if (!on || e.metaKey || e.ctrlKey || e.altKey) return;
    const target = e.target as HTMLElement | null;
    if (target?.closest("input, textarea, [contenteditable]")) return;
    const named: Record<string, ControlAction> = {
      Enter: { k: "key", key: "enter" }, Backspace: { k: "key", key: "del" }, ArrowLeft: { k: "key", key: "left" }, ArrowRight: { k: "key", key: "right" },
      ArrowUp: { k: "key", key: "up" }, ArrowDown: { k: "key", key: "down" }, Escape: { k: "global", a: "back" },
    };
    if (named[e.key]) {
      flushText();
      send(named[e.key]);
    } else if (e.key.length === 1 || [...e.key].length === 1) {
      type(e.key);
    } else {
      return;
    }
    // Keys belong to the phone now: the player's own shortcuts stay out of it.
    e.preventDefault();
    e.stopPropagation();
  };

  const start = () => {
    on = true;
    button.classList.add("active");
    button.title = "Stop controlling this phone";
    overlay = h("div", { class: "pc-overlay", tabindex: -1 });
    let down: { x: number; y: number; at: number; path: [number, number][]; id: number } | null = null;
    overlay.addEventListener("contextmenu", (e) => e.preventDefault());
    overlay.addEventListener("pointerdown", (e) => {
      e.preventDefault();
      overlay?.focus();
      if (e.button === 2) return send({ k: "global", a: "back" });
      if (e.button === 1) return send({ k: "global", a: "home" });
      const f = at(e);
      if (!f) return;
      overlay?.setPointerCapture(e.pointerId);
      down = { x: e.clientX, y: e.clientY, at: performance.now(), path: [f], id: e.pointerId };
    });
    overlay.addEventListener("pointermove", (e) => {
      if (!down || e.pointerId !== down.id) return;
      const f = at(e);
      if (f) down.path.push(f);
    });
    const finish = (e: PointerEvent) => {
      if (!down || e.pointerId !== down.id) return;
      const g = down;
      down = null;
      const f = at(e);
      if (f && g.path.length > 1) g.path.push(f);
      send(gestureAction(g.path, performance.now() - g.at, Math.hypot(e.clientX - g.x, e.clientY - g.y)));
    };
    overlay.addEventListener("pointerup", finish);
    overlay.addEventListener("pointercancel", () => (down = null));
    let wheelDx = 0;
    let wheelDy = 0;
    let wheelTimer = 0;
    overlay.addEventListener("wheel", (e) => {
      e.preventDefault();
      const f = at(e);
      if (!f) return;
      wheelDx += e.deltaX;
      wheelDy += e.deltaY;
      clearTimeout(wheelTimer);
      // One scroll for a burst of wheel events: the phone's gesture takes a moment.
      wheelTimer = window.setTimeout(() => {
        send(wheelAction(f, wheelDx, wheelDy));
        wheelDx = wheelDy = 0;
      }, 60);
    }, { passive: false });
    bar = h("div", { class: "pc-bar", role: "toolbar", "aria-label": "Phone buttons" },
      ...TOOLS.map((t) => {
        const b = h("button", { class: "icon-btn", type: "button", title: t.title, "aria-label": t.title }, icon(t.icon, 16));
        b.addEventListener("click", () => send(t.action));
        return b;
      }));
    stage.append(overlay, bar);
    window.addEventListener("keydown", onKey, true);
    overlay.focus();
  };

  const stop = (tell: boolean) => {
    if (!on) return;
    on = false;
    flushText();
    button.classList.remove("active");
    button.title = "Control this phone from here";
    window.removeEventListener("keydown", onKey, true);
    overlay?.remove();
    bar?.remove();
    overlay = bar = null;
    if (tell) void phoneApi.controlStop(phoneId).catch(() => undefined);
  };

  button.addEventListener("click", () => {
    if (on) return stop(true);
    button.disabled = true;
    notify("Waiting for you to allow it on the phone…");
    void phoneApi.controlStart(phoneId).then(start, (err) => notify(errorText(err))).finally(() => (button.disabled = false));
  });

  // The phone says control ended (its Stop button, the link dropped, the service was turned off).
  void listen<{ phoneId: string; on: boolean; reason?: string }>("phone-control-state", (e) => {
    if (e.payload.phoneId !== phoneId || e.payload.on) return;
    if (on) notify(e.payload.reason ? `Control ended: ${e.payload.reason}` : "Control ended");
    stop(false);
  }).then((un) => unlisten.push(un));

  return {
    dispose() {
      stop(true);
      unlisten.forEach((un) => un());
    },
  };
}

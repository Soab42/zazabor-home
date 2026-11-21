import { getCurrentWindow, currentMonitor, LogicalSize, PhysicalPosition, PhysicalSize } from "@tauri-apps/api/window";
import { basename, h, stripExtension } from "../../../../ui/dom";
import { icon } from "../../../../ui/icons";
import type { MenuItem } from "../../../../ui/menu";
import { MAX_VOLUME } from "../../api";
import type { MenuPlace, VideoContext, VideoFeature } from "../../extensions";
import { isFocusClick } from "../../focus-click";
import { formatTime } from "../../format";
import { Library } from "../library/store";
import "./mini.css";

/**
 * The mini player (docs/design/vlc/PLAYER.md, frame 970:3091): the window shrinks to just the
 * picture, always on top, at the video's own aspect ratio. Moving the pointer over it fades in
 * glass controls (title, previous / play / next, a thin seek bar along the bottom edge, and in the
 * corners: close, pin, volume, back to the full player); they fade out when the pointer rests.
 * Drag the picture to move the window; double-click it (or press Escape / Ctrl+M) to return.
 */

/** Matches `minWidth` / `minHeight` in tauri.conf.json: the window API cannot read the minimum back. */
const NORMAL_MIN = { width: 860, height: 560 };
/** The mini window is only the picture: 420 wide for a landscape one (420×236 for 16:9), 420 tall for a portrait one. */
const MINI_LONG = 420;
const MINI_MIN = 200;
const MARGIN = 28;
/** The controls fade out this long after the pointer last moved over the picture. */
const IDLE_MS = 1500;
const DRAG_THRESHOLD = 4;

interface Saved {
  size: PhysicalSize;
  position: PhysicalPosition;
  alwaysOnTop: boolean;
}

export function createMiniFeature(ctx: VideoContext): VideoFeature {
  const { player } = ctx;
  const root = ctx.stageLayer.closest<HTMLElement>(".vp");
  const lib = Library.of(ctx.shell, player);
  let active = false;
  let busy = false;
  let saved: Saved | null = null;
  let nativeDragBroken = false;
  let clickTimer = 0;
  let watchdog = 0;
  let pinned = true;
  let shownTimer = 0;
  let resizeTimer = 0;
  let unlistenResize: (() => void) | null = null;

  // ---- the controls over the picture ------------------------------------------------------------

  const titleEl = h("div", { class: "vp-mini-title" });
  const timeEl = h("div", { class: "vp-mini-time" });
  const seekFill = h("div", { class: "vp-mini-seek-fill" });
  const seekThumb = h("div", { class: "vp-mini-seek-thumb" });
  const seek = h("div", { class: "vp-mini-seek", role: "slider", "aria-label": "Seek", tabIndex: 0 }, seekFill, seekThumb);
  const iconButton = (name: Parameters<typeof icon>[0], label: string, onClick: () => void, extra = "") => {
    const b = h("button", { class: `vp-mini-btn${extra}`, type: "button", "aria-label": label }, icon(name, extra.includes("primary") ? 22 : 15));
    b.addEventListener("click", onClick);
    return b;
  };
  const prev = iconButton("prev", "Previous", () => player.previous(), " side");
  const play = iconButton("play", "Play / Pause", () => player.togglePlay(), " primary");
  const next = iconButton("next", "Next", () => player.next(), " side");
  const closeButton = iconButton("close", "Close the mini player and stop", () => void closeAndStop());
  const pinButton = iconButton("pin", "Keep on top", () => void togglePin(), " active");
  const returnButton = iconButton("fullscreen", "Back to the full player (double-click the picture)", () => void exit());
  const muteButton = iconButton("volume", "Mute", () => player.toggleMute());
  const volume = h("input", { class: "vp-mini-volume", type: "range", min: 0, max: MAX_VOLUME * 100, step: 1, "aria-label": "Volume" });
  volume.addEventListener("input", () => player.setVolume(Number(volume.value) / 100));
  const volumeBox = h("div", { class: "vp-mini-vol" }, muteButton, volume);
  const dragLayer = h("div", { class: "vp-mini-drag"});
  const chrome = h("div", { class: "vp-mini-chrome" },
    h("div", { class: "vp-mini-top" },
      h("div", { class: "vp-mini-corner" }, closeButton, pinButton),
      h("div", { class: "vp-mini-corner" }, volumeBox, returnButton)),
    h("div", { class: "vp-mini-mid" }, prev, play, next),
    h("div", { class: "vp-mini-bottom" }, h("div", { class: "vp-mini-row" }, titleEl, timeEl), seek));
  const overlay = h("div", { class: "vp-mini", hidden: true }, dragLayer, chrome);
  root?.append(overlay);

  /** Fades the controls in; they fade out `IDLE_MS` after the pointer last moved (or at once on leaving). */
  function showChrome() {
    overlay.classList.add("shown");
    clearTimeout(shownTimer);
    shownTimer = window.setTimeout(hideChrome, IDLE_MS);
  }
  function hideChrome() {
    clearTimeout(shownTimer);
    // The pointer is still on a control being used: keep them until it lets go.
    if (seek.classList.contains("dragging") || document.activeElement === volume) {
      shownTimer = window.setTimeout(hideChrome, IDLE_MS);
      return;
    }
    overlay.classList.remove("shown");
  }
  overlay.addEventListener("pointermove", showChrome);
  overlay.addEventListener("pointerdown", showChrome);
  overlay.addEventListener("pointerleave", () => {
    clearTimeout(shownTimer);
    shownTimer = window.setTimeout(hideChrome, 120);
  });
  chrome.addEventListener("focusin", showChrome);

  async function togglePin() {
    pinned = !pinned;
    pinButton.classList.toggle("active", pinned);
    try {
      await getCurrentWindow().setAlwaysOnTop(pinned);
    } catch (err) {
      console.warn("Could not change always-on-top:", err);
    }
  }

  async function closeAndStop() {
    await exit();
    await player.stop();
  }

  // ---- seeking ----------------------------------------------------------------------------------

  const seekFrom = (e: PointerEvent) => {
    const r = seek.getBoundingClientRect();
    const ratio = Math.min(1, Math.max(0, (e.clientX - r.left) / r.width));
    if (player.state.length > 0) player.seekTo(ratio * player.state.length);
  };
  seek.addEventListener("pointerdown", (e) => {
    seek.setPointerCapture(e.pointerId);
    seek.classList.add("dragging");
    seekFrom(e);
  });
  seek.addEventListener("pointermove", (e) => {
    if (seek.hasPointerCapture(e.pointerId)) seekFrom(e);
  });
  const endSeek = (e: PointerEvent) => {
    if (seek.hasPointerCapture(e.pointerId)) seek.releasePointerCapture(e.pointerId);
    seek.classList.remove("dragging");
  };
  seek.addEventListener("pointerup", endSeek);
  seek.addEventListener("pointercancel", endSeek);
  seek.addEventListener("keydown", (e) => {
    if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
      player.seekBy((e.key === "ArrowLeft" ? -1 : 1) * 5000);
      e.preventDefault();
      e.stopPropagation();
    }
  });

  // ---- the picture: click, double-click, drag ---------------------------------------------------

  dragLayer.addEventListener("click", () => {
    if (isFocusClick()) return;
    clearTimeout(clickTimer);
    clickTimer = window.setTimeout(() => player.togglePlay(), 230);
  });
  dragLayer.addEventListener("dblclick", () => {
    clearTimeout(clickTimer);
    void exit();
  });
  dragLayer.addEventListener("pointerdown", (down) => {
    if (down.button !== 0) return;
    const start = { x: down.screenX, y: down.screenY };
    let moved = false;
    let origin: PhysicalPosition | null = null;
    let frame = 0;
    const win = getCurrentWindow();
    const dpr = window.devicePixelRatio || 1;
    void win.outerPosition().then((p) => (origin = p)).catch(() => undefined);

    const move = (e: PointerEvent) => {
      const dx = e.screenX - start.x;
      const dy = e.screenY - start.y;
      if (!moved) {
        if (Math.hypot(dx, dy) < DRAG_THRESHOLD) return;
        moved = true;
        clearTimeout(clickTimer);
        if (!nativeDragBroken) {
          // The window manager takes over the pointer from here; if the permission is missing
          // (core:window:allow-start-dragging), fall back to moving the window ourselves.
          win.startDragging().then(() => cleanup(), () => (nativeDragBroken = true));
          return;
        }
      }
      if (!nativeDragBroken || !origin) return;
      const target = origin;
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        void win.setPosition(new PhysicalPosition(Math.round(target.x + dx * dpr), Math.round(target.y + dy * dpr))).catch(() => undefined);
      });
    };
    const up = () => {
      cleanup();
      if (moved) {
        // A drag must not count as a click.
        const swallow = (c: Event) => c.stopPropagation();
        dragLayer.addEventListener("click", swallow, { capture: true, once: true });
        setTimeout(() => dragLayer.removeEventListener("click", swallow, true), 0);
      }
    };
    const cleanup = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", up);
  });

  // ---- state ------------------------------------------------------------------------------------

  let shownKey = "";
  function update() {
    if (!active) return;
    if (!player.current) {
      void exit();
      return;
    }
    const { state } = player;
    const path = player.current;
    const key = path;
    if (key !== shownKey) {
      shownKey = key;
      titleEl.textContent = lib.label(path) || stripExtension(basename(path));
    }
    fitToPicture();
    play.replaceChildren(icon(state.playing ? "pause" : "play", 22));
    muteButton.replaceChildren(icon(state.muted || state.volume === 0 ? "mute" : "volume", 15));
    prev.disabled = !player.hasPrevious;
    next.disabled = !player.hasNext;
    const ratio = state.length > 0 ? Math.min(1, state.time / state.length) : 0;
    seekFill.style.width = `${ratio * 100}%`;
    seekThumb.style.left = `${ratio * 100}%`;
    timeEl.textContent = state.length > 0 ? `${formatTime(state.time)} / ${formatTime(state.length)}` : formatTime(state.time);
    seek.setAttribute("aria-valuenow", String(Math.round(ratio * 100)));
    if (document.activeElement !== volume) volume.value = String(Math.round(state.volume * 100));
  }
  const unsubscribe = player.subscribe(update);

  // ---- entering and leaving ---------------------------------------------------------------------

  async function leaveFullscreen() {
    if (!ctx.fullscreen) return;
    // The module owns fullscreen; Escape is how it is left.
    ctx.stageLayer.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    for (let i = 0; i < 20 && ctx.fullscreen; i++) await new Promise((r) => setTimeout(r, 50));
  }

  /** The window size for the picture's aspect ratio (16:9 until the first frame says otherwise). */
  function miniSize(width = player.state.videoWidth, height = player.state.videoHeight) {
    const aspect = width > 0 && height > 0 ? width / height : 16 / 9;
    return aspect >= 1
      ? { width: MINI_LONG, height: Math.round(MINI_LONG / aspect), aspect }
      : { width: Math.round(MINI_LONG * aspect), height: MINI_LONG, aspect };
  }

  /** What aspect the window has now; a change of picture (next video, first frame) refits it. */
  let fitted = "";
  function fitToPicture() {
    const { videoWidth, videoHeight } = player.state;
    const key = `${videoWidth}x${videoHeight}`;
    if (key === fitted) return;
    fitted = key;
    if (videoWidth > 0 && videoHeight > 0) void applySize(miniSize());
  }

  async function applySize(size: { width: number; height: number; aspect: number }) {
    const win = getCurrentWindow();
    const min = size.aspect >= 1 ? { width: MINI_MIN, height: Math.round(MINI_MIN / size.aspect) } : { width: Math.round(MINI_MIN * size.aspect), height: MINI_MIN };
    try {
      await win.setMinSize(new LogicalSize(min.width, min.height));
      await win.setSize(new LogicalSize(size.width, size.height));
    } catch (err) {
      console.warn("Could not size the mini player:", err);
    }
  }

  /** Dragging an edge keeps the picture's aspect ratio: the height follows the width. */
  function keepAspect() {
    clearTimeout(resizeTimer);
    resizeTimer = window.setTimeout(async () => {
      if (!active || busy) return;
      const win = getCurrentWindow();
      const scale = (await win.scaleFactor()) || 1;
      const inner = await win.innerSize();
      const { aspect } = miniSize();
      const width = inner.width / scale;
      const height = inner.height / scale;
      const wanted = aspect >= 1 ? { width, height: Math.round(width / aspect) } : { width: Math.round(height * aspect), height };
      if (Math.abs(wanted.width - width) > 1.5 || Math.abs(wanted.height - height) > 1.5) {
        await win.setSize(new LogicalSize(wanted.width, wanted.height)).catch(() => undefined);
      }
    }, 140);
  }

  async function enter() {
    if (active || busy || !root) return;
    if (!player.current) {
      ctx.showOsd("Play something to use the mini player");
      return;
    }
    busy = true;
    try {
      await leaveFullscreen();
      if (ctx.fullscreen) {
        ctx.showOsd("Leave fullscreen first");
        return;
      }
      const win = getCurrentWindow();
      if (await win.isMaximized()) {
        try {
          await win.unmaximize();
        } catch {
          ctx.showOsd("Restore the window size to use the mini player");
          return;
        }
      }
      saved = { size: await win.innerSize(), position: await win.outerPosition(), alwaysOnTop: await win.isAlwaysOnTop() };
      ctx.closePanel();
      ctx.showPlayer();
      fitted = `${player.state.videoWidth}x${player.state.videoHeight}`;
      const size = miniSize();
      await applySize(size);
      pinned = true;
      pinButton.classList.add("active");
      await win.setAlwaysOnTop(true);
      await moveToCorner(win, size);
      unlistenResize = await win.onResized(keepAspect);
      active = true;
      shownKey = "";
      root.classList.add("mini");
      overlay.hidden = false;
      overlay.classList.remove("shown");
      update();
      watchdog = window.setInterval(() => {
        // The module was hidden (another app module took over): the mini window goes with it.
        if (active && root.getClientRects().length === 0) void exit();
      }, 600);
    } catch (err) {
      console.warn("Mini player failed:", err);
      await restoreWindow();
      ctx.showOsd("The mini player couldn't start");
    } finally {
      busy = false;
    }
  }

  async function moveToCorner(win: ReturnType<typeof getCurrentWindow>, size: { width: number; height: number }) {
    try {
      const monitor = await currentMonitor();
      if (!monitor) return;
      const scale = monitor.scaleFactor || 1;
      const x = monitor.position.x + monitor.size.width - Math.round((size.width + MARGIN) * scale);
      const y = monitor.position.y + monitor.size.height - Math.round((size.height + MARGIN * 2.5) * scale);
      await win.setPosition(new PhysicalPosition(Math.max(monitor.position.x, x), Math.max(monitor.position.y, y)));
    } catch (err) {
      console.warn("Could not place the mini player:", err);
    }
  }

  /** Puts the window back exactly as it was: always-on-top, minimum size, size and position. */
  async function restoreWindow() {
    const win = getCurrentWindow();
    const before = saved;
    saved = null;
    try {
      await win.setAlwaysOnTop(before?.alwaysOnTop ?? false);
      await win.setMinSize(new LogicalSize(NORMAL_MIN.width, NORMAL_MIN.height));
      if (before) {
        await win.setSize(before.size);
        await win.setPosition(before.position);
      }
    } catch (err) {
      console.warn("Could not restore the window:", err);
    }
  }

  async function exit() {
    if (!active || busy) return;
    busy = true;
    try {
      active = false;
      clearInterval(watchdog);
      clearTimeout(clickTimer);
      clearTimeout(shownTimer);
      clearTimeout(resizeTimer);
      unlistenResize?.();
      unlistenResize = null;
      overlay.hidden = true;
      overlay.classList.remove("shown");
      root?.classList.remove("mini");
      await restoreWindow();
    } finally {
      busy = false;
    }
  }

  const toggle = () => (active ? exit() : enter());

  return {
    menuItems(place: MenuPlace): MenuItem[] {
      if (place === "open") return [];
      return [{ label: active ? "Exit Mini Player" : "Mini Player", icon: "pip", action: () => void toggle() }];
    },
    handleKey(e: KeyboardEvent): boolean {
      if ((e.ctrlKey || e.metaKey) && !e.altKey && !e.shiftKey && e.key.toLowerCase() === "m") {
        void toggle();
        return true;
      }
      if (active && e.key === "Escape" && !document.querySelector(".menu")) {
        void exit();
        return true;
      }
      return false;
    },
    dispose() {
      unsubscribe();
      clearInterval(watchdog);
      clearTimeout(clickTimer);
      clearTimeout(shownTimer);
      clearTimeout(resizeTimer);
      unlistenResize?.();
      if (active) {
        active = false;
        root?.classList.remove("mini");
        void restoreWindow();
      }
      overlay.remove();
    },
  };
}

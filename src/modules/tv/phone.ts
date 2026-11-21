/**
 * The phone side of TV. Zazabor TV for Android (`mobile-tv/`, entry `mobile-tv.html`) runs this same
 * section with `class="phone"` on <html>: the desktop's hover tilt, arrow-key focus and top pill
 * give way to a bottom tab bar, touch scrolling and pull-to-refresh. Everything here is only used
 * when `isPhone` is true, so the desktop is unchanged.
 */
export const isPhone = document.documentElement.classList.contains("phone");

/**
 * Pull down from the top of `scroll` to refresh: a small spinner follows the finger and `refresh`
 * runs when it is pulled far enough. `ready` says whether pulling applies right now (Home only).
 */
export function pullToRefresh(scroll: HTMLElement, indicator: HTMLElement, ready: () => boolean, refresh: () => void) {
  const LIMIT = 84;
  let startY = 0;
  let pull = 0;
  let tracking = false;
  const set = (px: number, settle = false) => {
    indicator.style.transition = settle ? "transform 0.25s ease, opacity 0.25s" : "none";
    indicator.style.transform = `translate3d(-50%, ${px - 44}px, 0)`;
    indicator.style.opacity = String(Math.min(1, px / 50));
    indicator.classList.toggle("armed", px >= LIMIT);
  };
  scroll.addEventListener("touchstart", (e) => {
    tracking = ready() && scroll.scrollTop <= 0 && e.touches.length === 1;
    startY = e.touches[0].clientY;
    pull = 0;
  }, { passive: true });
  scroll.addEventListener("touchmove", (e) => {
    if (!tracking) return;
    const dy = e.touches[0].clientY - startY;
    if (dy <= 0 || scroll.scrollTop > 0) {
      if (pull) set(0, true);
      pull = 0;
      return;
    }
    pull = Math.min(dy * 0.5, LIMIT * 1.4);
    set(pull);
  }, { passive: true });
  const end = () => {
    if (!tracking) return;
    tracking = false;
    const go = pull >= LIMIT;
    set(go ? 56 : 0, true);
    if (go) {
      indicator.classList.add("spin");
      refresh();
      window.setTimeout(() => {
        indicator.classList.remove("spin");
        set(0, true);
      }, 1400);
    }
    pull = 0;
  };
  scroll.addEventListener("touchend", end, { passive: true });
  scroll.addEventListener("touchcancel", end, { passive: true });
}

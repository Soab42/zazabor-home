/**
 * A click that only brought the window to the front must not pause or play the video. WebKitGTK
 * may fire the window's `focus` before or after the `pointerdown` of that click, so both orders
 * count: the window wasn't focused at pointerdown, or it became focused just before it.
 */

/** Focus that arrived this recently before a pointerdown belongs to that same click. */
const FOCUS_CLICK_MS = 300;

let focused = document.hasFocus();
let focusedAt = 0;
/** Whether the latest pointerdown only focused the window. */
let lastDownFocused = false;

window.addEventListener("focus", () => {
  focused = true;
  focusedAt = performance.now();
});
window.addEventListener("blur", () => {
  focused = false;
});
document.addEventListener("pointerdown", () => {
  lastDownFocused = !focused || !document.hasFocus() || performance.now() - focusedAt < FOCUS_CLICK_MS;
}, true);

/** For a click handler: true when this click only brought the window to the front. */
export function isFocusClick(): boolean {
  return lastDownFocused;
}

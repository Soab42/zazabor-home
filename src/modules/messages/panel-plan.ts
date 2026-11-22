// The Messages panel's show/hide rules as plain functions (index.ts does the I/O).
// Tested by tools/messages-panel.test.mjs: node --test tools/messages-panel.test.mjs

export interface WantInput {
  /** The section is on screen (Settings and the guide hide it without deactivating it). */
  visible: boolean;
  /** A sheet, menu, toast or the lock screen overlaps the content area. */
  covered: boolean;
  selected: string | null;
  /** The id of the Phone entry, which has no panel. */
  phoneId: string;
  /** The account the user put to sleep while it was selected. */
  napping: string | null;
  /** How the selected account opens; null when there is no such account. */
  mode: "panel" | "browser" | null;
  /** The content area has a usable size. */
  hasRect: boolean;
}

/** The account whose panel must be on screen now, or null when no panel should be. */
export function wantedPanel(i: WantInput): string | null {
  if (!i.visible || i.covered || !i.selected || i.selected === i.phoneId || i.selected === i.napping || !i.hasRect) return null;
  return i.mode === "panel" ? i.selected : null;
}

export type Step =
  /** Show this panel (it isn't the one on screen), at the current rectangle. */
  | { kind: "show"; id: string }
  /** The right panel is up: move or resize it. */
  | { kind: "rect"; id: string }
  /** The right panel is up at the right place: say it again, in case Rust's side drifted. */
  | { kind: "assert"; id: string }
  | { kind: "keep"; id: string }
  /** Hide the panel on screen; `snapshot` leaves a still picture of it under the sheet. */
  | { kind: "hide"; id: string; snapshot: boolean }
  /** Nothing is wanted and nothing is up. */
  | { kind: "idle" };

export interface StepInput {
  want: string | null;
  /** What Rust shows now, as far as the page knows. */
  shownId: string | null;
  rectChanged: boolean;
  /** The periodic re-assertion is due. */
  assertDue: boolean;
  visible: boolean;
  covered: boolean;
  locked: boolean;
}

export function nextStep(i: StepInput): Step {
  if (i.want) {
    if (i.shownId !== i.want) return { kind: "show", id: i.want };
    if (i.rectChanged) return { kind: "rect", id: i.want };
    return i.assertDue ? { kind: "assert", id: i.want } : { kind: "keep", id: i.want };
  }
  if (i.shownId) return { kind: "hide", id: i.shownId, snapshot: i.visible && i.covered && !i.locked };
  return { kind: "idle" };
}

/** The still picture stands in for the panel only while a sheet covers the section. */
export const snapshotShown = (i: { visible: boolean; covered: boolean; locked: boolean }): boolean => i.visible && i.covered && !i.locked;

/** How often the wanted panel is re-asserted to Rust even when nothing changed (ms). */
export const ASSERT_EVERY = 2000;
/** The shortest wait before a failed show is tried again (ms). */
export const RETRY_AFTER = 1000;

/** Whether the poll should run a sync now: something is off, or the re-assertion is due. */
export function pollDue(i: { want: string | null; shownId: string | null; now: number; lastAssert: number; lastFailure: number }): "retry" | "assert" | "check" | "none" {
  if (i.want && i.shownId !== i.want) return i.now - i.lastFailure >= RETRY_AFTER ? "retry" : "none";
  if (i.want && i.now - i.lastAssert >= ASSERT_EVERY) return "assert";
  return i.want || i.shownId ? "check" : "none";
}

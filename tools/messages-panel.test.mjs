// The Messages panel's show/hide rules (src/modules/messages/panel-plan.ts).
// Run: node --test tools/messages-panel.test.mjs   (Node 23.6+ reads the TypeScript directly)
import assert from "node:assert/strict";
import test from "node:test";
import { nextStep, pollDue, snapshotShown, wantedPanel, ASSERT_EVERY, RETRY_AFTER } from "../src/modules/messages/panel-plan.ts";

const base = { visible: true, covered: false, selected: "teams-1", phoneId: "phone", napping: null, mode: "panel", hasRect: true };

test("the selected account's panel is wanted when the section shows with nothing over it", () => {
  assert.equal(wantedPanel(base), "teams-1");
});

test("no panel while the section is hidden, covered, or has no area", () => {
  assert.equal(wantedPanel({ ...base, visible: false }), null);
  assert.equal(wantedPanel({ ...base, covered: true }), null);
  assert.equal(wantedPanel({ ...base, hasRect: false }), null);
});

test("no panel for Phone, a sleeping account, a browser account or a missing one", () => {
  assert.equal(wantedPanel({ ...base, selected: "phone" }), null);
  assert.equal(wantedPanel({ ...base, napping: "teams-1" }), null);
  assert.equal(wantedPanel({ ...base, mode: "browser" }), null);
  assert.equal(wantedPanel({ ...base, mode: null }), null);
  assert.equal(wantedPanel({ ...base, selected: null }), null);
});

const step = (o) => nextStep({ want: null, shownId: null, rectChanged: false, assertDue: false, visible: true, covered: false, locked: false, ...o });

test("a different or missing panel is shown", () => {
  assert.deepEqual(step({ want: "a" }), { kind: "show", id: "a" });
  assert.deepEqual(step({ want: "a", shownId: "b" }), { kind: "show", id: "a" });
});

test("the right panel is moved, re-asserted when due, otherwise left alone", () => {
  assert.deepEqual(step({ want: "a", shownId: "a", rectChanged: true }), { kind: "rect", id: "a" });
  assert.deepEqual(step({ want: "a", shownId: "a", assertDue: true }), { kind: "assert", id: "a" });
  assert.deepEqual(step({ want: "a", shownId: "a" }), { kind: "keep", id: "a" });
});

test("a sheet over the section hides the panel and leaves a picture; leaving the section does not", () => {
  assert.deepEqual(step({ shownId: "a", covered: true }), { kind: "hide", id: "a", snapshot: true });
  assert.deepEqual(step({ shownId: "a", visible: false }), { kind: "hide", id: "a", snapshot: false });
  assert.deepEqual(step({ shownId: "a", covered: true, locked: true }), { kind: "hide", id: "a", snapshot: false });
  assert.deepEqual(step({}), { kind: "idle" });
});

test("the picture shows only under a sheet in a visible, unlocked section", () => {
  assert.equal(snapshotShown({ visible: true, covered: true, locked: false }), true);
  assert.equal(snapshotShown({ visible: true, covered: false, locked: false }), false);
  assert.equal(snapshotShown({ visible: false, covered: true, locked: false }), false);
  assert.equal(snapshotShown({ visible: true, covered: true, locked: true }), false);
});

test("the poll retries a panel that isn't up (after a pause), and re-asserts one that is", () => {
  const t = 100000;
  assert.equal(pollDue({ want: "a", shownId: null, now: t, lastAssert: 0, lastFailure: t - RETRY_AFTER }), "retry");
  assert.equal(pollDue({ want: "a", shownId: null, now: t, lastAssert: 0, lastFailure: t - 10 }), "none");
  assert.equal(pollDue({ want: "a", shownId: "b", now: t, lastAssert: t, lastFailure: 0 }), "retry");
  assert.equal(pollDue({ want: "a", shownId: "a", now: t, lastAssert: t - ASSERT_EVERY, lastFailure: 0 }), "assert");
  assert.equal(pollDue({ want: "a", shownId: "a", now: t, lastAssert: t - 10, lastFailure: 0 }), "check");
  assert.equal(pollDue({ want: null, shownId: "a", now: t, lastAssert: t, lastFailure: 0 }), "check");
  assert.equal(pollDue({ want: null, shownId: null, now: t, lastAssert: 0, lastFailure: 0 }), "none");
});

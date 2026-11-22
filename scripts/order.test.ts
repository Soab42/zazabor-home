// Run with: node --test scripts/order.test.ts
import assert from "node:assert/strict";
import { test } from "node:test";
import { fullOrder, moveTo, normalizeOrder, orderEnabled } from "../src/shell/order.ts";

const ALL = ["files", "gallery", "video", "notes", "mail"];

test("normalizeOrder keeps known ids once, in order", () => {
  assert.deepEqual(normalizeOrder(["mail", "nope", "files", "mail", 3, null], ALL), ["mail", "files"]);
  assert.deepEqual(normalizeOrder("files", ALL), []);
  assert.deepEqual(normalizeOrder(undefined, ALL), []);
});

test("fullOrder appends unknown (new) sections at the end", () => {
  assert.deepEqual(fullOrder(["mail", "files"], ALL), ["mail", "files", "gallery", "video", "notes"]);
  assert.deepEqual(fullOrder([], ALL), ALL);
});

test("orderEnabled follows the saved order and leaves hidden sections out", () => {
  const enabled = ["files", "video", "mail"];
  assert.deepEqual(orderEnabled(enabled, ["mail", "video"], ALL), ["mail", "video", "files"]);
  assert.deepEqual(orderEnabled(enabled, [], ALL), ["files", "video", "mail"]);
});

test("moveTo moves among the visible ones and keeps hidden ones in place", () => {
  const visible = ["files", "video", "mail"];
  const moved = moveTo(visible, "mail", 0, [], ALL);
  assert.deepEqual(orderEnabled(visible, moved, ALL), ["mail", "files", "video"]);
  assert.equal(moved.indexOf("gallery"), 1);
  assert.equal(moved.indexOf("notes"), 3);
  assert.deepEqual(orderEnabled(visible, moveTo(visible, "files", 99, [], ALL), ALL), ["video", "mail", "files"]);
  assert.deepEqual(moveTo(visible, "gallery", 0, ["mail"], ALL), fullOrder(["mail"], ALL));
});

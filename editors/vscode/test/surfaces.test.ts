import assert from "node:assert/strict";
import { test } from "node:test";
import { CHAT_VIEW, chatViewId, lastUsedSurface, parseVersion, supportsSecondarySidebar, tabTitle, versionAtLeast } from "../src/model/surfaces";

test("the right-hand panel needs VS Code 1.106", () => {
  assert.equal(supportsSecondarySidebar("1.139.1"), true, "the user's VS Code");
  assert.equal(supportsSecondarySidebar("1.128.0"), true, "Cursor's VS Code base");
  assert.equal(supportsSecondarySidebar("1.106.0"), true);
  assert.equal(supportsSecondarySidebar("1.106.0-insider"), true);
  assert.equal(supportsSecondarySidebar("1.105.9"), false);
  assert.equal(supportsSecondarySidebar("1.90.2"), false);
  assert.equal(supportsSecondarySidebar("2.0.0"), true);
  assert.equal(supportsSecondarySidebar("weird"), false, "unknown versions keep the activity bar");
  assert.equal(chatViewId("1.139.1"), CHAT_VIEW.secondary);
  assert.equal(chatViewId("1.99.3"), CHAT_VIEW.activityBar);
});

test("versions compare numerically", () => {
  assert.deepEqual(parseVersion("1.106"), [1, 106, 0]);
  assert.equal(parseVersion(""), null);
  assert.ok(versionAtLeast("1.10.0", "1.9.9"));
  assert.ok(!versionAtLeast("1.9.9", "1.10.0"));
  assert.ok(versionAtLeast("1.106.1", "1.106.0"));
});

test("Open brings back the editor tab used last, else the side-bar chat", () => {
  const side = { id: CHAT_VIEW.secondary, kind: "side" as const, open: true, visible: false };
  const tab = { id: "tab-1", kind: "tab" as const, open: true, visible: true };
  assert.equal(lastUsedSurface([side, tab], "tab-1"), "tab-1");
  assert.equal(lastUsedSurface([side, { ...tab, open: false }], "tab-1"), CHAT_VIEW.secondary, "a closed tab falls back");
  assert.equal(lastUsedSurface([side, tab], null), CHAT_VIEW.secondary);
  assert.equal(lastUsedSurface([side, tab], CHAT_VIEW.secondary), CHAT_VIEW.secondary);
  assert.equal(lastUsedSurface([], null), CHAT_VIEW.secondary);
});

test("editor tabs are titled after their agent", () => {
  assert.equal(tabTitle(null), "malac");
  assert.equal(tabTitle({ name: "" }), "malac");
  assert.equal(tabTitle({ name: "fix the\nflaky test" }), "fix the flaky test");
  assert.equal(tabTitle({ name: "a".repeat(40) }), `${"a".repeat(31)}…`);
});

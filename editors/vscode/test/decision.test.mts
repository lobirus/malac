import assert from "node:assert/strict";
import { test } from "node:test";
import { decisionOf, decisionsByCall, decisionText } from "../src/webview/decision.mjs";

test("decisions read as the terminal UI words them, with where they were answered", () => {
  assert.equal(decisionText("allow", "Pixel 9"), "Allowed · Pixel 9");
  assert.equal(decisionText("allow_session", "this computer"), "Allowed for the session · this computer");
  assert.equal(decisionText("deny", "talk mode"), "Denied · talk mode");
  assert.equal(decisionText("deny", null), "Denied", "nobody answered: a cancelled turn");
  assert.equal(decisionText("allow", "  "), "Allowed");
});

test("the answer comes from permission_resolved, and older daemons send none", () => {
  assert.deepEqual(decisionOf({ type: "permission_resolved", request_id: "r1", allowed: true, decision: "allow_session", by: "Pixel 9" }), { decision: "allow_session", by: "Pixel 9" });
  assert.deepEqual(decisionOf({ allowed: false, decision: "deny" }), { decision: "deny", by: null });
  assert.equal(decisionOf({ request_id: "r1", allowed: true }), null, "an older daemon: no decision, no line");
  assert.equal(decisionOf({ decision: "maybe" }), null);
  assert.equal(decisionOf(null), null);
});

test("a saved session's decisions are matched to tool cards by call id", () => {
  const map = decisionsByCall([
    { call_id: "c1", tool: "bash", summary: "cargo publish", decision: "allow", by: "Pixel 9", at: "2026-10-29T10:00:00Z" },
    { call_id: "c2", tool: "bash", summary: "rm -rf target", decision: "deny", by: "this computer", at: "2026-10-29T10:01:00Z" },
    { call_id: "", tool: "malac_configure", summary: "enable plugin", decision: "allow", by: "talk mode", at: "" },
    { call_id: "c3", decision: "bogus" },
    { call_id: "c1", tool: "bash", summary: "cargo publish", decision: "allow_session", by: "this computer", at: "2026-10-29T10:02:00Z" },
  ]);
  assert.deepEqual([...map.keys()], ["c1", "c2"], "calls without an id or a known answer are left out");
  assert.deepEqual(map.get("c1"), { decision: "allow_session", by: "this computer" }, "a later answer to the same call wins");
  assert.equal(decisionText(map.get("c2")!.decision, map.get("c2")!.by), "Denied · this computer");
  assert.equal(decisionsByCall(undefined).size, 0, "transcripts from older daemons have no decisions");
});

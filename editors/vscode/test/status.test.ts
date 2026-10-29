import assert from "node:assert/strict";
import { test } from "node:test";
import type { ConnectionStatus } from "../src/connection/client";
import { statusSummary } from "../src/model/status";
import { DaemonStore } from "../src/model/store";
import { usageHtml, usageModel } from "../src/model/usage";
import type { ServerMessage, UsageReport } from "../src/protocol";

const online: ConnectionStatus = { state: "online", detail: "", endpoint: { url: "http://127.0.0.1:7791", token: "t", source: "daemon.json" } };

function store(mismatch = false): DaemonStore {
  const s = new DaemonStore();
  s.apply({
    type: "snapshot",
    daemon: { version: "1.0.0", pid: 1, started: "", version_mismatch: mismatch, clients: 1, attached: false, remote: false },
    workspaces: [],
    workspace: null,
    pending: [{ id: "r1", call_id: "c1", agent: "a1", agent_label: "main", tool: "bash", summary: "ls", input: {}, tab: "t1" }],
    jobs: [],
    models: [],
    default_model: null,
  } as ServerMessage);
  return s;
}

test("the status bar counts running agents and waiting approvals", () => {
  const s = store();
  s.apply({ type: "event", tab: "t1", envelope: { agent: "a2", seq: 1, at: "", type: "user_message", text: "go" } });
  const summary = statusSummary(s, online, "1.0.0");
  assert.equal(summary.text, "$(malac-pig) $(sync~spin) 1 $(bell-dot) 1", "busy: the pig and the counts");
  assert.equal(summary.severity, "warning");
  assert.match(summary.tooltip.join("\n"), /1 agent running\n1 approval waiting/);
});

test("idle, the status bar reads like Claude Code's: the pig and the name", () => {
  const s = new DaemonStore();
  s.apply({ type: "snapshot", daemon: { version: "1.1.1", pid: 1, started: "", version_mismatch: false, clients: 1, attached: false, remote: false }, workspaces: [], workspace: null, pending: [], jobs: [], models: [], default_model: null } as ServerMessage);
  const summary = statusSummary(s, online, "1.1.1");
  assert.equal(summary.text, "$(malac-pig) malac");
  assert.equal(summary.severity, "normal");
  assert.match(summary.tooltip.at(-1)!, /Click to open malac/);
});

test("the status bar flags a daemon from another version", () => {
  const summary = statusSummary(store(true), online, "1.0.1");
  assert.match(summary.text, /\$\(warning\)/);
  assert.match(summary.tooltip.join("\n"), /The daemon runs malac 1\.0\.0, this malac is 1\.0\.1/);
});

test("the status bar shows the connection when offline", () => {
  const s = new DaemonStore();
  assert.equal(statusSummary(s, { state: "offline", detail: "connection lost", endpoint: null }, undefined).text, "$(malac-pig) malac $(debug-disconnect)");
  assert.equal(statusSummary(s, { state: "stopped", detail: "not running", endpoint: null }, undefined).text, "$(malac-pig) malac $(debug-disconnect)");
  assert.equal(statusSummary(s, { state: "failed", detail: "no binary", endpoint: null }, undefined).severity, "error");
});

const bucket = (input: number, cost = 0, sessions = 1) => ({ input_tokens: input, output_tokens: 0, cache_read_tokens: 0, cache_write_tokens: 0, cost, sessions });

test("usage bars are relative to the week and the busiest model", () => {
  const today = new Date(2026, 9, 26);
  const report: UsageReport = {
    days: 7,
    since: "2026-10-20",
    today: bucket(250),
    week: bucket(1000, 0.5, 4),
    total: bucket(1000, 0.5, 4),
    by_model: [{ model: "lora/smart", ...bucket(800, 0.4) }, { model: "<b>x</b>", ...bucket(200, 0.1) }],
    by_day: [{ date: "2026-10-26", ...bucket(250) }, { date: "2026-10-24", ...bucket(750) }],
  };
  const model = usageModel(report, today);
  assert.deepEqual(model.summary.map((r) => [r.label, r.fraction]), [["Today", 0.25], ["Last 7 days", 1]]);
  assert.deepEqual(model.models.map((r) => r.fraction), [1, 0.25]);
  assert.equal(model.days.length, 7);
  assert.equal(model.days.at(-1)!.label, "Today");
  assert.equal(model.days[4].tokens, 750);
  assert.ok(model.hasCost);
  const html = usageHtml(model, { nonce: "n" });
  assert.ok(!/ style="/.test(html), "no inline style attributes: the CSP allows only the nonced stylesheet");
  assert.match(html, /&lt;b&gt;x&lt;\/b&gt;/, "model names are escaped");
  assert.match(html, /aria-valuenow="25"/);
  assert.match(usageHtml(null, { nonce: "n", message: "Not connected" }), /Not connected/);
});

test("while approvals wait, a click on the status bar answers them", () => {
  const busy = statusSummary(store(), online, "1.0.0");
  assert.equal(busy.click, "approvals");
  assert.match(busy.tooltip.at(-1)!, /Click to answer it/);
  const s = new DaemonStore();
  s.apply({ type: "snapshot", daemon: { version: "1.1.1", pid: 1, started: "", version_mismatch: false, clients: 1, attached: false, remote: false }, workspaces: [], workspace: null, pending: [], jobs: [], models: [], default_model: null } as ServerMessage);
  assert.equal(statusSummary(s, online, "1.1.1").click, "open");
});

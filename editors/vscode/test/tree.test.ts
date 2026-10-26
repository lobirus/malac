import assert from "node:assert/strict";
import { test } from "node:test";
import { DaemonStore } from "../src/model/store";
import { agentNode, dayGroup, jobNodes, sessionNodes, STATUS_ICONS, workspaceTree } from "../src/model/tree";
import type { AgentView, JobView, ServerMessage, SessionSummary, TabView } from "../src/protocol";

const usage = { input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_write_tokens: 0, cost: 0 };

function agent(id: string, overrides: Partial<AgentView> = {}): AgentView {
  return { id, label: "main", parent: null, depth: 0, cwd: "/src/app", created: "2026-10-26T08:00:00Z", model: "lora/fast", status: "idle", activity: "", usage, report: null, live: true, ...overrides };
}

function tab(id: string, agents: AgentView[], overrides: Partial<TabView> = {}): TabView {
  return { id, name: id, cwd: `/src/${id}`, agents: agents.filter((a) => !a.parent).map((a) => a.id), active_agent: agents[0]?.id ?? null, mode: "ask", options: {}, loaded: true, snapshots: agents, ...overrides };
}

function snapshot(tabs: TabView[]): ServerMessage {
  return {
    type: "snapshot",
    daemon: { version: "1.0.1", pid: 1, started: "", version_mismatch: false, clients: 1, attached: false, remote: false, user_home: "/src" },
    workspaces: [
      { id: "w1", name: "default", tabs: tabs.length, active_tab: tabs[0]?.id ?? null, running: 0, created: "", updated: "2026-10-26T08:00:00Z" },
      { id: "w2", name: "other", tabs: 2, active_tab: null, running: 1, created: "", updated: "2026-10-25T08:00:00Z" },
    ],
    workspace: { id: "w1", name: "default", active_tab: tabs[0]?.id ?? null, created: "", updated: "", tabs },
    pending: [],
    jobs: [],
    models: ["lora/fast"],
    default_model: "lora/fast",
  };
}

let seq = 0;
function event(agentId: string, type: string, fields: Record<string, unknown> = {}, tabId = "api"): ServerMessage {
  return { type: "event", tab: tabId, workspace: "w1", envelope: { agent: agentId, seq: seq++, at: "2026-10-26T08:01:00Z", type, ...fields } };
}

function storeWith(): DaemonStore {
  const store = new DaemonStore();
  store.apply(snapshot([tab("api", [agent("a1"), agent("a2"), agent("a3"), agent("a4")]), tab("web", [])]));
  return store;
}

test("workspaces list the connection's first, then tabs and their agents", () => {
  const store = storeWith();
  const roots = workspaceTree(store);
  assert.deepEqual(roots.map((n) => [n.label, n.description, n.contextValue]), [
    ["default", "current · 2 tabs", "workspace.current"],
    ["other", "2 tabs · 1 running", "workspace"],
  ]);
  assert.equal(roots[0].collapsed, false);
  assert.equal(roots[1].children, undefined, "another workspace's tabs load when it is expanded");
  const [api, web] = roots[0].children!;
  assert.equal(api.label, "api");
  assert.equal(api.description, "~/api", "paths under the home show as ~/…");
  assert.deepEqual(api.command, { id: "malac.tab.open", args: [{ tab: "api", workspace: "w1" }] });
  assert.equal(api.children!.length, 4);
  assert.equal(web.children![0].kind, "info", "an empty tab says so");
});

test("agents show running, awaiting approval, done and failed", () => {
  const store = storeWith();
  store.apply(event("a1", "user_message", { text: "fix the tests\nplease" }));
  store.apply(event("a1", "tool_started", { call_id: "c1", tool: "bash", summary: "cargo test", input: {} }));
  store.apply(event("a2", "user_message", { text: "deploy" }));
  store.apply(event("a2", "permission_requested", { request_id: "r1", call_id: "c2", tool: "bash", summary: "rm -rf build", input: {} }));
  store.apply(event("a3", "user_message", { text: "summarize" }));
  store.apply(event("a3", "turn_finished", { status: "completed", text: "done" }));
  store.apply(event("a4", "user_message", { text: "break" }));
  store.apply(event("a4", "turn_finished", { status: "failed", text: "error" }));
  const agents = workspaceTree(store)[0].children![0].children!;
  assert.deepEqual(agents.map((n) => [n.label, n.icon, n.contextValue]), [
    ["fix the tests", STATUS_ICONS.running, "agent.running"],
    ["deploy", STATUS_ICONS.waiting, "agent.running"],
    ["summarize", STATUS_ICONS.completed, "agent.idle"],
    ["break", STATUS_ICONS.failed, "agent.idle"],
  ]);
  assert.equal(agents[0].description, "bash cargo test · fast");
  assert.equal(agents[1].description, "waiting for approval: bash · fast");
  assert.equal(agents[2].description, "Done · fast");
  assert.deepEqual(STATUS_ICONS.running, { id: "sync~spin", color: "charts.green" });
  const tabNode = workspaceTree(store)[0].children![0];
  assert.deepEqual(tabNode.icon, STATUS_ICONS.waiting, "a tab with an approval waiting shows it");
  store.apply(event("a2", "permission_resolved", { request_id: "r1", allowed: true }));
  assert.deepEqual(workspaceTree(store)[0].children![0].icon, STATUS_ICONS.running);
});

test("subagents nest under their parent", () => {
  const store = storeWith();
  store.apply(event("a1", "user_message", { text: "plan" }));
  store.apply(event("s1", "agent_started", { label: "tests", parent: "a1", model: "lora/fast", cwd: "/src/api" }));
  store.apply(event("s1", "user_message", { text: "run the tests" }));
  const parent = workspaceTree(store)[0].children![0].children![0];
  assert.equal(parent.children?.length, 1);
  const child = parent.children![0];
  assert.equal(child.label, "tests", "subagents go by their label");
  assert.equal(child.contextValue, "agent.running.sub");
  assert.deepEqual(child.command, { id: "malac.agent.open", args: [{ agent: "s1", tab: "api" }] });
});

test("saved sessions in a tab are marked as saved", () => {
  const store = new DaemonStore();
  store.apply(snapshot([tab("api", [agent("old", { live: false, title: "earlier work" })])]));
  const node = agentNode(store, store.agents.get("old")!);
  assert.equal(node.label, "earlier work");
  assert.deepEqual(node.icon, STATUS_ICONS.saved);
  assert.match(node.tooltip!, /prompting it resumes it/);
});

test("nothing is shown before the snapshot", () => {
  assert.deepEqual(workspaceTree(new DaemonStore()), []);
});

test("jobs: running first, with a kill action, then by start time", () => {
  const base = { cwd: "/src/api", pid: 10, log: "/tmp/x.log", agent: "a1", tab: "api", exit_code: null, finished: null };
  const jobs: JobView[] = [
    { ...base, id: "aaaa0001", command: "npm run build", started: "2026-10-26T08:00:00Z", finished: "2026-10-26T08:00:30Z", status: "exited", exit_code: 0 },
    { ...base, id: "aaaa0002", command: "npm run dev\n--port 3000", started: "2026-10-26T08:01:00Z", status: "running" },
    { ...base, id: "aaaa0003", command: "cargo test", started: "2026-10-26T08:02:00Z", finished: "2026-10-26T08:02:05Z", status: "exited", exit_code: 101 },
    { ...base, id: "aaaa0004", command: "sleep 100", started: "2026-10-26T07:00:00Z", status: "killed" },
  ];
  const nodes = jobNodes(jobs, Date.parse("2026-10-26T08:03:00Z"));
  assert.deepEqual(nodes.map((n) => [n.label, n.description, n.icon?.id, n.contextValue]), [
    ["npm run dev", "running 2 min · aaaa0002", "sync~spin", "job.running"],
    ["cargo test", "exited 101 · aaaa0003", "error", "job"],
    ["npm run build", "exited 0 · aaaa0001", "pass", "job"],
    ["sleep 100", "killed · aaaa0004", "circle-slash", "job"],
  ]);
  assert.deepEqual(nodes[0].command, { id: "malac.job.log", args: [{ job: "aaaa0002" }] });
  assert.equal(jobNodes([])[0].kind, "info");
});

test("sessions group by day, newest first", () => {
  const now = new Date(2026, 9, 26, 12, 0);
  const at = (days: number, hour = 10) => new Date(2026, 9, 26 - days, hour).toISOString();
  const session = (id: string, updated: string, title = ""): SessionSummary => ({ id, label: "main", parent: null, cwd: "/src/api", model: "lora/fast", created: updated, title, updated });
  const nodes = sessionNodes([session("s1", at(0), "today's work"), session("s2", at(1)), session("s3", at(3), "midweek"), session("s4", at(30), "long ago")], now, new Set(["s1"]));
  assert.deepEqual(nodes.map((n) => [n.label, n.description, n.collapsed]), [
    ["Today", "1", false],
    ["Yesterday", "1", false],
    ["This week", "1", true],
    ["Older", "1", true],
  ]);
  const today = nodes[0].children![0];
  assert.equal(today.label, "today's work");
  assert.equal(today.description, "2 h ago · api");
  assert.equal(today.icon?.id, "comment-discussion", "sessions open in a tab look different");
  assert.equal(nodes[1].children![0].label, "main", "untitled sessions fall back to the label");
  assert.equal(dayGroup(at(0, 0), now), "Today");
  assert.equal(sessionNodes([], now)[0].kind, "info");
});

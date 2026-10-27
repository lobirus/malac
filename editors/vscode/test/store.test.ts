import assert from "node:assert/strict";
import { test } from "node:test";
import { Change, DaemonStore } from "../src/model/store";
import type { ServerMessage, TabView } from "../src/protocol";

const tab = (id: string, cwd: string, agents: string[] = []): TabView => ({ id, name: id, cwd, agents, active_agent: agents[0] ?? null, mode: "ask", options: {}, loaded: false, snapshots: [] });

function start(): { store: DaemonStore; changes: Change[] } {
  const store = new DaemonStore();
  const changes: Change[] = [];
  store.onChange.event((c) => changes.push(c));
  store.apply({
    type: "snapshot",
    daemon: { version: "1.0.1", pid: 1, started: "", version_mismatch: false, clients: 1, attached: false, remote: false },
    workspaces: [{ id: "w1", name: "default", tabs: 1, active_tab: "t1", running: 0, created: "", updated: "" }],
    workspace: { id: "w1", name: "default", active_tab: "t1", created: "", updated: "", tabs: [tab("t1", "/src/app/")] },
    pending: [],
    jobs: [],
    models: [],
    default_model: null,
  } as ServerMessage);
  return { store, changes };
}

test("tab deltas keep the workspace current", () => {
  const { store, changes } = start();
  store.apply({ type: "tab_created", workspace: "w1", index: 0, tab: tab("t2", "/src/web") });
  assert.deepEqual(store.views.get("w1")!.tabs.map((t) => t.id), ["t2", "t1"]);
  store.apply({ type: "tabs_reordered", workspace: "w1", tabs: ["t1", "t2"] });
  assert.deepEqual(store.views.get("w1")!.tabs.map((t) => t.id), ["t1", "t2"]);
  store.apply({ type: "tab_updated", workspace: "w1", tab: { ...tab("t2", "/src/web"), name: "web" } });
  assert.equal(store.tab("t2")!.tab.name, "web");
  store.apply({ type: "mode", mode: "autopilot", tab: null, workspace: "w1" });
  assert.deepEqual(store.views.get("w1")!.tabs.map((t) => t.mode), ["autopilot", "autopilot"]);
  store.apply({ type: "tab_closed", workspace: "w1", tab: "t2" });
  assert.equal(store.tab("t2"), null);
  assert.equal(changes.at(-1)!.closedTab, "t2");
  assert.equal(store.tabForCwd("/src/app")!.id, "t1", "trailing slashes do not matter");
});

test("deltas merge and approvals come and go", () => {
  const { store, changes } = start();
  const ev = (type: string, fields: Record<string, unknown>) => store.apply({ type: "event", tab: "t1", envelope: { agent: "a1", seq: 0, at: "", type, ...fields } });
  ev("user_message", { text: "hi" });
  ev("text_delta", { text: "Hel" });
  ev("text_delta", { text: "lo" });
  const agent = store.agents.get("a1")!;
  assert.deepEqual(agent.events.map((e) => e.env.text), ["hi", "Hello"]);
  assert.equal(agent.status, "running");
  assert.equal(agent.title, "hi");
  ev("permission_requested", { request_id: "r1", call_id: "c1", tool: "bash", summary: "ls", input: {} });
  assert.deepEqual(changes.at(-1)!.requested!.map((r) => [r.id, r.tab]), [["r1", "t1"]]);
  assert.equal(store.agentStatus(agent), "waiting");
  ev("permission_resolved", { request_id: "r1", allowed: true });
  assert.equal(store.pending.size, 0);
  ev("usage", { turn: {}, total: { input_tokens: 10, output_tokens: 2, cache_read_tokens: 0, cache_write_tokens: 0, cost: 0 }, context_tokens: 12 });
  ev("turn_finished", { status: "completed", text: "done" });
  assert.equal(agent.contextTokens, 12);
  assert.equal(store.agentStatus(agent), "completed");
  assert.deepEqual(changes.at(-1)!.finished, [{ agent: "a1", status: "completed" }]);
});

test("a new snapshot drops buffered events and other workspaces", () => {
  const { store } = start();
  store.apply({ type: "event", tab: "t1", envelope: { agent: "a1", seq: 0, at: "", type: "user_message", text: "hi" } });
  store.addView({ id: "w2", name: "x", active_tab: null, created: "", updated: "", tabs: [tab("t9", "/x")] });
  assert.ok(store.tab("t9"));
  const generation = store.generation;
  store.apply({ type: "snapshot", daemon: store.daemon, workspaces: [], workspace: null, pending: [], jobs: [], models: [], default_model: null } as unknown as ServerMessage);
  assert.equal(store.tab("t9"), null);
  assert.deepEqual(store.agents.get("a1")!.events, []);
  assert.equal(store.generation, generation + 1);
});

test("job events update the job table", () => {
  const { store } = start();
  const job = { id: "j1", command: "sleep 1", cwd: "/", pid: 1, log: "", started: "", finished: null, exit_code: null, status: "running" as const, agent: null, tab: null };
  store.apply({ type: "job_started", job });
  assert.equal(store.jobs.get("j1")!.status, "running");
  store.apply({ type: "job_finished", job: { ...job, status: "exited", exit_code: 0 } });
  assert.equal(store.jobs.get("j1")!.status, "exited");
});

const feature = (enabled: boolean, model: string | null, models: string[] = model ? [model] : []) => ({ enabled, model, configured: null, voice: null, models });

test("voice status comes with the snapshot and voice broadcasts; say passes through", () => {
  const { store, changes } = start();
  assert.equal(store.voice === null, true, "an older daemon has none");
  const voice = { dictation: feature(true, "fake/whisper-1"), speech: feature(false, "system"), talk: feature(true, null), language: "en", voices: ["alloy"], checked: null };
  store.apply({ type: "snapshot", daemon: store.daemon, workspaces: [], workspace: null, pending: [], jobs: [], models: [], default_model: null, voice } as unknown as ServerMessage);
  assert.equal(store.voice?.dictation.model, "fake/whisper-1");
  assert.ok(changes.at(-1)!.voice);
  store.apply({ type: "voice", voice: { ...voice, speech: feature(true, "system") } });
  assert.equal(store.voice?.speech.enabled, true);
  assert.deepEqual(changes.at(-1), { voice: true });
  const say = { type: "say", id: "s1", text: "Done.", reason: "finished", tab: "t1", agent: "a1", listener: 3 };
  store.apply(say);
  assert.deepEqual(changes.at(-1)!.say, say);
  store.setVoice({ ...voice, talk: feature(false, null) });
  assert.equal(store.voice?.talk.enabled, false);
});

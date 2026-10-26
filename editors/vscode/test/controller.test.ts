import assert from "node:assert/strict";
import { test } from "node:test";
import { ChatController, ChatHost } from "../src/chat/controller";
import type { ToWebview } from "../src/chat/messages";
import type { ConnectionStatus, DaemonClient } from "../src/connection/client";
import { Emitter } from "../src/model/emitter";
import { DaemonStore } from "../src/model/store";
import type { ServerMessage } from "../src/protocol";

const usage = { input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_write_tokens: 0, cost: 0 };

function setup() {
  const store = new DaemonStore();
  const sent: Record<string, unknown>[] = [];
  const replies: ((command: Record<string, unknown>) => ServerMessage)[] = [];
  const status: ConnectionStatus = { state: "online", detail: "", endpoint: null };
  const client = {
    connection: status,
    online: true,
    onStatus: new Emitter<ConnectionStatus>(),
    send(command: Record<string, unknown>) {
      sent.push(command);
      return true;
    },
    async request(command: Record<string, unknown>) {
      sent.push(command);
      const reply = replies.shift();
      return reply ? reply(command) : { type: "ack" };
    },
    async get(path: string) {
      if (path.startsWith("/api/sessions/")) return { history: [{ role: "user", content: [{ type: "text", text: "earlier" }] }] };
      return { sessions: [] };
    },
  } as unknown as DaemonClient;
  const posted: ToWebview[] = [];
  const memory = new Map<string, unknown>();
  let editor: ReturnType<ChatHost["editor"]> = null;
  const host: ChatHost = {
    post: (m) => posted.push(m),
    editor: () => editor,
    findFiles: async () => [],
    openFile: () => {},
    saveUpload: async (name) => `/uploads/${name}`,
    runCommand: () => {},
    remember: (k, v) => memory.set(k, v),
    recall: <T,>(k: string) => memory.get(k) as T | undefined,
    log: () => {},
    clientVersion: () => "1.0.1",
    dropHint: "",
  };
  const chat = new ChatController(store, client, host);
  store.apply({
    type: "snapshot",
    daemon: { version: "1.0.1", pid: 1, started: "", version_mismatch: false, clients: 1, attached: false, remote: false },
    workspaces: [{ id: "w1", name: "default", tabs: 1, active_tab: "t1", running: 0, created: "", updated: "" }],
    workspace: {
      id: "w1",
      name: "default",
      active_tab: "t1",
      created: "",
      updated: "",
      tabs: [
        {
          id: "t1",
          name: "app",
          cwd: "/src/app",
          agents: ["a1"],
          active_agent: "a1",
          mode: "ask",
          options: { model: "lora/smart" },
          loaded: true,
          snapshots: [{ id: "a1", label: "main", parent: null, depth: 0, cwd: "/src/app", created: "", model: "lora/fast", status: "idle", activity: "", usage, report: null, live: true }],
        },
      ],
    },
    pending: [
      { id: "r1", call_id: "c1", agent: "a1", agent_label: "main", tool: "bash", summary: "ls", input: {}, tab: "t1" },
      { id: "r2", call_id: "c2", agent: "zz", agent_label: "other", tool: "bash", summary: "ls", input: {}, tab: "t9" },
    ],
    jobs: [],
    models: ["lora/fast", "lora/smart"],
    default_model: "lora/fast",
  });
  return { store, chat, sent, replies, posted, memory, setEditor: (e: typeof editor) => (editor = e) };
}

const tick = () => new Promise((r) => setTimeout(r, 60));

test("the chat opens on the workspace's active tab and agent, with its transcript", async () => {
  const { chat, posted } = setup();
  chat.receive({ type: "ready" });
  await tick();
  const state = [...posted].reverse().find((m) => m.type === "state");
  assert.ok(state && state.type === "state");
  assert.equal(state.state.tab?.name, "app");
  assert.equal(state.state.agent?.id, "a1");
  assert.deepEqual(state.state.pending.map((p) => p.id), ["r1"], "only this tab's approvals");
  const transcript = posted.find((m) => m.type === "transcript");
  assert.ok(transcript && transcript.type === "transcript" && transcript.agent === "a1" && transcript.history.length === 1);
});

test("a new agent's prompt carries the tab, its model and the editor context", async () => {
  const { chat, sent, replies, posted, setEditor } = setup();
  chat.receive({ type: "ready" });
  chat.newAgent();
  setEditor({
    path: "/src/app/src/cart.ts",
    displayPath: "src/cart.ts",
    languageId: "typescript",
    selection: { startLine: 6, startCharacter: 0, endLine: 8, endCharacter: 1 },
    selectedText: "export function total() {\n  return 0;\n}",
    dirty: false,
    text: () => "",
  });
  replies.push(() => ({ type: "ack", agent: "a2", tab: "t1" }));
  chat.receive({ type: "submit", text: "explain", attachments: [{ path: "/src/app/README.md" }], images: [{ media_type: "image/png", data: "AAAA", name: "Pasted image.png" }] });
  await tick();
  const prompt = sent.find((c) => c.type === "prompt")!;
  assert.equal(prompt.tab, "t1");
  assert.equal(prompt.agent, undefined);
  assert.equal(prompt.model, "lora/smart", "the tab's model");
  assert.equal(prompt.text, "explain\n\nsrc/cart.ts, lines 7-9:\n\n```typescript\nexport function total() {\n  return 0;\n}\n```");
  assert.deepEqual(prompt.attachments, [{ path: "/src/app/README.md" }]);
  assert.deepEqual(prompt.images, [{ media_type: "image/png", data: "AAAA", name: "Pasted image.png" }]);
  assert.ok(posted.some((m) => m.type === "sent" && m.ok));
  assert.equal(chat.currentAgent, "a2", "the chat follows the new agent");
});

test("a follow-up goes to the agent; a turned-off chip adds nothing", async () => {
  const { chat, sent, memory, setEditor } = setup();
  chat.receive({ type: "ready" });
  setEditor({ path: "/src/app/a.ts", displayPath: "a.ts", languageId: "typescript", selection: null, selectedText: "", dirty: false, text: () => "" });
  chat.receive({ type: "toggleContext", enabled: false });
  assert.equal(memory.get("chat.context"), false);
  chat.receive({ type: "submit", text: "go on", attachments: [], images: [] });
  await tick();
  const prompt = sent.find((c) => c.type === "prompt")!;
  assert.equal(prompt.agent, "a1");
  assert.equal(prompt.tab, undefined);
  assert.deepEqual(prompt.attachments, []);
  chat.receive({ type: "toggleContext", enabled: true });
  chat.receive({ type: "submit", text: "and this file", attachments: [], images: [] });
  await tick();
  assert.deepEqual(sent.filter((c) => c.type === "prompt")[1].attachments, [{ path: "/src/app/a.ts" }]);
});

test("approvals, stop, mode and model go to the daemon", async () => {
  const { chat, sent } = setup();
  chat.receive({ type: "ready" });
  chat.receive({ type: "approve", request: "r1", decision: "allow_session" });
  chat.receive({ type: "stop" });
  chat.receive({ type: "setMode", mode: "autopilot" });
  chat.receive({ type: "setModel", model: "lora/smart" });
  await tick();
  assert.deepEqual(
    sent.filter((c) => c.type !== "tab_activate").map((c) => ({ ...c, client_ref: undefined })),
    [
      { type: "approve", request_id: "r1", decision: "allow_session", client_ref: undefined },
      { type: "cancel", agent: "a1", client_ref: undefined },
      { type: "mode", mode: "autopilot", tab: "t1", client_ref: undefined },
      { type: "model", agent: "a1", model: "lora/smart", client_ref: undefined },
    ],
  );
});

test("live events of the shown agent stream to the webview", async () => {
  const { chat, store, posted } = setup();
  chat.receive({ type: "ready" });
  await tick();
  store.apply({ type: "event", tab: "t1", envelope: { agent: "a1", seq: 1, at: "", type: "text_delta", text: "Hel" } });
  store.apply({ type: "event", tab: "t1", envelope: { agent: "other", seq: 2, at: "", type: "text_delta", text: "x" } });
  store.apply({ type: "event", tab: "t1", envelope: { agent: "a1", seq: 3, at: "", type: "text_delta", text: "lo" } });
  await tick();
  const events = posted.filter((m) => m.type === "events");
  assert.equal(events.length, 1, "batched");
  assert.ok(events[0].type === "events" && events[0].agent === "a1" && events[0].events.map((e) => e.text).join("") === "Hello");
});

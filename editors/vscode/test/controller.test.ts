import assert from "node:assert/strict";
import { test } from "node:test";
import { ChatController, ChatHost, ChatVoice } from "../src/chat/controller";
import type { ToWebview } from "../src/chat/messages";
import { HttpError, type ConnectionStatus, type DaemonClient } from "../src/connection/client";
import type { FileAccess } from "../src/model/attachments";
import { Emitter } from "../src/model/emitter";
import { DaemonStore } from "../src/model/store";
import type { Dictation, DictationResult } from "../src/voice/cli";
import { DictationManager } from "../src/voice/dictation";
import type { DecisionRecord, ProjectSummary, ServerMessage } from "../src/protocol";

const usage = { input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_write_tokens: 0, cost: 0 };

function setup({ project, address, files, voice, decisions }: { project?: ProjectSummary; address?: { url: string; token: string; source: string }; files?: FileAccess; voice?: ChatVoice; decisions?: DecisionRecord[] } = {}) {
  const store = new DaemonStore();
  const sent: Record<string, unknown>[] = [];
  const replies: ((command: Record<string, unknown>) => ServerMessage)[] = [];
  const status: ConnectionStatus = { state: "online", detail: "", endpoint: null };
  const client = {
    connection: status,
    online: true,
    address: address ?? { url: "http://127.0.0.1:7700", token: "t", source: "daemon.json" },
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
      if (path.startsWith("/api/sessions/")) return { history: [{ role: "user", content: [{ type: "text", text: "earlier" }] }], ...(decisions ? { decisions } : {}) };
      return { sessions: [] };
    },
    async download(path: string) {
      downloads.push(path);
      if (path.startsWith("/api/shared/gone")) throw new HttpError(404, "not found");
      return { data: Buffer.from("PNGDATA"), type: "image/png" };
    },
  } as unknown as DaemonClient;
  const downloads: string[] = [];
  const opened: { name: string; data: string; key: string }[] = [];
  const posted: ToWebview[] = [];
  const memory = new Map<string, unknown>();
  let editor: ReturnType<ChatHost["editor"]> = null;
  const host: ChatHost = {
    post: (m) => posted.push(m),
    editor: () => editor,
    findFiles: async () => [],
    openFile: () => {},
    saveUpload: async (name) => `/uploads/${name}`,
    openMedia: async (name, data, key) => void opened.push({ name, data: data.toString(), key }),
    saveMedia: async (name) => `/saved/${name}`,
    runCommand: () => {},
    remember: (k, v) => memory.set(k, v),
    recall: <T,>(k: string) => memory.get(k) as T | undefined,
    log: () => {},
    clientVersion: () => "1.0.1",
    dropHint: "",
  };
  const chat = new ChatController(store, client, host, { files, voice });
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
          project,
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
  return { store, chat, sent, replies, posted, memory, downloads, opened, setEditor: (e: typeof editor) => (editor = e) };
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

const lastState = (posted: ToWebview[]) => {
  const state = [...posted].reverse().find((m) => m.type === "state");
  assert.ok(state && state.type === "state");
  return state.state;
};

test("the composer gets the built-in and the tab's custom slash commands", async () => {
  const { chat, posted } = setup({ project: { root: "/src/app", commands: [{ name: "review", description: "Review the diff", argument_hint: "[pr]", source: "project" }] } });
  chat.receive({ type: "ready" });
  await tick();
  const commands = lastState(posted).commands;
  assert.equal(commands[0].name, "model");
  assert.deepEqual(commands.at(-1), { name: "review", description: "Review the diff", hint: "[pr]", source: "project" });
});

test("built-in commands run in the chat; custom ones go to the agent as typed", async () => {
  const { chat, sent, posted } = setup();
  chat.receive({ type: "ready" });
  chat.receive({ type: "submit", text: "/mode edits", attachments: [], images: [] });
  chat.receive({ type: "submit", text: "/model smart", attachments: [], images: [] });
  chat.receive({ type: "submit", text: "/compact", attachments: [], images: [] });
  chat.receive({ type: "submit", text: "/review 42", attachments: [], images: [] });
  await tick();
  assert.deepEqual(
    sent.filter((c) => c.type !== "tab_activate").map((c) => [c.type, c.mode ?? c.model ?? c.text ?? c.agent]),
    [
      ["mode", "edits"],
      ["model", "lora/smart"],
      ["compact", "a1"],
      ["prompt", "/review 42"],
    ],
  );
  assert.equal(posted.filter((m) => m.type === "sent" && m.ok).length, 4);
  chat.receive({ type: "submit", text: "/clear", attachments: [], images: [] });
  await tick();
  assert.equal(chat.currentAgent, null, "/clear starts a new agent");
});

test("a tab inside a larger repository shows a hint that opens the root", async () => {
  const { chat, posted, replies, sent, store } = setup({ project: { root: "/src", subdir: "app", suggest_root: "/src" } });
  chat.receive({ type: "ready" });
  await tick();
  const hint = lastState(posted).banners.find((b) => b.id === "root-hint");
  assert.ok(hint, "the hint is shown");
  assert.match(hint.text, /app is one part of the repository src/);
  const open = hint.actions[0];
  store.apply({ type: "tab_created", workspace: "w1", index: 1, tab: { id: "t2", name: "src", cwd: "/src", agents: [], active_agent: null, mode: "ask", options: {}, loaded: false, snapshots: [] } });
  replies.push(() => ({ type: "ack", tab: "t2", workspace: "w1" }));
  chat.receive({ type: "command", command: open.command, args: open.args });
  await tick();
  assert.deepEqual(sent.find((c) => c.type === "tab_create"), { type: "tab_create", cwd: "/src", reuse: true });
  assert.equal(chat.currentTab, "t2", "the chat switches to the root's tab");
  chat.selectTab("t1");
  const dismiss = hint.actions[1];
  chat.receive({ type: "command", command: dismiss.command, args: dismiss.args });
  await tick();
  assert.equal(lastState(posted).banners.find((b) => b.id === "root-hint"), undefined, "Not now hides it for the tab");
});

test("plugin approvals carry their contents for Show details", async () => {
  const { chat, store, posted } = setup();
  chat.receive({ type: "ready" });
  store.apply({
    type: "event",
    tab: "t1",
    envelope: { agent: "a1", seq: 9, at: "", type: "permission_requested", request_id: "p1", call_id: "", tool: "malac_configure", summary: "enable plugin deploy, which runs hooks", input: { plugin: "deploy", contents: "hooks (run commands): PostToolUse: ./lint.sh" } },
  });
  await tick();
  const request = lastState(posted).pending.find((p) => p.id === "p1")!;
  assert.equal(request.plugin, "deploy");
  assert.equal(request.title, "Enable plugin deploy, which runs hooks");
  assert.equal(request.details, "hooks (run commands): PostToolUse: ./lint.sh");
});

test("a remote daemon gets dropped files and the current file as data; local ones get paths", async () => {
  const table: Record<string, string> = { "/home/me/notes.txt": "remember", "/home/me/app/a.ts": "export {}" };
  const files: FileAccess = {
    stat: async (p) => ({ size: Buffer.byteLength(table[p] ?? ""), isDirectory: () => false }),
    read: async (p) => Buffer.from(table[p]),
  };
  const editor = { path: "/home/me/app/a.ts", displayPath: "a.ts", languageId: "typescript", selection: null, selectedText: "", dirty: false, text: () => "" };
  for (const remote of [true, false]) {
    const { chat, sent, setEditor } = setup({ files, address: remote ? { url: "https://studio.example.com", token: "d", source: "remote" } : undefined });
    chat.receive({ type: "ready" });
    setEditor(editor);
    chat.receive({ type: "submit", text: "look", attachments: [{ path: "/home/me/notes.txt" }], images: [] });
    await tick();
    const prompt = sent.find((c) => c.type === "prompt")!;
    assert.deepEqual(
      prompt.attachments,
      remote
        ? [
            { path: "notes.txt", data: Buffer.from("remember").toString("base64") },
            { path: "a.ts", data: Buffer.from("export {}").toString("base64") },
          ]
        : [{ path: "/home/me/notes.txt" }, { path: "/home/me/app/a.ts" }],
    );
  }
});

test("an attachment a remote daemon cannot take stops the prompt", async () => {
  const files: FileAccess = { stat: async () => ({ size: 0, isDirectory: () => true }), read: async () => Buffer.alloc(0) };
  const { chat, sent, posted } = setup({ files, address: { url: "http://10.0.0.5:7700", token: "t", source: "daemon.json" } });
  chat.receive({ type: "ready" });
  chat.receive({ type: "submit", text: "look", attachments: [{ path: "/home/me/src" }], images: [] });
  await tick();
  assert.equal(sent.find((c) => c.type === "prompt"), undefined);
  const result = posted.find((m) => m.type === "sent");
  assert.ok(result && result.type === "sent" && !result.ok && /src is a folder/.test(result.error ?? ""));
});

test("shared files load through the extension host, once, and open or save from there", async () => {
  const { chat, posted, downloads, opened } = setup();
  chat.receive({ type: "ready" });
  chat.receive({ type: "loadShared", id: "0193f0c2" });
  chat.receive({ type: "loadShared", id: "0193f0c2" });
  await tick();
  const shared = posted.filter((m) => m.type === "shared");
  assert.equal(shared.length, 2);
  assert.ok(shared[0].type === "shared" && shared[0].media_type === "image/png" && Buffer.from(shared[0].data!, "base64").toString() === "PNGDATA");
  assert.deepEqual(downloads, ["/api/shared/0193f0c2?download=true"], "fetched once");
  chat.receive({ type: "media", action: "open", file: { id: "0193f0c2", name: "shot.png", media_type: "image/png" } });
  chat.receive({ type: "media", action: "open", file: { name: "tool-image.png", media_type: "image/png", data: Buffer.from("TOOL").toString("base64") } });
  chat.receive({ type: "media", action: "save", file: { id: "0193f0c2", name: "shot.png", media_type: "image/png" } });
  await tick();
  assert.deepEqual(
    opened.sort((a, b) => a.name.localeCompare(b.name)),
    [
      { name: "shot.png", data: "PNGDATA", key: "0193f0c2" },
      { name: "tool-image.png", data: "TOOL", key: "" },
    ],
  );
  assert.ok(posted.some((m) => m.type === "toast" && m.message === "Saved /saved/shot.png"));
  chat.receive({ type: "loadShared", id: "gone1" });
  chat.receive({ type: "loadShared", id: "../../etc/passwd" });
  await tick();
  const errors = posted.filter((m) => m.type === "shared" && m.error).map((m) => m.type === "shared" && m.error);
  assert.deepEqual(errors, ["No longer on the daemon (shared files are kept 30 days)", "not a shared file"]);
  assert.equal(downloads.length, 2, "a bad id is never requested");
});

/** A recording that finishes when told to. */
function fakeDictation(): Dictation & { finish(result: DictationResult): void; stops: number; cancels: number } {
  let finish!: (result: DictationResult) => void;
  const fake = {
    started: 1000,
    stopping: false,
    stops: 0,
    cancels: 0,
    result: new Promise<DictationResult>((resolve) => (finish = resolve)),
    stop() {
      fake.stopping = true;
      fake.stops += 1;
    },
    cancel() {
      fake.cancels += 1;
      finish({ cancelled: true });
    },
    finish: (result: DictationResult) => finish(result),
  };
  return fake as unknown as Dictation & { finish(result: DictationResult): void; stops: number; cancels: number };
}

test("dictation records for the chat that asked and hands it the text", async () => {
  const recordings: ReturnType<typeof fakeDictation>[] = [];
  const dictation = new DictationManager(async () => {
    const recording = fakeDictation();
    recordings.push(recording);
    return recording;
  });
  const voice: ChatVoice = { offer: () => ({ dictation: true, talk: false, blocked: null }), dictation, onChange: () => ({ dispose() {} }) };
  const { chat, posted } = setup({ voice });
  chat.receive({ type: "ready" });
  await tick();
  assert.deepEqual(lastState(posted).voice, { dictation: true, talk: false, blocked: null });
  chat.receive({ type: "dictation", action: "toggle" });
  await tick();
  assert.deepEqual(posted.filter((m) => m.type === "dictation").at(-1), { type: "dictation", state: "recording", started: 1000 });
  chat.receive({ type: "dictation", action: "toggle" });
  await tick();
  assert.equal(recordings[0].stops, 1, "toggling again stops it");
  assert.equal(posted.filter((m) => m.type === "dictation").at(-1)?.type === "dictation" && (posted.filter((m) => m.type === "dictation").at(-1) as { state: string }).state, "transcribing");
  recordings[0].finish({ text: " make the button blue " });
  await tick();
  assert.deepEqual(posted.filter((m) => m.type === "dictation").at(-1), { type: "dictation", state: "done", text: "make the button blue" });
  chat.receive({ type: "dictation", action: "toggle" });
  await tick();
  chat.receive({ type: "dictation", action: "cancel" });
  await tick();
  assert.equal(recordings[1].cancels, 1);
  assert.deepEqual(posted.filter((m) => m.type === "dictation").at(-1), { type: "dictation", state: "done" }, "a cancelled recording inserts nothing");
});

test("dictation explains what stands in the way", async () => {
  const dictation = new DictationManager(async () => {
    throw new Error("This malac has no voice commands.");
  });
  let blocked: string | null = "Voice is not available in this remote window.";
  const voice: ChatVoice = { offer: () => ({ dictation: true, talk: true, blocked }), dictation, onChange: () => ({ dispose() {} }) };
  const { chat, posted } = setup({ voice });
  chat.receive({ type: "ready" });
  chat.receive({ type: "dictation", action: "toggle" });
  await tick();
  assert.ok(posted.some((m) => m.type === "toast" && m.message === "Voice is not available in this remote window."));
  blocked = null;
  chat.receive({ type: "dictation", action: "toggle" });
  await tick();
  assert.deepEqual(posted.filter((m) => m.type === "dictation").at(-1), { type: "dictation", state: "done", error: "This malac has no voice commands." });
});

test("a transcript carries its saved decisions, for the tool cards", async () => {
  const decisions: DecisionRecord[] = [{ call_id: "c1", tool: "bash", summary: "cargo publish", decision: "allow_session", by: "Pixel 9", at: "2026-10-29T10:00:00Z" }];
  const { chat, posted } = setup({ decisions });
  chat.receive({ type: "ready" });
  await tick();
  const transcript = posted.find((m) => m.type === "transcript");
  assert.ok(transcript && transcript.type === "transcript");
  assert.deepEqual(transcript.decisions, decisions);
  const { chat: older, posted: olderPosted } = setup();
  older.receive({ type: "ready" });
  await tick();
  const plain = olderPosted.find((m) => m.type === "transcript");
  assert.ok(plain && plain.type === "transcript" && Array.isArray(plain.decisions) && plain.decisions.length === 0, "an older daemon sends none");
});

test("the model menu groups by provider and follows models broadcasts", async () => {
  const { chat, posted, store } = setup();
  chat.receive({ type: "ready" });
  await tick();
  const last = () => {
    const message = [...posted].reverse().find((m) => m.type === "state");
    assert.ok(message && message.type === "state");
    return message.state;
  };
  assert.deepEqual(last().modelGroups, [{ provider: "lora", models: [{ spec: "lora/fast", name: "fast" }, { spec: "lora/smart", name: "smart" }] }]);
  // A provider added in another client's settings: no reload needed.
  store.apply({ type: "models", models: ["lora/fast", "lora/smart", "openrouter/anthropic/claude-sonnet"], default_model: "openrouter/anthropic/claude-sonnet" });
  await tick();
  assert.deepEqual(last().modelGroups.map((g) => [g.provider, g.models.map((m) => m.name)]), [["lora", ["fast", "smart"]], ["openrouter", ["anthropic/claude-sonnet"]]]);
  assert.equal(last().defaultModel, "openrouter/anthropic/claude-sonnet");
  // The model in use stays listed when the configuration drops it.
  store.apply({ type: "models", models: ["openrouter/x"], default_model: "openrouter/x" });
  await tick();
  assert.deepEqual(last().modelGroups.map((g) => [g.provider, g.models.map((m) => m.spec)]), [["openrouter", ["openrouter/x"]], ["lora", ["lora/fast"]]]);
});

// The approval notifications and "malac: Answer Approvals", against a small
// stand-in for the vscode module: progress notifications that end when
// their task does, and QuickPicks the test drives.
import assert from "node:assert/strict";
import Module from "node:module";
import { test } from "node:test";
import type { DaemonClient } from "../src/connection/client";
import { DaemonStore } from "../src/model/store";
import type { PendingApproval, ServerMessage } from "../src/protocol";

type Item = { label: string; kind?: number; [key: string]: unknown };

class FakeEvent<T> {
  private listeners: ((value: T) => void)[] = [];
  event = (listener: (value: T) => void) => {
    this.listeners.push(listener);
    return { dispose: () => (this.listeners = this.listeners.filter((l) => l !== listener)) };
  };
  fire(value: T) {
    for (const listener of [...this.listeners]) listener(value);
  }
}

class FakeQuickPick {
  title = "";
  placeholder = "";
  items: Item[] = [];
  activeItems: Item[] = [];
  selectedItems: Item[] = [];
  visible = false;
  disposed = false;
  private accepted = new FakeEvent<void>();
  private hidden = new FakeEvent<void>();
  onDidAccept = this.accepted.event;
  onDidHide = this.hidden.event;
  show() {
    this.visible = true;
  }
  hide() {
    if (!this.visible) return;
    this.visible = false;
    this.hidden.fire();
  }
  dispose() {
    this.disposed = true;
  }
  /** What a user picking the item whose label contains `text` does. */
  accept(text: string) {
    const item = this.items.find((i) => i.label.includes(text));
    assert.ok(item, `no item "${text}" in ${this.items.map((i) => i.label).join(", ")}`);
    this.selectedItems = [item];
    this.accepted.fire();
  }
}

const notifications: { title: string; open: boolean }[] = [];
const picks: FakeQuickPick[] = [];
const infos: string[] = [];
const modals: { text: string; detail?: string; buttons: string[] }[] = [];
let modalAnswer: string | undefined;

const vscode = {
  ProgressLocation: { Notification: 15 },
  QuickPickItemKind: { Separator: -1, Default: 0 },
  window: {
    withProgress: async (options: { title: string }, task: (progress: unknown) => Promise<unknown>) => {
      const entry = { title: options.title, open: true };
      notifications.push(entry);
      await task({ report() {} });
      entry.open = false;
    },
    createQuickPick: () => {
      const pick = new FakeQuickPick();
      picks.push(pick);
      return pick;
    },
    showInformationMessage: async (text: string) => void infos.push(text),
    showWarningMessage: async (text: string, options: { detail?: string }, ...buttons: string[]) => {
      modals.push({ text, detail: options.detail, buttons });
      return modalAnswer;
    },
  },
};

const loader = Module as unknown as { _load(request: string, ...rest: unknown[]): unknown };
const load = loader._load;
loader._load = function (request: string, ...rest: unknown[]) {
  return request === "vscode" ? vscode : load.call(this, request, ...rest);
};
// eslint-disable-next-line @typescript-eslint/no-require-imports
const { ApprovalNotifier, noticeText } = require("../src/ui/approvals") as typeof import("../src/ui/approvals");

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));
const usage = { input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_write_tokens: 0, cost: 0 };

function setup({ enabled = (): boolean => true, chatShows = (_: PendingApproval): boolean => false } = {}) {
  notifications.length = 0;
  picks.length = 0;
  infos.length = 0;
  modals.length = 0;
  modalAnswer = undefined;
  const store = new DaemonStore();
  const sent: Record<string, unknown>[] = [];
  const client = {
    async request(command: Record<string, unknown>) {
      sent.push(command);
      return { type: "ack" };
    },
  } as unknown as DaemonClient;
  const opened: string[] = [];
  const notifier = new ApprovalNotifier(store, client, chatShows, enabled, async (request) => void opened.push(request.agent));
  // As extension.ts wires it.
  store.onChange.event((change) => notifier.notify(change.requested ?? []));
  const snapshot = (pending: PendingApproval[] = []) =>
    store.apply({
      type: "snapshot",
      daemon: { version: "1.2.1", pid: 1, started: "", version_mismatch: false, clients: 2, attached: false, remote: false },
      workspaces: [{ id: "w1", name: "default", tabs: 1, active_tab: "t1", running: 1, created: "", updated: "" }],
      workspace: {
        id: "w1",
        name: "default",
        active_tab: "t1",
        created: "",
        updated: "",
        tabs: [
          {
            id: "t1",
            name: "api",
            cwd: "/src/api",
            agents: ["a1"],
            active_agent: "a1",
            mode: "ask",
            options: {},
            loaded: true,
            snapshots: [{ id: "a1", label: "main", parent: null, depth: 0, cwd: "/src/api", created: "", model: "lora/fast", status: "running", activity: "", usage, report: null, live: true }],
          },
        ],
      },
      pending,
      jobs: [],
      models: [],
      default_model: null,
    } as unknown as ServerMessage);
  snapshot();
  const ev = (type: string, fields: Record<string, unknown>, agent = "a1") => store.apply({ type: "event", tab: "t1", workspace: "w1", envelope: { agent, seq: 0, at: "", type, ...fields } });
  const ask = (id: string, summary: string, agent = "a1") => ev("permission_requested", { request_id: id, call_id: `c-${id}`, tool: "bash", summary, input: { command: summary } }, agent);
  return { store, notifier, sent, opened, snapshot, ev, ask };
}

test("each waiting request gets a notification that closes once it is answered anywhere", async () => {
  const { ev, ask, snapshot } = setup();
  ask("r1", "cargo publish");
  ask("r2", "rm -rf target");
  await tick();
  assert.deepEqual(
    notifications.map((n) => [n.title, n.open]),
    [
      [noticeText("main in api wants to use bash: cargo publish"), true],
      [noticeText("main in api wants to use bash: rm -rf target"), true],
    ],
  );
  assert.match(notifications[0].title, /^malac: main in api wants to use bash: cargo publish\. Answer in the chat or with malac: Answer Approvals \(/);
  // Answered on a phone: the daemon tells every client.
  ev("permission_resolved", { request_id: "r1", allowed: true, decision: "allow", by: "Pixel 9" });
  await tick();
  assert.deepEqual(notifications.map((n) => n.open), [false, true]);
  // The other one goes with a snapshot that no longer lists it (its agent was stopped).
  snapshot([]);
  await tick();
  assert.deepEqual(notifications.map((n) => n.open), [false, false]);
});

test("no notification while a visible chat shows the request, or with the setting off", async () => {
  let enabled = true;
  const shown = new Set(["r1"]);
  const { ask, notifier, store } = setup({ enabled: () => enabled, chatShows: (request) => shown.has(request.id) });
  ask("r1", "ls");
  ask("r2", "make");
  await tick();
  assert.deepEqual(notifications.map((n) => n.title.includes("make")), [true], "only the one the chat does not show");
  enabled = false;
  notifier.settle();
  await tick();
  assert.equal(notifications[0].open, false, "turning the setting off closes them");
  ask("r3", "make test");
  await tick();
  assert.equal(notifications.length, 1);
  enabled = true;
  store.reset();
  ask("r4", "make lint");
  await tick();
  assert.equal(notifications.length, 1, "nothing is shown while disconnected");
});

test("a lost connection closes the notifications, so none is left behind", async () => {
  const { ask, store, notifier } = setup();
  ask("r1", "ls");
  await tick();
  store.reset();
  await tick();
  assert.equal(notifications[0].open, false);
  ask("r2", "pwd");
  notifier.dispose();
  await tick();
  assert.ok(notifications.every((n) => !n.open), "and disposing the extension closes the rest");
});

test("Answer Approvals picks the request, then answers it", async () => {
  const { ask, notifier, sent, opened } = setup();
  await notifier.answer();
  assert.deepEqual(infos, ["malac: no approvals are waiting."]);
  ask("r1", "cargo publish");
  ask("r2", "rm -rf target");
  let done = notifier.answer();
  await tick();
  assert.equal(picks.length, 1);
  assert.equal(picks[0].title, "malac: 2 approvals waiting");
  assert.deepEqual(picks[0].items.map((i) => [i.label, i.description]), [
    ["$(shield) bash: cargo publish", "main in api"],
    ["$(shield) bash: rm -rf target", "main in api"],
  ]);
  picks[0].accept("rm -rf");
  await tick();
  assert.equal(picks[1].title, "malac: main in api wants to use bash: rm -rf target");
  assert.deepEqual(
    picks[1].items.filter((i) => i.kind !== -1).map((i) => i.label),
    ["$(check) Allow", "$(check-all) Always", "$(close) Deny", "$(comment-discussion) Open in chat"],
    "no details for a short command",
  );
  picks[1].accept("Always");
  await done;
  assert.deepEqual(sent, [{ type: "approve", request_id: "r2", decision: "allow_session" }]);
  assert.ok(picks.every((p) => p.disposed));
  // One request: straight to the answers. Open in chat shows its agent.
  sent.length = 0;
  done = notifier.answer();
  await tick();
  assert.equal(picks.length, 3, "two still wait (nothing was settled yet)");
  picks[2].accept("cargo publish");
  await tick();
  picks[3].accept("Open in chat");
  await done;
  assert.deepEqual(opened, ["a1"]);
  assert.deepEqual(sent, []);
});

test("the picker closes when its request is answered elsewhere; details answer in a dialog", async () => {
  const { ask, ev, notifier, sent } = setup();
  ask("r1", "cargo publish");
  let done = notifier.answer();
  await tick();
  assert.equal(picks.length, 1, "one request: the answers at once");
  assert.equal(picks[0].visible, true);
  ev("permission_resolved", { request_id: "r1", allowed: false, decision: "deny", by: "talk mode" });
  await done;
  assert.equal(picks[0].visible, false);
  assert.ok(picks[0].disposed);
  assert.deepEqual(sent, [], "nothing is sent for a settled request");
  // A long command has details; the dialog answers too.
  const command = "cargo build --release\ncargo publish";
  ev("permission_requested", { request_id: "r2", call_id: "c2", tool: "bash", summary: "cargo build --release", input: { command } });
  modalAnswer = "Deny";
  done = notifier.answer();
  await tick();
  picks[1].accept("Show details");
  await done;
  assert.deepEqual(modals, [{ text: "malac: main in api wants to use bash: cargo build --release", detail: command, buttons: ["Allow", "Always", "Deny"] }]);
  assert.deepEqual(sent, [{ type: "approve", request_id: "r2", decision: "deny" }]);
});

test("the request list follows the store while it is open", async () => {
  const { ask, ev, notifier } = setup();
  ask("r1", "one");
  ask("r2", "two");
  const done = notifier.answer();
  await tick();
  ask("r3", "three");
  assert.deepEqual(picks[0].items.map((i) => i.label), ["$(shield) bash: one", "$(shield) bash: two", "$(shield) bash: three"]);
  assert.equal(picks[0].title, "malac: 3 approvals waiting");
  for (const id of ["r1", "r2", "r3"]) ev("permission_resolved", { request_id: id, allowed: true, decision: "allow", by: "Pixel 9" });
  assert.equal(await done, undefined);
  assert.equal(picks[0].visible, false, "none left: it closes");
});

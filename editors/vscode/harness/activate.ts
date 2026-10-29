// Smoke test of the extension's activation against a real daemon, with a
// small stand-in for the `vscode` module: every command and view declared in
// package.json must be registered, the chat must come up on the folder's
// tab, and the trees and status bar must reflect the daemon.
//
//   MALAC_HOME=/tmp/home HARNESS_CWD=/some/project MALAC_BIN=malac node out/harness/activate.js
//
// FOLDER_TAB sets malac.folderTab (default "folder"); EXPECT_TAB is the tab
// directory the chat should end up in (default HARNESS_CWD); VSCODE_VERSION
// is what `vscode.version` reports (default 1.139.1); SMOKE_PROMPT=1 sends
// one real prompt from an editor tab; SMOKE_VOICE=1 checks dictation, talk
// mode and the voice settings (MALAC_BIN must have the voice commands, or be
// a stand-in for them); SMOKE_SAY_PROMPT, a prompt that makes the agent ask
// for approval, checks that the approval is spoken here (and then denies it).
// SMOKE_APPROVALS, a prompt that makes the agent ask for approval, runs it in
// a tab the chat does not show and checks the approval notification: it
// opens, closes when the request is answered elsewhere, and malac: Answer
// Approvals (the status bar while it waits) answers the next one.

import * as fs from "node:fs";
import Module from "node:module";
import * as os from "node:os";
import * as path from "node:path";

const ROOT = path.resolve(__dirname, "..", "..");
const CWD = path.resolve(process.env.HARNESS_CWD ?? process.cwd());
const pkg = JSON.parse(fs.readFileSync(path.join(ROOT, "package.json"), "utf8"));

type Listener = (value: unknown) => void;
class EventEmitter {
  private listeners: Listener[] = [];
  event = (listener: Listener) => {
    this.listeners.push(listener);
    return { dispose: () => (this.listeners = this.listeners.filter((l) => l !== listener)) };
  };
  fire(value?: unknown) {
    for (const l of this.listeners) l(value);
  }
  dispose() {}
}
const noEvent = () => ({ dispose() {} });

const commands = new Map<string, (...args: unknown[]) => unknown>();
const treeViews = new Map<string, { provider: { getChildren(n?: unknown): unknown; getTreeItem(n: unknown): unknown }; view: Record<string, unknown> }>();
const webviewProviders = new Map<string, { resolveWebviewView(view: unknown): void }>();
const statusItems: Record<string, unknown>[] = [];
const terminals: Record<string, unknown>[] = [];
const statusMessages: string[] = [];
const quickPicks: { items: { label: string; description?: string }[]; title?: string }[] = [];
/** Progress notifications: open until their task ends. */
const progress: { title: string; open: boolean }[] = [];
type Listener0 = () => void;
class MockQuickPick {
  title = "";
  placeholder = "";
  items: { label: string; kind?: number }[] = [];
  activeItems: unknown[] = [];
  selectedItems: unknown[] = [];
  visible = false;
  private accepted: Listener0[] = [];
  private hidden: Listener0[] = [];
  onDidAccept = (fn: Listener0) => (this.accepted.push(fn), { dispose() {} });
  onDidHide = (fn: Listener0) => (this.hidden.push(fn), { dispose() {} });
  show() {
    this.visible = true;
    livePicks.push(this);
  }
  hide() {
    if (!this.visible) return;
    this.visible = false;
    this.hidden.forEach((fn) => fn());
  }
  dispose() {}
  accept(text: string) {
    this.selectedItems = this.items.filter((i) => i.label.includes(text)).slice(0, 1);
    this.accepted.forEach((fn) => fn());
  }
}
const livePicks: MockQuickPick[] = [];
const serializers = new Map<string, { deserializeWebviewPanel(panel: unknown, state: unknown): Promise<void> }>();
const panels: MockPanel[] = [];
const VERSION = process.env.VSCODE_VERSION ?? "1.139.1";
const executed: string[] = [];
const messages: string[] = [];
const contexts = new Map<string, unknown>();
const settings: Record<string, unknown> = {
  "malac.path": process.env.MALAC_BIN ?? "malac",
  "malac.home": process.env.MALAC_HOME ?? "",
  "malac.folderTab": process.env.FOLDER_TAB ?? "folder",
};

class Uri {
  constructor(
    readonly scheme: string,
    readonly fsPath: string,
  ) {}
  get path() {
    return this.fsPath;
  }
  static file(p: string) {
    return new Uri("file", p);
  }
  static joinPath(base: Uri, ...parts: string[]) {
    return new Uri(base.scheme, path.join(base.fsPath, ...parts));
  }
  static from({ scheme, path: p }: { scheme: string; path: string }) {
    return new Uri(scheme, p);
  }
  static parse(text: string) {
    return new Uri("https", text);
  }
  toString() {
    return `${this.scheme}://${this.fsPath}`;
  }
}

interface MockWebview {
  options: unknown;
  html: string;
  cspSource: string;
  posted: { type: string; state?: Record<string, unknown> }[];
  receive: (m: unknown) => void;
  asWebviewUri(uri: Uri): Uri;
  postMessage(m: never): Promise<void>;
  onDidReceiveMessage(fn: (m: unknown) => void): { dispose(): void };
}

function mockWebview(): MockWebview {
  const webview: MockWebview = {
    options: {},
    html: "",
    cspSource: "vscode-webview://test",
    posted: [],
    receive: () => {},
    asWebviewUri: (uri: Uri) => uri,
    postMessage: async (m: never) => void webview.posted.push(m),
    onDidReceiveMessage: (fn: (m: unknown) => void) => {
      webview.receive = fn;
      return { dispose() {} };
    },
  };
  return webview;
}

interface MockPanel {
  viewType: string;
  webview: MockWebview;
  title: string;
  iconPath: unknown;
  visible: boolean;
  active: boolean;
  revealed: number;
  reveal(): void;
  dispose(): void;
  onDidChangeViewState: typeof noEvent;
  onDidDispose: (fn: () => void) => { dispose(): void };
}

function mockPanel(viewType: string): MockPanel {
  const closers: (() => void)[] = [];
  const panel: MockPanel = {
    viewType,
    webview: mockWebview(),
    title: "",
    iconPath: undefined,
    visible: true,
    active: true,
    revealed: 0,
    reveal: () => void (panel.revealed += 1),
    dispose: () => closers.splice(0).forEach((f) => f()),
    onDidChangeViewState: noEvent,
    onDidDispose: (fn) => {
      closers.push(fn);
      return { dispose() {} };
    },
  };
  panels.push(panel);
  return panel;
}

const vscode = {
  version: VERSION,
  EventEmitter,
  Uri,
  ThemeIcon: class {
    constructor(
      readonly id: string,
      readonly color?: unknown,
    ) {}
  },
  ThemeColor: class {
    constructor(readonly id: string) {}
  },
  MarkdownString: class {
    isTrusted: unknown;
    constructor(public value = "") {}
    appendMarkdown(text: string) {
      this.value += text;
      return this;
    }
  },
  TreeItem: class {
    id?: string;
    constructor(
      readonly label: string,
      readonly collapsibleState: number,
    ) {}
  },
  TreeItemCollapsibleState: { None: 0, Collapsed: 1, Expanded: 2 },
  StatusBarAlignment: { Left: 1, Right: 2 },
  ProgressLocation: { Notification: 15, Window: 10 },
  QuickPickItemKind: { Separator: -1, Default: 0 },
  ConfigurationTarget: { Global: 1, Workspace: 2 },
  ExtensionKind: { UI: 1, Workspace: 2 },
  ViewColumn: { Beside: -2, Active: -1 },
  Range: class {},
  TabInputText: class {},
  window: {
    activeTextEditor: undefined,
    visibleTextEditors: [],
    tabGroups: { all: [] },
    createOutputChannel: () => ({ appendLine: (line: string) => messages.push(`[log] ${line}`), append() {}, show() {}, dispose() {} }),
    createTreeView: (id: string, options: { treeDataProvider: never }) => {
      const view = { visible: false, badge: undefined, message: undefined, description: undefined, onDidChangeVisibility: noEvent, dispose() {} };
      treeViews.set(id, { provider: options.treeDataProvider, view });
      return view;
    },
    createStatusBarItem: (id: string, alignment: number, priority: number) => {
      const item: Record<string, unknown> = { id, alignment, priority, show() {}, dispose() {} };
      statusItems.push(item);
      return item;
    },
    registerWebviewPanelSerializer: (type: string, serializer: never) => {
      serializers.set(type, serializer);
      return { dispose() {} };
    },
    createWebviewPanel: (type: string) => mockPanel(type),
    registerWebviewViewProvider: (id: string, provider: never) => {
      webviewProviders.set(id, provider);
      return { dispose() {} };
    },
    onDidChangeActiveTextEditor: noEvent,
    onDidChangeTextEditorSelection: noEvent,
    onDidCloseTerminal: noEvent,
    setStatusBarMessage: (text: string) => {
      statusMessages.push(text);
      return { dispose() {} };
    },
    showInformationMessage: async (text: string) => void messages.push(`[info] ${text}`),
    showWarningMessage: async (text: string) => void messages.push(`[warning] ${text}`),
    showErrorMessage: async (text: string) => void messages.push(`[error] ${text}`),
    showQuickPick: async (items: { label: string; description?: string }[], options?: { title?: string }) => {
      quickPicks.push({ items: await items, title: options?.title });
      return undefined;
    },
    withProgress: async (options: { title?: string }, task: (p: unknown) => Promise<unknown>) => {
      const entry = { title: options.title ?? "", open: true };
      progress.push(entry);
      try {
        return await task({ report() {} });
      } finally {
        entry.open = false;
      }
    },
    createQuickPick: () => new MockQuickPick(),
    createTerminal: (options: Record<string, unknown>) => {
      terminals.push(options);
      return { exitStatus: undefined, show() {}, dispose() {} };
    },
  },
  workspace: {
    workspaceFolders: [{ uri: Uri.file(CWD), name: path.basename(CWD), index: 0 }],
    textDocuments: [],
    getConfiguration: (section?: string) => ({
      get: <T>(key: string, fallback?: T) => (settings[section ? `${section}.${key}` : key] ?? fallback) as T,
    }),
    createFileSystemWatcher: () => ({ onDidCreate: noEvent, onDidDelete: noEvent, onDidChange: noEvent, dispose() {} }),
    onDidChangeWorkspaceFolders: noEvent,
    onDidChangeTextDocument: noEvent,
    onDidSaveTextDocument: noEvent,
    onDidCloseTextDocument: noEvent,
    onDidChangeConfiguration: noEvent,
    registerTextDocumentContentProvider: () => ({ dispose() {} }),
    asRelativePath: (uri: Uri) => path.relative(CWD, uri.fsPath),
    findFiles: async () => [],
  },
  commands: {
    registerCommand: (id: string, fn: (...args: unknown[]) => unknown) => {
      commands.set(id, fn);
      return { dispose: () => commands.delete(id) };
    },
    executeCommand: async (id: string, ...args: unknown[]) => {
      executed.push(id);
      if (id === "setContext") contexts.set(String(args[0]), args[1]);
      else if (commands.has(id)) return commands.get(id)!(...args);
      return undefined;
    },
  },
  env: { asExternalUri: async (uri: Uri) => uri },
  languages: { setTextDocumentLanguage: async (d: unknown) => d },
};

const loader = Module as unknown as { _load(request: string, ...rest: unknown[]): unknown };
const load = loader._load;
loader._load = function (request: string, ...rest: unknown[]) {
  if (request === "vscode") return vscode;
  return load.call(this, request, ...rest);
};

const state = new Map<string, unknown>();
const context = {
  subscriptions: [] as { dispose(): void }[],
  extensionUri: Uri.file(ROOT),
  globalStorageUri: Uri.file(path.join(process.env.MALAC_HOME ?? "/tmp", "vscode-storage")),
  extension: { packageJSON: pkg },
  workspaceState: { get: (k: string) => state.get(k), update: async (k: string, v: unknown) => void state.set(k, v) },
};

function check(name: string, ok: boolean, detail = ""): void {
  console.log(`${ok ? "ok  " : "FAIL"} ${name}${detail ? `: ${detail}` : ""}`);
  if (!ok) process.exitCode = 1;
}

async function main(): Promise<void> {
  // eslint-disable-next-line @typescript-eslint/no-require-imports
  const extension = require("../src/extension") as { activate(c: unknown): void; deactivate(): void };
  extension.activate(context);

  const declared: string[] = pkg.contributes.commands.map((c: { command: string }) => c.command);
  const missing = declared.filter((id) => !commands.has(id));
  check("every declared command is registered", missing.length === 0, missing.join(", "));
  const extra = [...commands.keys()].filter((id) => !declared.includes(id));
  check("every registered command is declared", extra.length === 0, extra.join(", "));
  // The chat is in the right-hand panel from VS Code 1.106, else in the activity bar; never both.
  const secondary = !/^1\.(\d|[1-9]\d|10[0-5])\./.test(VERSION);
  const chatId = secondary ? "malac.chatSecondary" : "malac.chat";
  const otherChat = secondary ? "malac.chat" : "malac.chatSecondary";
  check(`VS Code ${VERSION}: the chat view is ${chatId}`, webviewProviders.has(chatId) && !webviewProviders.has(otherChat));
  check("the secondary side bar context key is set", contexts.get("malac.noSecondarySidebar") === !secondary, String(contexts.get("malac.noSecondarySidebar")));
  const views: { id: string; type?: string }[] = Object.values(pkg.contributes.views as Record<string, { id: string; type?: string }[]>).flat();
  for (const view of views.filter((v) => v.id !== otherChat)) {
    const registered = view.type === "webview" ? webviewProviders.has(view.id) : treeViews.has(view.id);
    check(`view ${view.id} is registered`, registered);
  }
  check("editor tabs come back after a reload", serializers.has("malac.chatTab"));
  for (const menu of pkg.contributes.menus["view/item/context"] as { command: string }[]) {
    if (!commands.has(menu.command)) check(`menu command ${menu.command}`, false);
  }

  // Resolve the chat view as VS Code would when it is shown.
  const webview = mockWebview();
  const posted = webview.posted;
  webviewProviders.get(chatId)!.resolveWebviewView({ webview, visible: true, onDidChangeVisibility: noEvent, onDidDispose: noEvent });
  check("the chat page has a strict CSP", /default-src 'none'/.test(webview.html) && /script-src 'nonce-/.test(webview.html));
  webview.receive({ type: "ready" });

  const deadline = Date.now() + 30_000;
  const chatState = () => [...posted].reverse().find((m) => m.type === "state")?.state as { tab: { cwd: string } | null; connection: { state: string } } | undefined;
  while (Date.now() < deadline && !(chatState()?.connection.state === "online" && chatState()?.tab)) {
    await new Promise((r) => setTimeout(r, 200));
  }
  const s = chatState();
  check("the chat connects", s?.connection.state === "online", JSON.stringify(s?.connection));
  const expected = process.env.EXPECT_TAB ?? CWD;
  check("the chat shows the folder's tab", s?.tab?.cwd === expected, s?.tab?.cwd);
  check("a transcript was sent", posted.some((m) => m.type === "transcript"));

  await new Promise((r) => setTimeout(r, 500));
  const tree = treeViews.get("malac.workspaces")!;
  const roots = (await tree.provider.getChildren()) as unknown[];
  const items = roots.map((n) => tree.provider.getTreeItem(n) as { label: string; description: string });
  check("the Workspaces tree lists workspaces", items.length > 0, items.map((i) => `${i.label} (${i.description})`).join("; "));
  const tabs = (await tree.provider.getChildren(roots[0])) as unknown[];
  check("the current workspace lists its tabs", tabs.length > 0, (tabs.map((n) => (tree.provider.getTreeItem(n) as { label: string }).label)).join(", "));
  const sessions = treeViews.get("malac.sessions")!;
  const groups = (await sessions.provider.getChildren()) as unknown[];
  check("the Sessions tree loads", groups.length > 0);
  check("the sessions scope context key is set", contexts.get("malac.sessionsScope") === "folder");
  const status = statusItems[0];
  check("the status bar shows the pig and malac", String(status?.text).startsWith("$(malac-pig)") && String(status?.text).includes("malac"), String(status?.text));
  check("the status bar item is on the right and opens malac", status?.alignment === 2 && status?.command === "malac.open");

  // Editor tabs: each has its own chat, titled after its agent.
  await commands.get("malac.openInNewTab")!();
  const tab = panels.at(-1)!;
  check("Open in New Tab makes a chat panel", tab?.viewType === "malac.chatTab" && /script-src 'nonce-/.test(tab.webview.html));
  tab.webview.receive({ type: "ready" });
  await new Promise((r) => setTimeout(r, 300));
  const tabState = [...tab.webview.posted].reverse().find((m) => m.type === "state")?.state as { tab: { cwd: string } | null; agent: unknown } | undefined;
  check("the new tab starts a new agent in the side chat's tab", tabState?.tab?.cwd === s?.tab?.cwd && tabState?.agent === null, `${tabState?.tab?.cwd} agent=${JSON.stringify(tabState?.agent)}`);
  check("the new tab is titled malac until its agent exists", tab.title === "malac", tab.title);
  if (process.env.SMOKE_PROMPT) {
    // One real turn: the tab is then titled after its new agent.
    tab.webview.receive({ type: "submit", text: "Reply with just the word ok.", attachments: [], images: [] });
    const until = Date.now() + 60_000;
    while (Date.now() < until && tab.title === "malac") await new Promise((r) => setTimeout(r, 250));
    check("the tab is titled after its agent", tab.title.startsWith("Reply with just"), tab.title);
  }
  tab.webview.receive({ type: "toggleContext", enabled: true });
  await commands.get("malac.open")!();
  check("Open brings back the tab used last", tab.revealed >= 2, String(tab.revealed));
  const restored = mockPanel("malac.chatTab");
  const sideTab = (s as unknown as { tab: { id: string } }).tab.id;
  await serializers.get("malac.chatTab")!.deserializeWebviewPanel(restored, { chat: { tab: sideTab, agent: null } });
  restored.webview.receive({ type: "ready" });
  await new Promise((r) => setTimeout(r, 300));
  const back = [...restored.webview.posted].reverse().find((m) => m.type === "state")?.state as { tab: { id: string } | null } | undefined;
  check("a restored tab comes back to its tab", back?.tab?.id === sideTab, back?.tab?.id);
  tab.dispose();
  executed.length = 0;
  await commands.get("malac.open")!();
  check("with the tab closed, Open goes to the side chat", executed.includes(`${chatId}.focus`), executed.join(", "));
  executed.length = 0;
  await commands.get("malac.focusChat")!();
  check("Focus Chat goes to the side chat", executed.includes(`${chatId}.focus`), executed.join(", "));
  if (process.env.SMOKE_VOICE) await voiceChecks(webview.posted, webview.receive, chatId);
  if (process.env.SMOKE_APPROVALS) await approvalChecks(process.env.SMOKE_APPROVALS, status);
  check("no error notifications", !messages.some((m) => m.startsWith("[error]")), messages.filter((m) => m.startsWith("[error]")).join(" | "));
  for (const m of messages.filter((m) => !m.startsWith("[log]"))) console.log(`     ${m}`);

  for (const d of context.subscriptions) d.dispose();
  extension.deactivate();
  setTimeout(() => process.exit(), 200);
}

/** Approval notifications for a tab the chat does not show, answered elsewhere and through Answer Approvals. */
async function approvalChecks(prompt: string, status: Record<string, unknown>): Promise<void> {
  const home = process.env.MALAC_HOME ?? path.join(process.env.HOME ?? "", ".malac");
  const { url, token } = JSON.parse(fs.readFileSync(path.join(home, "daemon.json"), "utf8")) as { url: string; token: string };
  // Another client: the daemon's REST API with the master token ("this computer").
  const command = async (body: Record<string, unknown>) => {
    const response = await fetch(`${url}/api/command`, { method: "POST", headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" }, body: JSON.stringify(body) });
    return (await response.json()) as Record<string, unknown>;
  };
  const until = async (ok: () => boolean, ms = 30_000) => {
    const end = Date.now() + ms;
    while (Date.now() < end && !ok()) await new Promise((r) => setTimeout(r, 100));
    return ok();
  };
  const dir = fs.mkdtempSync(path.join(fs.realpathSync(os.tmpdir()), "malac-smoke-approvals-"));
  const created = await command({ type: "tab_create", cwd: dir, name: "smoke approvals" });
  const tab = String(created.tab);
  const ask = async () => {
    const before = progress.length;
    await command({ type: "prompt", tab, text: prompt });
    await until(() => progress.length > before);
    return progress[before];
  };
  const first = await ask();
  check("a request no visible chat shows gets a notification naming the agent and the call", Boolean(first?.open && /wants to use/.test(first.title)), first?.title);
  await until(() => status.command === "malac.answerApprovals");
  check("while it waits, the status bar answers approvals, in warning colours", status.command === "malac.answerApprovals" && (status.backgroundColor as { id?: string })?.id === "statusBarItem.warningBackground", String(status.command));
  const pending = async () => ((await (await fetch(`${url}/api/snapshot`, { headers: { Authorization: `Bearer ${token}` } })).json()) as { pending: { id: string; tab: string }[] }).pending.filter((p) => p.tab === tab);
  for (const request of await pending()) await command({ type: "approve", request_id: request.id, decision: "allow" });
  check("answered elsewhere, the notification closes by itself", await until(() => !first.open, 10_000));
  await until(() => status.command === "malac.open");
  // Let the first turn end before the next prompt.
  await new Promise((r) => setTimeout(r, 1500));
  const second = await ask();
  check("the next request gets its own notification", Boolean(second?.open));
  const picks = livePicks.length;
  void commands.get("malac.answerApprovals")!();
  await until(() => livePicks.length > picks);
  const pick = livePicks.at(-1)!;
  check("Answer Approvals offers Allow, Always, Deny and Open in chat", ["Allow", "Always", "Deny", "Open in chat"].every((l) => pick.items.some((i) => i.label.includes(l))), pick.items.map((i) => i.label).join(" | "));
  pick.accept("Deny");
  check("answering there closes the notification", await until(() => !second.open, 10_000));
  check("and the status bar opens malac again", await until(() => status.command === "malac.open", 10_000), String(status.command));
  await command({ type: "tab_close", tab, force: true });
  fs.rmSync(dir, { recursive: true, force: true });
}

/** Dictation, talk mode and the voice settings against the daemon's voice status. */
async function voiceChecks(posted: { type: string; state?: Record<string, unknown> }[], receive: (m: unknown) => void, chatId: string): Promise<void> {
  const latest = () => [...posted].reverse().find((m) => m.type === "state")?.state as { voice: { dictation: boolean; talk: boolean; blocked: string | null }; tab: { id: string } } | undefined;
  await new Promise((r) => setTimeout(r, 500));
  const offer = latest()?.voice;
  check("the chat offers the mic and talk buttons", Boolean(offer?.dictation && offer.talk && !offer.blocked), JSON.stringify(offer));
  const dictations = () => posted.filter((m) => m.type === "dictation") as unknown as { state: string; text?: string; error?: string }[];
  await commands.get("malac.dictate")!();
  for (let i = 0; i < 50 && !dictations().length; i += 1) await new Promise((r) => setTimeout(r, 100));
  check("malac: Dictate starts recording in the side chat", dictations().at(-1)?.state === "recording" && executed.includes(`${chatId}.focus`), JSON.stringify(dictations().at(-1)));
  await new Promise((r) => setTimeout(r, 300));
  await commands.get("malac.dictate")!();
  for (let i = 0; i < 100 && dictations().at(-1)?.state !== "done"; i += 1) await new Promise((r) => setTimeout(r, 100));
  const done = dictations().at(-1);
  check("pressing it again stops, and the text goes to the composer", done?.state === "done" && Boolean(done.text), JSON.stringify(done));
  receive({ type: "dictation", action: "toggle" });
  await new Promise((r) => setTimeout(r, 500));
  receive({ type: "dictation", action: "cancel" });
  for (let i = 0; i < 50 && dictations().at(-1)?.state !== "done"; i += 1) await new Promise((r) => setTimeout(r, 100));
  check("Esc in the composer cancels without text", dictations().at(-1)?.state === "done" && !dictations().at(-1)?.text);
  await commands.get("malac.talk")!(latest()!.tab.id, null);
  const terminal = terminals.at(-1);
  check(
    "malac: Talk opens a terminal running malac voice talk for the tab",
    terminal?.shellPath === settings["malac.path"] && JSON.stringify(terminal?.shellArgs) === JSON.stringify(["voice", "talk", "--tab", latest()!.tab.id]),
    JSON.stringify(terminal),
  );
  await commands.get("malac.voiceSettings")!();
  const labels = quickPicks.at(-1)?.items.map((i) => `${i.label.trim()}${i.description ? ` = ${i.description}` : ""}`) ?? [];
  check("malac: Voice Settings lists each feature, its model and voice, and this device's switch", ["Dictation", "Speech", "Talk mode", "Speak on this device", "Language"].every((name) => labels.some((l) => l.includes(name))), labels.join(" | "));
  for (const label of labels) console.log(`     ${label}`);
  const prompt = process.env.SMOKE_SAY_PROMPT;
  if (!prompt) return;
  receive({ type: "submit", text: prompt, attachments: [], images: [] });
  const until = Date.now() + 30_000;
  while (Date.now() < until && !statusMessages.some((m) => m.startsWith("$(unmute)"))) await new Promise((r) => setTimeout(r, 250));
  check("the approval is spoken on this device", statusMessages.some((m) => m.startsWith("$(unmute)")), statusMessages.join(" | "));
  const pending = (latest() as unknown as { pending: { id: string }[] }).pending;
  for (const request of pending) receive({ type: "approve", request: request.id, decision: "deny" });
  await new Promise((r) => setTimeout(r, 500));
}

void main();

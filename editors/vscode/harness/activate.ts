// Smoke test of the extension's activation against a real daemon, with a
// small stand-in for the `vscode` module: every command and view declared in
// package.json must be registered, the chat must come up on the folder's
// tab, and the trees and status bar must reflect the daemon.
//
//   MALAC_HOME=/tmp/home HARNESS_CWD=/some/project MALAC_BIN=malac node out/harness/activate.js
//
// FOLDER_TAB sets malac.folderTab (default "folder"); EXPECT_TAB is the tab
// directory the chat should end up in (default HARNESS_CWD).

import * as fs from "node:fs";
import Module from "node:module";
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

const vscode = {
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
  ViewColumn: { Beside: -2 },
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
    createStatusBarItem: () => {
      const item: Record<string, unknown> = { show() {}, dispose() {} };
      statusItems.push(item);
      return item;
    },
    registerWebviewViewProvider: (id: string, provider: never) => {
      webviewProviders.set(id, provider);
      return { dispose() {} };
    },
    onDidChangeActiveTextEditor: noEvent,
    onDidChangeTextEditorSelection: noEvent,
    showInformationMessage: async (text: string) => void messages.push(`[info] ${text}`),
    showWarningMessage: async (text: string) => void messages.push(`[warning] ${text}`),
    showErrorMessage: async (text: string) => void messages.push(`[error] ${text}`),
    showQuickPick: async () => undefined,
    withProgress: async (_: unknown, task: () => Promise<unknown>) => task(),
    createTerminal: () => ({ show() {} }),
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
  const views: { id: string; type?: string }[] = pkg.contributes.views.malac;
  for (const view of views) {
    const registered = view.type === "webview" ? webviewProviders.has(view.id) : treeViews.has(view.id);
    check(`view ${view.id} is registered`, registered);
  }
  for (const menu of pkg.contributes.menus["view/item/context"] as { command: string }[]) {
    if (!commands.has(menu.command)) check(`menu command ${menu.command}`, false);
  }

  // Resolve the chat view as VS Code would when it is shown.
  const posted: { type: string; state?: Record<string, unknown> }[] = [];
  let receive: (m: unknown) => void = () => {};
  const webview = {
    options: {},
    html: "",
    cspSource: "vscode-webview://test",
    asWebviewUri: (uri: Uri) => uri,
    postMessage: async (m: never) => void posted.push(m),
    onDidReceiveMessage: (fn: (m: unknown) => void) => {
      receive = fn;
      return { dispose() {} };
    },
  };
  webviewProviders.get("malac.chat")!.resolveWebviewView({ webview, visible: true, onDidChangeVisibility: noEvent, onDidDispose: noEvent });
  check("the chat page has a strict CSP", /default-src 'none'/.test(webview.html) && /script-src 'nonce-/.test(webview.html));
  receive({ type: "ready" });

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
  check("the status bar shows malac", String(status?.text).includes("malac"), String(status?.text));
  check("no error notifications", !messages.some((m) => m.startsWith("[error]")), messages.filter((m) => m.startsWith("[error]")).join(" | "));
  for (const m of messages.filter((m) => !m.startsWith("[log]"))) console.log(`     ${m}`);

  for (const d of context.subscriptions) d.dispose();
  extension.deactivate();
  setTimeout(() => process.exit(), 200);
}

void main();

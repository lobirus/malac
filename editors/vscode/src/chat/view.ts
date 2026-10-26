// The Chat webview view: VS Code's side of the ChatHost. It renders the page,
// relays messages to the ChatController, follows the active editor for the
// context chip and answers the "@" picker from the workspace's files.

import { randomBytes } from "node:crypto";
import * as fs from "node:fs/promises";
import * as path from "node:path";
import * as vscode from "vscode";
import type { EditorSnapshot } from "../model/context";
import { FileHit, rankFiles } from "../model/files";
import { ChatController, ChatHost } from "./controller";
import { chatHtml } from "./html";
import type { FromWebview, ToWebview } from "./messages";

const FILE_CACHE_MS = 30_000;
const FILE_LIMIT = 20_000;
const UPLOAD_MAX_AGE_MS = 7 * 24 * 3600 * 1000;

export class ChatView implements vscode.WebviewViewProvider, ChatHost, vscode.Disposable {
  static readonly id = "malac.chat";

  private view: vscode.WebviewView | undefined;
  private queue: ToWebview[] = [];
  private files: { at: number; hits: FileHit[] } | null = null;
  private lastEditor: vscode.TextEditor | undefined = vscode.window.activeTextEditor;
  private disposables: vscode.Disposable[] = [];
  controller!: ChatController;
  /** Called when the view first appears. */
  onResolve: () => void = () => {};
  readonly dropHint = "Drop files here (hold Shift while dragging in VS Code)";

  constructor(
    private readonly context: vscode.ExtensionContext,
    private readonly output: vscode.OutputChannel,
    private readonly version: () => string | undefined,
  ) {
    const watcher = vscode.workspace.createFileSystemWatcher("**/*", false, true, false);
    const invalidate = () => (this.files = null);
    this.disposables.push(
      watcher,
      watcher.onDidCreate(invalidate),
      watcher.onDidDelete(invalidate),
      vscode.workspace.onDidChangeWorkspaceFolders(invalidate),
      vscode.window.onDidChangeActiveTextEditor((editor) => {
        if (editor || !vscode.window.visibleTextEditors.includes(this.lastEditor!)) this.lastEditor = editor;
        this.controller?.editorChanged();
      }),
      vscode.window.onDidChangeTextEditorSelection((event) => {
        if (event.textEditor === this.lastEditor) this.controller?.editorChanged();
      }),
      vscode.workspace.onDidChangeTextDocument((event) => {
        if (event.document === this.lastEditor?.document && event.document.isDirty !== this.lastDirty) {
          this.lastDirty = event.document.isDirty;
          this.controller?.editorChanged();
        }
      }),
      vscode.workspace.onDidSaveTextDocument(() => this.controller?.editorChanged()),
    );
    void this.pruneUploads();
  }

  private lastDirty = false;

  get visible(): boolean {
    return Boolean(this.view?.visible);
  }

  dispose(): void {
    for (const d of this.disposables) d.dispose();
  }

  resolveWebviewView(view: vscode.WebviewView): void {
    this.view = view;
    this.onResolve();
    const media = vscode.Uri.joinPath(this.context.extensionUri, "media");
    const scripts = vscode.Uri.joinPath(this.context.extensionUri, "out", "src", "webview");
    view.webview.options = { enableScripts: true, localResourceRoots: [media, scripts] };
    const nonce = randomBytes(16).toString("base64");
    view.webview.html = chatHtml({
      cspSource: view.webview.cspSource,
      nonce,
      scriptUri: view.webview.asWebviewUri(vscode.Uri.joinPath(scripts, "main.mjs")).toString(),
      styleUri: view.webview.asWebviewUri(vscode.Uri.joinPath(media, "chat.css")).toString(),
    });
    view.webview.onDidReceiveMessage((message: FromWebview) => this.controller.receive(message), undefined, this.disposables);
    view.onDidChangeVisibility(() => {
      if (view.visible) this.controller.editorChanged();
    });
    view.onDidDispose(() => {
      if (this.view === view) this.view = undefined;
    });
  }

  /** Reveals the chat and focuses the composer. */
  async reveal(text?: string): Promise<void> {
    await vscode.commands.executeCommand(`${ChatView.id}.focus`);
    this.controller.focus(text);
  }

  // ChatHost

  post(message: ToWebview): void {
    if (!this.view) {
      // Only what the page cannot rebuild on "ready" is worth keeping.
      if (message.type === "attach" || message.type === "focus") this.queue.push(message);
      return;
    }
    if (message.type === "state" && this.queue.length) {
      for (const queued of this.queue.splice(0)) void this.view.webview.postMessage(queued);
    }
    void this.view.webview.postMessage(message);
  }

  editor(): EditorSnapshot | null {
    const editor = this.lastEditor ?? vscode.window.activeTextEditor;
    if (!editor) return null;
    const { document, selection } = editor;
    const local = document.uri.scheme === "file" || document.uri.scheme === "vscode-remote";
    if (!local) return null;
    this.lastDirty = document.isDirty;
    return {
      path: document.uri.fsPath,
      displayPath: vscode.workspace.asRelativePath(document.uri, false),
      languageId: document.languageId,
      selection: selection.isEmpty
        ? null
        : { startLine: selection.start.line, startCharacter: selection.start.character, endLine: selection.end.line, endCharacter: selection.end.character },
      selectedText: selection.isEmpty ? "" : document.getText(selection),
      dirty: document.isDirty,
      text: () => document.getText(),
    };
  }

  async findFiles(query: string): Promise<FileHit[]> {
    if (!this.files || Date.now() - this.files.at > FILE_CACHE_MS) {
      const uris = await vscode.workspace.findFiles("**/*", excludeGlob(), FILE_LIMIT);
      this.files = {
        at: Date.now(),
        hits: uris.map((uri) => ({ path: uri.fsPath, relative: vscode.workspace.asRelativePath(uri, true) })),
      };
    }
    const open = vscode.window.tabGroups.all
      .flatMap((group) => group.tabs)
      .map((tab) => (tab.input instanceof vscode.TabInputText ? tab.input.uri.fsPath : null))
      .filter((p): p is string => Boolean(p));
    return rankFiles(this.files.hits, query, 30, open);
  }

  openFile(file: string, line?: number): void {
    const uri = vscode.Uri.file(file);
    const selection = line ? new vscode.Range(line - 1, 0, line - 1, 0) : undefined;
    void vscode.window.showTextDocument(uri, { selection, preview: true }).then(undefined, (error: Error) => {
      void vscode.window.showWarningMessage(`Cannot open ${file}: ${error.message}`);
    });
  }

  async saveUpload(name: string, data: Buffer): Promise<string> {
    const dir = path.join(this.context.globalStorageUri.fsPath, "uploads", new Date().toISOString().replace(/[:.]/g, "-"));
    await fs.mkdir(dir, { recursive: true });
    const safe = path.basename(name).replace(/[^\w.\- ]+/g, "_") || "file";
    const target = path.join(dir, safe);
    await fs.writeFile(target, data);
    return target;
  }

  runCommand(command: string, args: unknown[]): void {
    void vscode.commands.executeCommand(command, ...args);
  }

  remember(key: string, value: unknown): void {
    void this.context.workspaceState.update(`malac.${key}`, value);
  }

  recall<T>(key: string): T | undefined {
    return this.context.workspaceState.get<T>(`malac.${key}`);
  }

  log(line: string): void {
    this.output.appendLine(line);
  }

  clientVersion(): string | undefined {
    return this.version();
  }

  /** Dropped files are kept for a week. */
  private async pruneUploads(): Promise<void> {
    const root = path.join(this.context.globalStorageUri.fsPath, "uploads");
    try {
      for (const entry of await fs.readdir(root, { withFileTypes: true })) {
        const full = path.join(root, entry.name);
        const stat = await fs.stat(full);
        if (Date.now() - stat.mtimeMs > UPLOAD_MAX_AGE_MS) await fs.rm(full, { recursive: true, force: true });
      }
    } catch {
      // Nothing uploaded yet.
    }
  }
}

/** `files.exclude` and `search.exclude` together, plus build output that is never worth attaching. */
function excludeGlob(): string {
  const patterns = new Set(["**/node_modules", "**/.git", "**/target", "**/out", "**/dist"]);
  for (const section of ["files.exclude", "search.exclude"]) {
    const table = vscode.workspace.getConfiguration().get<Record<string, unknown>>(section) ?? {};
    for (const [pattern, on] of Object.entries(table)) if (on === true && !/[{}]/.test(pattern)) patterns.add(pattern);
  }
  return `{${[...patterns].map((p) => (p.endsWith("/**") ? p : `${p}/**,${p}`)).join(",")}}`;
}

// What every chat surface (the side-bar view and editor tabs) shares on the
// VS Code side: the active editor for the context chip, the workspace's files
// for "@", dropped uploads, persisted settings and the log.

import { randomBytes } from "node:crypto";
import * as fs from "node:fs/promises";
import * as path from "node:path";
import * as vscode from "vscode";
import type { EditorSnapshot } from "../model/context";
import { Emitter } from "../model/emitter";
import { FileHit, rankFiles } from "../model/files";
import type { ChatHost } from "./controller";
import { chatHtml } from "./html";
import type { ToWebview } from "./messages";

const FILE_CACHE_MS = 30_000;
const FILE_LIMIT = 20_000;
const UPLOAD_MAX_AGE_MS = 7 * 24 * 3600 * 1000;

export const DROP_HINT = "Drop files here (hold Shift while dragging in VS Code)";

export class ChatEnvironment implements vscode.Disposable {
  private files: { at: number; hits: FileHit[] } | null = null;
  private lastEditor: vscode.TextEditor | undefined = vscode.window.activeTextEditor;
  private lastDirty = false;
  private disposables: vscode.Disposable[] = [];
  /** The active editor, its selection or its saved state changed. */
  readonly onEditorChanged = new Emitter<void>();

  constructor(
    readonly context: vscode.ExtensionContext,
    private readonly output: vscode.OutputChannel,
    private readonly version: () => string | undefined,
  ) {
    const watcher = vscode.workspace.createFileSystemWatcher("**/*", false, true, false);
    const invalidate = () => (this.files = null);
    const changed = () => this.onEditorChanged.fire();
    this.disposables.push(
      watcher,
      watcher.onDidCreate(invalidate),
      watcher.onDidDelete(invalidate),
      vscode.workspace.onDidChangeWorkspaceFolders(invalidate),
      vscode.window.onDidChangeActiveTextEditor((editor) => {
        // Focusing a chat (a view, or an editor tab of its own) leaves no
        // active text editor; the chip keeps the file the user came from
        // while it is open.
        if (editor || this.lastEditor?.document.isClosed) this.lastEditor = editor;
        changed();
      }),
      vscode.window.onDidChangeTextEditorSelection((event) => {
        if (event.textEditor === this.lastEditor) changed();
      }),
      vscode.workspace.onDidChangeTextDocument((event) => {
        if (event.document === this.lastEditor?.document && event.document.isDirty !== this.lastDirty) {
          this.lastDirty = event.document.isDirty;
          changed();
        }
      }),
      vscode.workspace.onDidSaveTextDocument(changed),
      vscode.workspace.onDidCloseTextDocument((document) => {
        if (document === this.lastEditor?.document) {
          this.lastEditor = vscode.window.activeTextEditor;
          changed();
        }
      }),
    );
    void this.pruneUploads();
  }

  dispose(): void {
    for (const d of this.disposables) d.dispose();
    this.onEditorChanged.dispose();
  }

  /** The chat page for a webview (view or panel). */
  html(webview: vscode.Webview): string {
    const media = vscode.Uri.joinPath(this.context.extensionUri, "media");
    const scripts = vscode.Uri.joinPath(this.context.extensionUri, "out", "src", "webview");
    webview.options = { enableScripts: true, localResourceRoots: [media, scripts] };
    return chatHtml({
      cspSource: webview.cspSource,
      nonce: randomBytes(16).toString("base64"),
      scriptUri: webview.asWebviewUri(vscode.Uri.joinPath(scripts, "main.mjs")).toString(),
      styleUri: webview.asWebviewUri(vscode.Uri.joinPath(media, "chat.css")).toString(),
    });
  }

  editor(): EditorSnapshot | null {
    const editor = this.lastEditor ?? vscode.window.activeTextEditor;
    if (!editor || editor.document.isClosed) return null;
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

/**
 * The part of a ChatHost that is the same for every surface; a surface adds
 * how it posts to its webview and may keep some settings to itself.
 */
export abstract class SurfaceHost implements ChatHost {
  readonly dropHint = DROP_HINT;

  constructor(protected readonly env: ChatEnvironment) {}

  abstract post(message: ToWebview): void;

  editor(): EditorSnapshot | null {
    return this.env.editor();
  }

  findFiles(query: string): Promise<FileHit[]> {
    return this.env.findFiles(query);
  }

  openFile(file: string, line?: number): void {
    this.env.openFile(file, line);
  }

  saveUpload(name: string, data: Buffer): Promise<string> {
    return this.env.saveUpload(name, data);
  }

  runCommand(command: string, args: unknown[]): void {
    this.env.runCommand(command, args);
  }

  remember(key: string, value: unknown): void {
    this.env.remember(key, value);
  }

  recall<T>(key: string): T | undefined {
    return this.env.recall<T>(key);
  }

  log(line: string): void {
    this.env.log(line);
  }

  clientVersion(): string | undefined {
    return this.env.clientVersion();
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

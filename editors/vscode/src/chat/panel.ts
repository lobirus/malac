// The chat in an editor tab ("malac: Open in New Tab"). Each tab has its own
// ChatController, so it follows its own agent, and is titled after it. Open
// tabs come back after a reload: the page keeps its tab and agent in the
// webview state, which the panel serializer hands back.

import * as vscode from "vscode";
import type { DaemonClient } from "../connection/client";
import type { DaemonStore } from "../model/store";
import { tabTitle } from "../model/surfaces";
import { ChatController, ChatVoice } from "./controller";
import { ChatEnvironment, SurfaceHost } from "./environment";
import type { FromWebview, ToWebview } from "./messages";

export const CHAT_PANEL_TYPE = "malac.chatTab";

/** What a tab's page saves for a reload. */
export interface PanelState {
  chat?: { tab?: string | null; agent?: string | null };
}

let nextId = 1;

export class ChatPanel extends SurfaceHost implements vscode.Disposable {
  readonly id = `tab-${nextId++}`;
  readonly controller: ChatController;
  private disposables: vscode.Disposable[] = [];
  private disposed = false;
  /** The chat's own tab: editor tabs do not move the side bar's. */
  private local = new Map<string, unknown>();
  onActive: () => void = () => {};
  onClose: () => void = () => {};

  constructor(
    env: ChatEnvironment,
    private readonly store: DaemonStore,
    client: DaemonClient,
    readonly panel: vscode.WebviewPanel,
    start: { tab: string | null; agent: string | null | undefined },
    options: { includeCurrentFile: boolean; voice?: ChatVoice },
  ) {
    super(env);
    this.local.set("chat.tab", start.tab);
    this.controller = new ChatController(store, client, this, { includeCurrentFile: options.includeCurrentFile, voice: options.voice, titles: false });
    if (start.tab && start.agent !== undefined) this.controller.selectTab(start.tab, start.agent);
    const media = vscode.Uri.joinPath(env.context.extensionUri, "media");
    panel.iconPath = { light: vscode.Uri.joinPath(media, "pig-light.svg"), dark: vscode.Uri.joinPath(media, "pig-dark.svg") };
    panel.webview.html = env.html(panel.webview);
    this.disposables.push(
      panel.webview.onDidReceiveMessage((message: FromWebview) => {
        if (message.type !== "ready") this.onActive();
        this.controller.receive(message);
      }),
      panel.onDidChangeViewState(() => {
        if (panel.active) this.onActive();
        if (panel.visible) this.controller.editorChanged();
      }),
      panel.onDidDispose(() => this.dispose()),
      env.onEditorChanged.event(() => this.controller.editorChanged()),
      this.controller.onSelection.event(() => this.retitle()),
      store.onChange.event((change) => {
        if (change.agents || change.tabs || change.snapshot) this.retitle();
      }),
    );
    this.retitle();
  }

  get open(): boolean {
    return !this.disposed;
  }

  get visible(): boolean {
    return !this.disposed && this.panel.visible;
  }

  reveal(text?: string): void {
    this.panel.reveal(undefined, false);
    this.onActive();
    this.controller.focus(text);
  }

  post(message: ToWebview): void {
    if (!this.disposed) void this.panel.webview.postMessage(message);
  }

  override remember(key: string, value: unknown): void {
    if (key === "chat.tab") this.local.set(key, value);
    else super.remember(key, value);
  }

  override recall<T>(key: string): T | undefined {
    return key === "chat.tab" ? (this.local.get(key) as T | undefined) : super.recall<T>(key);
  }

  private retitle(): void {
    if (this.disposed) return;
    const agentId = this.controller.currentAgent;
    const agent = agentId ? this.store.agents.get(agentId) : undefined;
    const name = agent ? (!agent.parent && agent.title) || agent.label : "";
    const title = tabTitle(name ? { name } : null);
    if (this.panel.title !== title) this.panel.title = title;
  }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    for (const d of this.disposables) d.dispose();
    this.controller.dispose();
    this.panel.dispose();
    this.onClose();
  }
}

export function createChatPanel(): vscode.WebviewPanel {
  return vscode.window.createWebviewPanel(CHAT_PANEL_TYPE, "malac", { viewColumn: vscode.ViewColumn.Active, preserveFocus: false }, panelOptions());
}

export function panelOptions(): vscode.WebviewPanelOptions & vscode.WebviewOptions {
  return { enableScripts: true, retainContextWhenHidden: true, enableFindWidget: true };
}

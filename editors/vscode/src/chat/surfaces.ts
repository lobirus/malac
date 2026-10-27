// Every place the chat lives: the side-bar view (the right-hand panel where
// VS Code supports it) and any editor tabs. Commands go through here to the
// right one.

import * as vscode from "vscode";
import type { DaemonClient } from "../connection/client";
import type { DaemonStore } from "../model/store";
import { lastUsedSurface } from "../model/surfaces";
import type { PendingApproval } from "../protocol";
import { ChatController, ChatVoice } from "./controller";
import type { ChatEnvironment } from "./environment";
import { ChatPanel, createChatPanel, PanelState, panelOptions } from "./panel";
import { ChatView } from "./view";

export class ChatSurfaces implements vscode.Disposable, vscode.WebviewPanelSerializer<PanelState> {
  readonly side: ChatView;
  /** The side-bar chat's controller: the window's main chat. */
  readonly chat: ChatController;
  private panels = new Map<string, ChatPanel>();
  private lastActive: string | null = null;
  private disposables: vscode.Disposable[] = [];

  constructor(
    private readonly env: ChatEnvironment,
    private readonly store: DaemonStore,
    private readonly client: DaemonClient,
    sideId: string,
    private readonly options: () => { includeCurrentFile: boolean; voice?: ChatVoice },
  ) {
    this.side = new ChatView(env, sideId);
    this.chat = new ChatController(store, client, this.side, options());
    this.side.controller = this.chat;
    this.side.onActive = () => (this.lastActive = sideId);
    this.disposables.push(this.chat, env.onEditorChanged.event(() => this.chat.editorChanged()));
  }

  dispose(): void {
    for (const panel of [...this.panels.values()]) panel.dispose();
    for (const d of this.disposables) d.dispose();
  }

  /** Reveals the side-bar chat (the right-hand panel where it exists). */
  async revealSide(text?: string): Promise<void> {
    this.client.ensureStarted();
    await this.side.reveal(text);
  }

  /** "malac: Open": the editor tab used last while it is open, else the side-bar chat. */
  async openLast(): Promise<void> {
    const id = lastUsedSurface(this.surfaces(), this.lastActive);
    const panel = this.panels.get(id);
    if (panel) {
      this.client.ensureStarted();
      panel.reveal();
    } else {
      await this.revealSide();
    }
  }

  /** A new editor tab with a new agent in the side chat's tab. */
  openInNewTab(): ChatPanel {
    this.client.ensureStarted();
    const panel = this.adopt(createChatPanel(), { tab: this.chat.currentTab, agent: null });
    panel.reveal();
    return panel;
  }

  /** Brings back an editor tab after a reload. */
  async deserializeWebviewPanel(webview: vscode.WebviewPanel, state: PanelState | undefined): Promise<void> {
    this.client.ensureStarted();
    webview.webview.options = panelOptions();
    this.adopt(webview, { tab: state?.chat?.tab ?? null, agent: state?.chat?.agent ?? undefined });
  }

  private adopt(webview: vscode.WebviewPanel, start: { tab: string | null; agent: string | null | undefined }): ChatPanel {
    const panel = new ChatPanel(this.env, this.store, this.client, webview, start, this.options());
    this.panels.set(panel.id, panel);
    panel.onActive = () => (this.lastActive = panel.id);
    panel.onClose = () => {
      this.panels.delete(panel.id);
      if (this.lastActive === panel.id) this.lastActive = null;
    };
    return panel;
  }

  /**
   * The chat a command works on: an editor tab while it is the one in use
   * (open and visible), else the side-bar chat.
   */
  target(): { controller: ChatController; reveal: (text?: string) => Promise<void> } {
    const panel = this.lastActive ? this.panels.get(this.lastActive) : undefined;
    if (panel?.visible) {
      return {
        controller: panel.controller,
        reveal: async (text) => {
          this.client.ensureStarted();
          panel.reveal(text);
        },
      };
    }
    return { controller: this.chat, reveal: (text) => this.revealSide(text) };
  }

  /** Whether a visible chat shows this request's tab, so no notification is needed. */
  shows(request: Pick<PendingApproval, "tab" | "agent">): boolean {
    const surfaces: { visible: boolean; controller: ChatController }[] = [{ visible: this.side.visible, controller: this.chat }, ...this.panels.values()];
    return surfaces.some(({ visible, controller }) => {
      const tab = controller.currentTab;
      if (!visible || !tab) return false;
      if (request.tab === tab) return true;
      const view = this.store.tab(tab)?.tab;
      return Boolean(view && this.store.agentsOfTab(view).some((a) => a.id === request.agent));
    });
  }

  private surfaces() {
    return [
      { id: this.side.id, kind: "side" as const, open: true, visible: this.side.visible },
      ...[...this.panels.values()].map((p) => ({ id: p.id, kind: "tab" as const, open: p.open, visible: p.visible })),
    ];
  }
}

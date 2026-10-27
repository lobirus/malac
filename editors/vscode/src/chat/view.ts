// The side-bar chat: a webview view in the right-hand panel (VS Code 1.106
// and later) or in malac's activity-bar container on older versions. Which
// one is picked at activation; the other view stays hidden by its `when`.

import * as vscode from "vscode";
import type { ChatController } from "./controller";
import { ChatEnvironment, SurfaceHost } from "./environment";
import type { FromWebview, ToWebview } from "./messages";

export class ChatView extends SurfaceHost implements vscode.WebviewViewProvider {
  private view: vscode.WebviewView | undefined;
  private queue: ToWebview[] = [];
  controller!: ChatController;
  /** Called when the view first appears. */
  onResolve: () => void = () => {};
  /** Called when the user works in this chat. */
  onActive: () => void = () => {};

  constructor(
    env: ChatEnvironment,
    readonly id: string,
  ) {
    super(env);
  }

  get visible(): boolean {
    return Boolean(this.view?.visible);
  }

  resolveWebviewView(view: vscode.WebviewView): void {
    this.view = view;
    this.onResolve();
    view.webview.html = this.env.html(view.webview);
    view.webview.onDidReceiveMessage((message: FromWebview) => {
      if (message.type !== "ready") this.onActive();
      this.controller.receive(message);
    });
    view.onDidChangeVisibility(() => {
      if (view.visible) this.controller.editorChanged();
    });
    view.onDidDispose(() => {
      if (this.view === view) this.view = undefined;
    });
  }

  /** Reveals the chat and focuses the composer. */
  async reveal(text?: string): Promise<void> {
    await vscode.commands.executeCommand(`${this.id}.focus`);
    this.onActive();
    this.controller.focus(text);
  }

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
}

// The Usage view: a script-free webview with bars from `GET /api/usage`.

import { randomBytes } from "node:crypto";
import * as vscode from "vscode";
import { DaemonClient, HttpError } from "../connection/client";
import { usageHtml, usageModel } from "../model/usage";
import type { UsageReport } from "../protocol";

const REFRESH_MS = 60_000;

export class UsageView implements vscode.WebviewViewProvider, vscode.Disposable {
  static readonly id = "malac.usage";

  private view: vscode.WebviewView | undefined;
  private timer: ReturnType<typeof setInterval> | undefined;
  private pending: ReturnType<typeof setTimeout> | undefined;

  constructor(private readonly client: DaemonClient) {}

  resolveWebviewView(view: vscode.WebviewView): void {
    this.view = view;
    view.webview.options = { enableScripts: false, localResourceRoots: [] };
    view.webview.html = usageHtml(null, { nonce: nonce() });
    view.onDidChangeVisibility(() => {
      if (view.visible) this.refresh();
    });
    view.onDidDispose(() => {
      this.view = undefined;
      clearInterval(this.timer);
      this.timer = undefined;
    });
    this.timer = setInterval(() => {
      if (this.view?.visible) void this.load();
    }, REFRESH_MS);
    void this.load();
  }

  /** Reloads soon; bursts (several turns ending) are coalesced. */
  refresh(delay = 0): void {
    if (!this.view) return;
    clearTimeout(this.pending);
    this.pending = setTimeout(() => void this.load(), delay);
  }

  private async load(): Promise<void> {
    const view = this.view;
    if (!view) return;
    if (!this.client.online) {
      view.webview.html = usageHtml(null, { nonce: nonce(), message: "Not connected to malac." });
      return;
    }
    try {
      const report = await this.client.get<UsageReport>("/api/usage?days=7");
      view.webview.html = usageHtml(usageModel(report), { nonce: nonce() });
    } catch (error) {
      const message =
        error instanceof HttpError && (error.status === 404 || error.status === 405)
          ? "This daemon has no usage endpoint yet. Update malac and restart the daemon."
          : `Usage is unavailable: ${error instanceof Error ? error.message : String(error)}`;
      view.webview.html = usageHtml(null, { nonce: nonce(), message });
    }
  }

  dispose(): void {
    clearInterval(this.timer);
    clearTimeout(this.pending);
  }
}

function nonce(): string {
  return randomBytes(16).toString("base64");
}

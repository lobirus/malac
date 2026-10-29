// The status bar item: agents running, approvals waiting, version notice.

import * as vscode from "vscode";
import type { DaemonClient } from "../connection/client";
import { statusSummary } from "../model/status";
import type { DaemonStore } from "../model/store";
import { ANSWER_COMMAND } from "./approvals";

export class StatusBar implements vscode.Disposable {
  // On the right, like Claude Code's "✻ Claude Code" and Copilot's items.
  private item = vscode.window.createStatusBarItem("malac.status", vscode.StatusBarAlignment.Right, 100);

  constructor(
    private readonly store: DaemonStore,
    private readonly client: DaemonClient,
    private readonly version: () => string | undefined,
  ) {
    this.item.name = "malac";
    this.item.command = "malac.open";
    this.update();
    this.item.show();
  }

  update(): void {
    const summary = statusSummary(this.store, this.client.connection, this.version());
    this.item.text = summary.text;
    // While approvals wait, a click answers them (the warning background says so).
    this.item.command = summary.click === "approvals" ? ANSWER_COMMAND : "malac.open";
    const tooltip = new vscode.MarkdownString(summary.tooltip.map((line) => line.replace(/[\\`*_{}[\]()#+\-.!|]/g, "\\$&")).join("  \n"));
    if (this.store.daemon?.version_mismatch) tooltip.appendMarkdown("\n\n[Restart daemon](command:malac.restartDaemon)");
    tooltip.isTrusted = { enabledCommands: ["malac.restartDaemon"] };
    this.item.tooltip = tooltip;
    this.item.backgroundColor =
      summary.severity === "error"
        ? new vscode.ThemeColor("statusBarItem.errorBackground")
        : summary.severity === "warning"
          ? new vscode.ThemeColor("statusBarItem.warningBackground")
          : undefined;
    this.item.accessibilityInformation = { label: `malac: ${summary.tooltip.join(". ")}` };
  }

  dispose(): void {
    this.item.dispose();
  }
}

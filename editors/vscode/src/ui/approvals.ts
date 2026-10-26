// Approval requests as notifications (Allow / Always / Deny) when the chat
// cannot show them: it is hidden, or it shows another tab.

import * as vscode from "vscode";
import type { DaemonClient } from "../connection/client";
import type { DaemonStore } from "../model/store";
import type { PendingApproval } from "../protocol";

const CHOICES: { label: string; decision: "allow" | "allow_session" | "deny" }[] = [
  { label: "Allow", decision: "allow" },
  { label: "Always", decision: "allow_session" },
  { label: "Deny", decision: "deny" },
];

export class ApprovalNotifier {
  private shown = new Set<string>();

  constructor(
    private readonly store: DaemonStore,
    private readonly client: DaemonClient,
    /** Whether the chat shows this request right now. */
    private readonly chatShows: (request: PendingApproval) => boolean,
    private readonly enabled: () => boolean,
  ) {}

  /** Called with requests that just arrived. */
  notify(requests: PendingApproval[]): void {
    for (const request of requests) {
      if (this.shown.has(request.id) || !this.enabled() || this.chatShows(request)) continue;
      this.shown.add(request.id);
      void this.show(request);
    }
    // Forget requests that are settled.
    for (const id of this.shown) if (!this.store.pending.has(id)) this.shown.delete(id);
  }

  private async show(request: PendingApproval): Promise<void> {
    const tab = this.store.tab(request.tab)?.tab.name;
    const where = tab ? ` in ${tab}` : "";
    const what = request.summary ? `${request.tool}: ${request.summary}` : request.tool;
    const choice = await vscode.window.showWarningMessage(`malac: ${request.agent_label}${where} wants to use ${truncate(what, 200)}`, ...CHOICES.map((c) => c.label));
    const picked = CHOICES.find((c) => c.label === choice);
    if (!picked) return;
    if (!this.store.pending.has(request.id)) return;
    try {
      await this.client.request({ type: "approve", request_id: request.id, decision: picked.decision });
    } catch (error) {
      void vscode.window.showInformationMessage(`malac: ${error instanceof Error ? error.message : String(error)}`);
    }
  }
}

function truncate(text: string, max: number): string {
  return text.length > max ? `${text.slice(0, max - 1)}…` : text;
}

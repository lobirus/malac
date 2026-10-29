// Approval requests outside the chat. While a request waits and no visible
// chat shows it, a notification says which agent wants what. It is a
// progress notification, the only kind an extension can close, so it goes
// away the moment the request is settled anywhere: in a chat, on another
// device, in talk mode. Progress notifications take no buttons; requests are
// answered in the chat, or with "malac: Answer Approvals" (also the status
// bar item while approvals wait): a QuickPick of the waiting requests, then
// Allow / Always / Deny / Show details / Open in chat.

import * as vscode from "vscode";
import type { DaemonClient } from "../connection/client";
import { approvalHeadline, describeApproval } from "../model/approval";
import { plural } from "../model/format";
import type { DaemonStore } from "../model/store";
import type { Decision, PendingApproval } from "../protocol";

export const ANSWER_COMMAND = "malac.answerApprovals";
/** The command's keybinding as package.json declares it. */
const ANSWER_KEYS = process.platform === "darwin" ? "⌥⇧⌘A" : "Ctrl+Alt+Shift+A";

type RequestItem = vscode.QuickPickItem & { request: PendingApproval };
type ActionItem = vscode.QuickPickItem & { action?: Decision | "details" | "open" };

const DECISIONS: { label: string; decision: Decision }[] = [
  { label: "Allow", decision: "allow" },
  { label: "Always", decision: "allow_session" },
  { label: "Deny", decision: "deny" },
];

export class ApprovalNotifier implements vscode.Disposable {
  /** Open notifications by request id, each with what closes it. */
  private shown = new Map<string, () => void>();

  constructor(
    private readonly store: DaemonStore,
    private readonly client: DaemonClient,
    /** Whether a visible chat shows this request right now. */
    private readonly chatShows: (request: PendingApproval) => boolean,
    private readonly enabled: () => boolean,
    /** Shows the request's agent in the chat. */
    private readonly open: (request: PendingApproval) => Promise<void>,
  ) {}

  /** Called with requests that just arrived; closes the notifications of settled ones. */
  notify(requests: PendingApproval[]): void {
    for (const request of requests) {
      if (this.shown.has(request.id) || !this.store.loaded || !this.store.pending.has(request.id) || !this.enabled() || this.chatShows(request)) continue;
      this.show(request);
    }
    this.settle();
  }

  /**
   * Closes the notification of every request that no longer waits (answered
   * anywhere, or gone with its agent), and all of them when notifications
   * were turned off or the daemon connection was lost.
   */
  settle(): void {
    const all = !this.enabled() || !this.store.loaded;
    for (const [id, close] of [...this.shown]) if (all || !this.store.pending.has(id)) close();
  }

  dispose(): void {
    for (const close of [...this.shown.values()]) close();
  }

  private show(request: PendingApproval): void {
    let finish!: () => void;
    const settled = new Promise<void>((resolve) => (finish = resolve));
    this.shown.set(request.id, () => {
      this.shown.delete(request.id);
      finish();
    });
    // VS Code joins a title and a message with a colon; one sentence reads better.
    void vscode.window.withProgress({ location: vscode.ProgressLocation.Notification, title: noticeText(approvalHeadline(request, this.store.tab(request.tab)?.tab.name)), cancellable: false }, () => settled);
  }

  // Answering: "malac: Answer Approvals"

  /** Picks a waiting request (the only one, without asking), then what to do with it. */
  async answer(): Promise<void> {
    if (!this.store.loaded) {
      void vscode.window.showInformationMessage("malac: not connected to the daemon.");
      return;
    }
    if (!this.store.pending.size) {
      void vscode.window.showInformationMessage("malac: no approvals are waiting.");
      return;
    }
    const request = this.store.pending.size === 1 ? [...this.store.pending.values()][0] : await this.pickRequest();
    if (request) await this.pickAction(request);
  }

  private pickRequest(): Promise<PendingApproval | undefined> {
    const items = (): RequestItem[] | null => {
      const requests = [...this.store.pending.values()];
      if (!requests.length) return null;
      return requests.map((request) => {
        const { title } = describeApproval(request.tool, request.summary, request.input);
        const tab = this.store.tab(request.tab)?.tab.name;
        return { label: `$(shield) ${title}`, description: `${request.agent_label}${tab ? ` in ${tab}` : ""}`, request };
      });
    };
    return this.pick<RequestItem>(items, {
      title: () => `malac: ${plural(this.store.pending.size, "approval")} waiting`,
      placeholder: "Pick a request to answer",
      key: (item) => item.request.id,
    }).then((item) => item?.request);
  }

  private async pickAction(request: PendingApproval): Promise<void> {
    const { details } = describeApproval(request.tool, request.summary, request.input);
    const headline = approvalHeadline(request, this.store.tab(request.tab)?.tab.name);
    const items = (): ActionItem[] | null =>
      this.store.pending.has(request.id)
        ? [
            { label: "$(check) Allow", description: "this call", action: "allow" },
            { label: "$(check-all) Always", description: "this and similar calls, for the rest of the session", action: "allow_session" },
            { label: "$(close) Deny", action: "deny" },
            { label: "", kind: vscode.QuickPickItemKind.Separator },
            ...(details ? [{ label: "$(list-unordered) Show details", description: "the full request", action: "details" as const }] : []),
            { label: "$(comment-discussion) Open in chat", description: request.agent_label, action: "open" },
          ]
        : null;
    const action = (await this.pick<ActionItem>(items, { title: () => `malac: ${headline}`, placeholder: "Answer the request", key: (item) => String(item.action) }))?.action;
    if (!action) return;
    if (action === "open") return this.open(request);
    if (action === "details") {
      // A modal dialog shows the whole content (a plugin's hooks, MCP servers and scripts).
      const choice = await vscode.window.showWarningMessage(`malac: ${headline}`, { modal: true, detail: details ?? undefined }, ...DECISIONS.map((d) => d.label));
      const decision = DECISIONS.find((d) => d.label === choice)?.decision;
      if (decision) await this.decide(request, decision);
      return;
    }
    await this.decide(request, action);
  }

  private async decide(request: PendingApproval, decision: Decision): Promise<void> {
    if (!this.store.pending.has(request.id)) {
      void vscode.window.showInformationMessage("malac: that request was already answered.");
      return;
    }
    try {
      await this.client.request({ type: "approve", request_id: request.id, decision });
    } catch (error) {
      void vscode.window.showInformationMessage(`malac: ${error instanceof Error ? error.message : String(error)}`);
    }
  }

  /**
   * A QuickPick that follows the store: its items are rebuilt when requests
   * come and go, and it closes itself when `items` returns null (the request
   * was answered elsewhere, or none are left).
   */
  private pick<T extends vscode.QuickPickItem>(
    items: () => T[] | null,
    options: { title: () => string; placeholder: string; key: (item: T) => string },
  ): Promise<T | undefined> {
    const first = items();
    if (!first) return Promise.resolve(undefined);
    return new Promise((resolve) => {
      const quickPick = vscode.window.createQuickPick<T>();
      quickPick.title = options.title();
      quickPick.placeholder = options.placeholder;
      quickPick.items = first;
      let chosen: T | undefined;
      const subscriptions: { dispose(): void }[] = [
        this.store.onChange.event((change) => {
          if (!change.pending && !change.snapshot) return;
          const next = items();
          if (!next || !this.store.loaded) return quickPick.hide();
          const active = quickPick.activeItems[0];
          quickPick.title = options.title();
          quickPick.items = next;
          const keep = active && next.find((item) => item.kind !== vscode.QuickPickItemKind.Separator && options.key(item) === options.key(active));
          if (keep) quickPick.activeItems = [keep];
        }),
        quickPick.onDidAccept(() => {
          chosen = quickPick.selectedItems[0] ?? quickPick.activeItems[0];
          quickPick.hide();
        }),
        quickPick.onDidHide(() => {
          for (const subscription of subscriptions) subscription.dispose();
          quickPick.dispose();
          resolve(chosen);
        }),
      ];
      quickPick.show();
    });
  }
}

/** The notification's text: who wants what, and where to answer. */
export function noticeText(headline: string): string {
  return `malac: ${headline}${/[.!?]$/.test(headline) ? "" : "."} Answer in the chat or with malac: Answer Approvals (${ANSWER_KEYS}).`;
}

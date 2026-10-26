// TreeDataProviders for Workspaces, Jobs and Sessions. They render the plain
// nodes from model/tree.ts; all the shaping happens there.

import * as vscode from "vscode";
import type { DaemonClient } from "../connection/client";
import type { DaemonStore } from "../model/store";
import { jobNodes, sessionNodes, TreeNode, workspaceTree } from "../model/tree";
import type { SessionSummary, WorkspaceView } from "../protocol";

class NodeProvider implements vscode.TreeDataProvider<TreeNode> {
  protected readonly changed = new vscode.EventEmitter<TreeNode | undefined>();
  readonly onDidChangeTreeData = this.changed.event;
  protected roots: TreeNode[] = [];

  getTreeItem(node: TreeNode): vscode.TreeItem {
    const state =
      node.children === undefined && node.kind !== "workspace"
        ? vscode.TreeItemCollapsibleState.None
        : node.collapsed
          ? vscode.TreeItemCollapsibleState.Collapsed
          : vscode.TreeItemCollapsibleState.Expanded;
    const item = new vscode.TreeItem(node.label, state);
    item.id = node.id;
    item.description = node.description;
    item.tooltip = node.tooltip;
    item.contextValue = node.contextValue;
    if (node.icon) item.iconPath = new vscode.ThemeIcon(node.icon.id, node.icon.color ? new vscode.ThemeColor(node.icon.color) : undefined);
    if (node.command) item.command = { command: node.command.id, title: node.label, arguments: node.command.args };
    item.accessibilityInformation = { label: [node.label, node.description].filter(Boolean).join(", ") };
    return item;
  }

  getChildren(node?: TreeNode): vscode.ProviderResult<TreeNode[]> {
    return node ? (node.children ?? []) : this.roots;
  }

  getParent(): undefined {
    return undefined;
  }

  dispose(): void {
    this.changed.dispose();
  }
}

export class WorkspacesTree extends NodeProvider {
  private loading = new Set<string>();
  chatTab: string | null = null;
  chatAgent: string | null = null;

  constructor(
    private readonly store: DaemonStore,
    private readonly client: DaemonClient,
  ) {
    super();
  }

  refresh(): void {
    this.roots = workspaceTree(this.store, { loading: this.loading, chatTab: this.chatTab, chatAgent: this.chatAgent });
    this.changed.fire(undefined);
  }

  override getChildren(node?: TreeNode): vscode.ProviderResult<TreeNode[]> {
    if (node?.kind === "workspace" && node.ref.workspace && !this.store.views.has(node.ref.workspace)) {
      void this.fetch(node.ref.workspace);
      return [{ id: `info:${node.ref.workspace}:loading`, kind: "info", label: "Loading…", ref: {} }];
    }
    return super.getChildren(node);
  }

  /** Other workspaces' tabs come from REST, which leaves this connection's workspace alone. */
  private async fetch(id: string): Promise<void> {
    if (this.loading.has(id)) return;
    this.loading.add(id);
    try {
      const snapshot = await this.client.get<{ workspace: WorkspaceView | null }>(`/api/snapshot?workspace=${encodeURIComponent(id)}`);
      if (snapshot.workspace) this.store.addView(snapshot.workspace);
    } catch {
      // The node keeps its placeholder; expanding it again retries.
    } finally {
      this.loading.delete(id);
      this.refresh();
    }
  }
}

export class JobsTree extends NodeProvider {
  private timer: ReturnType<typeof setInterval> | undefined;

  constructor(private readonly store: DaemonStore) {
    super();
  }

  refresh(): void {
    this.roots = this.store.loaded ? jobNodes(this.store.jobs.values()) : [];
    this.changed.fire(undefined);
    // Running jobs show their elapsed time.
    const running = [...this.store.jobs.values()].some((j) => j.status === "running");
    if (running && !this.timer) this.timer = setInterval(() => this.refresh(), 5000);
    if (!running && this.timer) {
      clearInterval(this.timer);
      this.timer = undefined;
    }
  }

  override dispose(): void {
    clearInterval(this.timer);
    super.dispose();
  }
}

export class SessionsTree extends NodeProvider {
  /** Only sessions started in the workspace folder, or all of them. */
  scope: "folder" | "all" = "folder";
  private sessions: SessionSummary[] = [];
  private pending: ReturnType<typeof setTimeout> | undefined;

  constructor(
    private readonly store: DaemonStore,
    private readonly client: DaemonClient,
    private readonly folders: () => string[],
  ) {
    super();
  }

  /** Refetches, coalescing bursts (every turn end asks). */
  refresh(delay = 0): void {
    clearTimeout(this.pending);
    this.pending = setTimeout(() => void this.load(), delay);
  }

  private async load(): Promise<void> {
    if (!this.client.online) {
      this.roots = [];
      this.changed.fire(undefined);
      return;
    }
    try {
      const folders = this.scope === "folder" ? this.folders() : [];
      if (folders.length) {
        const lists = await Promise.all(
          folders.map((cwd) => this.client.get<{ sessions: SessionSummary[] }>(`/api/sessions?limit=100&cwd=${encodeURIComponent(cwd)}`)),
        );
        const byId = new Map(lists.flatMap((l) => l.sessions).map((s) => [s.id, s]));
        this.sessions = [...byId.values()].sort((a, b) => String(b.updated).localeCompare(String(a.updated)));
      } else {
        this.sessions = (await this.client.get<{ sessions: SessionSummary[] }>("/api/sessions?limit=150")).sessions;
      }
    } catch {
      // Keep the last list.
    }
    const live = new Set([...this.store.agents.values()].filter((a) => a.live).map((a) => a.id));
    this.roots = sessionNodes(this.sessions, new Date(), live);
    this.changed.fire(undefined);
  }

  override dispose(): void {
    clearTimeout(this.pending);
    super.dispose();
  }
}

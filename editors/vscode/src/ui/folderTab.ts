// The workspace folder's tab: on connecting, show the folder's tab in the
// chat, or offer to open one, suggesting the repository root when the folder
// sits inside a larger repository.

import * as path from "node:path";
import * as vscode from "vscode";
import type { ChatController } from "../chat/controller";
import type { DaemonClient } from "../connection/client";
import { isRemoteEndpoint } from "../model/attachments";
import { findGitRoot, FolderChoice, FolderSetting, largerRoot, planFolder } from "../model/folder";
import type { DaemonStore } from "../model/store";

const CHOICE_KEY = "malac.folderChoice";

export class FolderTabs {
  private checked = false;
  private offered = false;

  constructor(
    private readonly context: vscode.ExtensionContext,
    private readonly store: DaemonStore,
    private readonly client: DaemonClient,
    private readonly chat: ChatController,
  ) {}

  /** The first local workspace folder. */
  folder(): string | null {
    const folder = vscode.workspace.workspaceFolders?.find((f) => f.uri.scheme === "file" || f.uri.scheme === "vscode-remote");
    return folder?.uri.fsPath ?? null;
  }

  root(): string | null {
    const folder = this.folder();
    return folder ? largerRoot(folder, findGitRoot(folder)) : null;
  }

  /** Where `malac web --cwd` should open a tab when the extension starts the daemon. */
  launchDir(): string | undefined {
    const folder = this.folder();
    if (!folder) return undefined;
    const remembered = this.context.workspaceState.get<FolderChoice>(CHOICE_KEY);
    const setting = this.setting();
    if (setting === "never" || remembered === "dismissed") return undefined;
    const root = this.root();
    if (root && (remembered === "root" || setting === "repositoryRoot")) return root;
    return folder;
  }

  private setting(): FolderSetting {
    return vscode.workspace.getConfiguration("malac").get<FolderSetting>("folderTab", "ask");
  }

  /** Runs after each snapshot; acts once per window unless the chat lost its tab. */
  async onSnapshot(): Promise<void> {
    const folder = this.folder();
    // A daemon on another machine has its own folders; this one's paths mean nothing there.
    if (!folder || this.store.daemon?.attached || isRemoteEndpoint(this.client.address)) return;
    const chatHasTab = Boolean(this.chat.currentTab);
    if (this.checked && chatHasTab) return;
    this.checked = true;
    const root = this.root();
    const plan = planFolder({
      folder,
      root,
      tabs: this.store.allTabs().map(({ tab }) => ({ id: tab.id, cwd: tab.cwd })),
      remembered: this.context.workspaceState.get<FolderChoice>(CHOICE_KEY),
      setting: this.setting(),
    });
    switch (plan.kind) {
      case "select":
        this.chat.selectTab(plan.tab);
        this.chat.setBanners("folder", []);
        return;
      case "create":
        await this.openTab(plan.cwd);
        return;
      case "suggestRoot":
        this.chat.selectTab(plan.tab);
        // Once the tab is loaded the daemon reports `suggest_root` itself and
        // the chat shows the hint; ask here only for a tab it has not loaded.
        if (!this.store.tab(plan.tab)?.tab.project) await this.suggestRoot(plan.tab, plan.root, folder);
        return;
      case "offer":
        this.offer(plan.folder, plan.root);
        return;
      case "none":
        return;
    }
  }

  private offer(folder: string, root: string | null): void {
    const name = path.basename(folder);
    const actions = [
      { label: `Open a tab for ${name}`, command: "malac.folderTab.open", args: [folder], primary: !root },
      ...(root ? [{ label: `Use the repository root (${path.basename(root)})`, command: "malac.folderTab.open", args: [root, "root"], primary: true }] : []),
      { label: "Not now", command: "malac.folderTab.dismiss" },
    ];
    const text = root
      ? `${name} is inside the repository ${path.basename(root)}. Work on the full stack from the repository root, or just this folder?`
      : `No malac tab for ${name} yet.`;
    this.chat.setBanners("folder", [{ id: "folder", level: "info", text, actions }]);
    if (this.offered) return;
    this.offered = true;
    const buttons = root ? ["Use repository root", `Just ${name}`, "Not now"] : [`Open tab for ${name}`, "Not now"];
    void vscode.window.showInformationMessage(`malac: ${text}`, ...buttons).then((choice) => {
      if (!choice) return;
      if (choice === "Not now") void this.dismiss();
      else if (choice === "Use repository root" && root) void this.openTab(root, "root");
      else void this.openTab(folder, "folder");
    });
  }

  private async suggestRoot(tab: string, root: string, folder: string): Promise<void> {
    const name = path.basename(folder);
    const choice = await vscode.window.showInformationMessage(
      `malac: ${name} is part of the repository ${path.basename(root)}. Work on the full stack from the repository root?`,
      "Use repository root",
      `Keep ${name}`,
    );
    if (choice === "Use repository root") {
      const view = this.store.tab(tab)?.tab;
      await this.openTab(root, "root");
      // The folder tab was opened for us and holds nothing yet.
      if (view && view.agents.length === 0) this.client.send({ type: "tab_close", tab });
    } else if (choice) {
      await this.context.workspaceState.update(CHOICE_KEY, "folder");
    }
  }

  async dismiss(): Promise<void> {
    await this.context.workspaceState.update(CHOICE_KEY, "dismissed");
    this.chat.setBanners("folder", []);
  }

  /** Opens (or reuses) a tab for `cwd` and shows it in the chat. */
  async openTab(cwd: string, remember?: FolderChoice): Promise<string | null> {
    this.client.ensureStarted();
    if (remember) await this.context.workspaceState.update(CHOICE_KEY, remember);
    try {
      const reply = await this.client.request({ type: "tab_create", cwd, reuse: true });
      const tab = reply.tab as string;
      this.chat.setBanners("folder", []);
      this.chat.selectTab(tab);
      return tab;
    } catch (error) {
      void vscode.window.showErrorMessage(`malac: cannot open a tab for ${cwd}: ${error instanceof Error ? error.message : String(error)}`);
      return null;
    }
  }

  /** "New Tab for This Folder": the folder, the repository root, or another folder. */
  async pickAndOpen(): Promise<void> {
    const folder = this.folder();
    const root = this.root();
    type Item = vscode.QuickPickItem & { dir?: string; choice?: FolderChoice; browse?: boolean };
    const items: Item[] = [];
    if (root) items.push({ label: `$(repo) ${path.basename(root)}`, description: "repository root, the full stack", detail: root, dir: root, choice: "root" });
    if (folder) items.push({ label: `$(folder) ${path.basename(folder)}`, description: "this folder", detail: folder, dir: folder, choice: "folder" });
    for (const other of vscode.workspace.workspaceFolders?.slice(1) ?? []) {
      items.push({ label: `$(folder) ${other.name}`, description: "workspace folder", detail: other.uri.fsPath, dir: other.uri.fsPath });
    }
    items.push({ label: "$(folder-opened) Choose a folder…", browse: true });
    const picked = items.length === 1 ? items[0] : await vscode.window.showQuickPick(items, { title: "New malac tab", placeHolder: "Where should the agents work?" });
    if (!picked) return;
    let dir = picked.dir;
    if (picked.browse) {
      const chosen = await vscode.window.showOpenDialog({ canSelectFolders: true, canSelectFiles: false, canSelectMany: false, openLabel: "Open tab here" });
      dir = chosen?.[0]?.fsPath;
    }
    if (dir) await this.openTab(dir, picked.choice);
  }
}

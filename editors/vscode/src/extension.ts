// The malac extension: one daemon connection per window, shared by the chat
// view, the Workspaces/Jobs/Sessions trees, the Usage view, the status bar
// and approval notifications. "Open full UI" still frames the web UI.

import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import * as vscode from "vscode";
import { ChatController } from "./chat/controller";
import { ChatView } from "./chat/view";
import { DaemonClient } from "./connection/client";
import { EndpointProvider, LocalEndpoints, malacHome, parseConnectionSetting, SavedEndpoints } from "./connection/endpoint";
import { Launcher, MissingBinaryError, parseVersion, runMalac } from "./connection/launcher";
import { isRemoteEndpoint } from "./model/attachments";
import { Change, DaemonStore } from "./model/store";
import type { NodeRef } from "./model/tree";
import type { WorkspaceView } from "./protocol";
import { ApprovalNotifier } from "./ui/approvals";
import { FolderTabs } from "./ui/folderTab";
import { MalacPanel } from "./ui/fullUi";
import { JOB_SCHEME, JobLogProvider, jobUri } from "./ui/jobLog";
import { StatusBar } from "./ui/statusBar";
import { JobsTree, SessionsTree, WorkspacesTree } from "./ui/trees";
import { UsageView } from "./ui/usageView";

export function activate(context: vscode.ExtensionContext): void {
  const output = vscode.window.createOutputChannel("malac", { log: true });
  const config = () => vscode.workspace.getConfiguration("malac");
  const home = () => malacHome(config().get<string>("home"));
  const command = () => resolveCommand(config().get<string>("path", "malac"));
  const extensionVersion = String(context.extension.packageJSON.version ?? "");
  let cliVersion: string | undefined;
  const version = () => cliVersion ?? extensionVersion;
  const log = (line: string) => output.appendLine(line);

  const store = new DaemonStore();
  let folders: FolderTabs | undefined;
  const launcher = new Launcher(
    () => ({
      command: command(),
      extraArgs: config().get<string[]>("extraArgs", []),
      cwd: folders?.launchDir(),
      home: config().get<string>("home")?.trim() || undefined,
    }),
    { appendLine: (line) => output.appendLine(line), append: (text) => output.append(text) },
  );
  const local = new LocalEndpoints(home, launcher);
  const provider = (): EndpointProvider => {
    const setting = parseConnectionSetting(config().get("connection"));
    return setting.kind === "local" ? local : new SavedEndpoints(setting.name, home);
  };
  const client = new DaemonClient({ provider, version, log });

  const chatView = new ChatView(context, output, version);
  const chat = new ChatController(store, client, chatView, { includeCurrentFile: config().get<boolean>("chat.includeCurrentFile", true) });
  chatView.controller = chat;
  folders = new FolderTabs(context, store, client, chat);

  const workspacesTree = new WorkspacesTree(store, client);
  const jobsTree = new JobsTree(store);
  const sessionsTree = new SessionsTree(store, client, () => (vscode.workspace.workspaceFolders ?? []).map((f) => f.uri.fsPath));
  const workspacesView = vscode.window.createTreeView("malac.workspaces", { treeDataProvider: workspacesTree, showCollapseAll: true });
  const jobsView = vscode.window.createTreeView("malac.jobs", { treeDataProvider: jobsTree });
  const sessionsView = vscode.window.createTreeView("malac.sessions", { treeDataProvider: sessionsTree, showCollapseAll: true });
  const usageView = new UsageView(client);
  const statusBar = new StatusBar(store, client, version);
  const jobLogs = new JobLogProvider(client, store);
  let panel: MalacPanel | undefined;

  const chatShows = (request: { tab: string | null; agent: string }) => {
    if (!chatView.visible) return false;
    const tab = chat.currentTab;
    if (!tab) return false;
    if (request.tab === tab) return true;
    const view = store.tab(tab)?.tab;
    return Boolean(view && store.agentsOfTab(view).some((a) => a.id === request.agent));
  };
  const notifier = new ApprovalNotifier(store, client, chatShows, () => config().get<boolean>("approvalNotifications", true));

  // Store → views

  let treeTimer: ReturnType<typeof setTimeout> | undefined;
  const refreshTrees = () => {
    treeTimer ??= setTimeout(() => {
      treeTimer = undefined;
      workspacesTree.refresh();
      jobsTree.refresh();
      const pending = store.pending.size;
      workspacesView.badge = pending ? { value: pending, tooltip: `${pending} approval${pending === 1 ? "" : "s"} waiting` } : undefined;
    }, 120);
  };

  const onChange = (change: Change) => {
    if (change.snapshot) {
      sessionsTree.refresh();
      usageView.refresh();
      void folders?.onSnapshot();
    }
    if (change.tabs || change.agents || change.pending || change.jobs || change.workspaces || change.snapshot) refreshTrees();
    if (change.requested) notifier.notify(change.requested);
    if (change.pending) notifier.notify([]);
    if (change.finished) {
      sessionsTree.refresh(1500);
      usageView.refresh(3000);
    }
    if (change.jobs) for (const job of store.jobs.values()) if (job.status !== "running") jobLogs.refresh(job.id);
    if (change.notice && change.notice.level !== "info") {
      const show = change.notice.level === "error" ? vscode.window.showErrorMessage : vscode.window.showWarningMessage;
      void show(`malac: ${change.notice.message}`);
    }
    statusBar.update();
  };

  context.subscriptions.push(
    output,
    { dispose: () => client.dispose() },
    { dispose: () => launcher.dispose() },
    { dispose: () => panel?.dispose() },
    chatView,
    chat,
    workspacesTree,
    jobsTree,
    sessionsTree,
    workspacesView,
    jobsView,
    sessionsView,
    usageView,
    statusBar,
    jobLogs,
    vscode.window.registerWebviewViewProvider(ChatView.id, chatView, { webviewOptions: { retainContextWhenHidden: true } }),
    vscode.window.registerWebviewViewProvider(UsageView.id, usageView),
    vscode.workspace.registerTextDocumentContentProvider(JOB_SCHEME, jobLogs),
    store.onChange.event(onChange),
    client.onMessage.event((message) => store.apply(message)),
    client.onStatus.event((status) => {
      log(`connection: ${status.state}${status.detail ? ` (${status.detail})` : ""}`);
      if (status.state !== "online" && store.loaded) store.reset();
      const message =
        status.state === "online"
          ? undefined
          : status.state === "stopped"
            ? "The malac daemon is not running. Open the chat to start it."
            : status.state === "failed"
              ? status.detail
              : "Connecting to malac…";
      workspacesView.message = message;
      jobsView.message = message;
      sessionsView.message = message;
      statusBar.update();
      refreshTrees();
      if (status.state === "online") sessionsTree.refresh();
      usageView.refresh();
    }),
    chat.onSelection.event(({ tab, agent }) => {
      workspacesTree.chatTab = tab;
      workspacesTree.chatAgent = agent;
      refreshTrees();
    }),
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (["malac.connection", "malac.home", "malac.path"].some((key) => event.affectsConfiguration(key))) client.reconnect({ allowStart: true });
      if (event.affectsConfiguration("malac.folderTab")) void folders?.onSnapshot();
    }),
  );

  // Commands

  const reveal = async (text?: string) => {
    client.ensureStarted();
    await chatView.reveal(text);
  };

  const whenOnline = (timeoutMs = 30_000) =>
    new Promise<void>((resolve, reject) => {
      if (client.online && store.loaded) return resolve();
      const timer = setTimeout(() => {
        sub.dispose();
        reject(new Error(client.connection.detail || "malac is not connected"));
      }, timeoutMs);
      const sub = store.onChange.event((change) => {
        if (change.snapshot && client.online) {
          clearTimeout(timer);
          sub.dispose();
          resolve();
        }
      });
      client.ensureStarted();
    });

  /** Makes sure a tab (of any workspace) is in the store, fetching its workspace if needed. */
  const knowTab = async (tab: string, workspace?: string) => {
    if (store.tab(tab)) return true;
    const candidates = workspace ? [workspace] : [...store.workspaces.keys()].filter((id) => !store.views.has(id));
    for (const id of candidates) {
      try {
        const snapshot = await client.get<{ workspace: WorkspaceView | null }>(`/api/snapshot?workspace=${encodeURIComponent(id)}`);
        if (snapshot.workspace) store.addView(snapshot.workspace);
      } catch {
        // Try the next one.
      }
      if (store.tab(tab)) return true;
    }
    return false;
  };

  const register = (id: string, fn: (...args: any[]) => unknown) =>
    context.subscriptions.push(
      vscode.commands.registerCommand(id, async (...args: unknown[]) => {
        try {
          await fn(...args);
        } catch (error) {
          void vscode.window.showErrorMessage(`malac: ${error instanceof Error ? error.message : String(error)}`);
        }
      }),
    );

  register("malac.focusChat", () => reveal());

  register("malac.askAboutSelection", async () => {
    chat.setContextEnabled(true);
    await reveal();
  });

  register("malac.addFileToContext", async (uri?: vscode.Uri, uris?: vscode.Uri[]) => {
    const picked = uris?.length ? uris : uri instanceof vscode.Uri ? [uri] : [];
    const targets = picked.length ? picked : vscode.window.activeTextEditor ? [vscode.window.activeTextEditor.document.uri] : [];
    const files = targets.filter((u) => u.scheme === "file" || u.scheme === "vscode-remote");
    if (!files.length) {
      void vscode.window.showInformationMessage("malac: pick a file in the Explorer or open one in an editor first.");
      return;
    }
    await reveal();
    chat.addAttachments(files.map((u) => ({ path: u.fsPath, label: vscode.workspace.asRelativePath(u, false) })));
  });

  register("malac.newAgent", async () => {
    await reveal();
    chat.newAgent();
  });

  register("malac.newTabForFolder", async () => {
    client.ensureStarted();
    await whenOnline();
    await folders!.pickAndOpen();
    await reveal();
  });

  register("malac.folderTab.open", async (dir: string, remember?: "folder" | "root") => {
    await whenOnline();
    await folders!.openTab(dir, remember);
  });

  register("malac.folderTab.dismiss", () => folders!.dismiss());

  register("malac.stop", async () => {
    if (chat.stop()) return;
    const running = store.runningAgents();
    if (!running.length) {
      void vscode.window.showInformationMessage("malac: no agent is running.");
      return;
    }
    const picked =
      running.length === 1
        ? running[0]
        : (
            await vscode.window.showQuickPick(
              running.map((a) => ({ label: a.title || a.label, description: a.activity, agent: a })),
              { title: "Stop which agent?" },
            )
          )?.agent;
    if (picked) client.send({ type: "cancel", agent: picked.id });
  });

  const openFullUi = async () => {
    await whenOnline();
    const endpoint = client.address;
    if (!endpoint) throw new Error("malac is not connected");
    panel ??= new MalacPanel(() => (panel = undefined));
    // A daemon elsewhere pairs the browser on its own; its device token stays out of URLs.
    await panel.show({ url: endpoint.url, token: endpoint.source === "remote" ? "" : endpoint.token });
  };
  register("malac.openFullUi", openFullUi);
  register("malac.open", openFullUi);

  register("malac.restartDaemon", async () => {
    if (isRemoteEndpoint(client.address)) {
      void vscode.window.showInformationMessage(`malac: the daemon at ${client.address?.url} runs on another machine; restart it there with \`malac daemon restart\`.`);
      return;
    }
    let force = false;
    if (client.online) {
      const info = await client.get<{ agents_running: number; jobs_running: number }>("/api/daemon").catch(() => null);
      if (info && (info.agents_running || info.jobs_running)) {
        const choice = await vscode.window.showWarningMessage(
          `${info.agents_running} agents and ${info.jobs_running} jobs are running. Restarting the daemon stops them.`,
          { modal: true },
          "Restart anyway",
        );
        if (!choice) return;
        force = true;
      }
    }
    await vscode.window.withProgress({ location: vscode.ProgressLocation.Notification, title: "Restarting the malac daemon" }, async () => {
      const text = await runMalac(command(), ["daemon", "restart", ...(force ? ["--force"] : [])], config().get<string>("home")?.trim() || undefined, 90_000);
      log(text.trim());
    });
    launcher.dispose();
    client.reconnect({ allowStart: true });
    cliVersion = await detectVersion(command(), log);
  });

  register("malac.reconnect", () => client.reconnect({ allowStart: true }));
  register("malac.showLog", () => output.show());

  register("malac.runInTerminal", () => {
    const terminal = vscode.window.createTerminal({
      name: "malac",
      cwd: chat.currentTab ? store.tab(chat.currentTab)?.tab.cwd : vscode.workspace.workspaceFolders?.[0]?.uri.fsPath,
      shellPath: command(),
    });
    terminal.show();
  });

  // Tree item commands. Inline actions pass the TreeNode, clicks pass its ref.
  const refOf = (arg: unknown): NodeRef => ((arg as { ref?: NodeRef })?.ref ?? (arg as NodeRef) ?? {}) as NodeRef;

  register("malac.agent.open", async (arg: unknown) => {
    const ref = refOf(arg);
    if (!ref.agent) return;
    if (ref.tab) await knowTab(ref.tab);
    chat.selectAgent(ref.agent);
    await reveal();
  });
  register("malac.agent.stop", (arg: unknown) => {
    const ref = refOf(arg);
    if (ref.agent) client.send({ type: "cancel", agent: ref.agent });
  });
  register("malac.agent.close", async (arg: unknown) => {
    const ref = refOf(arg);
    const tab = ref.tab ?? (ref.agent ? store.tabOfAgent(ref.agent)?.id : undefined);
    if (ref.agent && tab) await client.request({ type: "agent_close", tab, agent: ref.agent });
  });
  register("malac.tab.open", async (arg: unknown) => {
    const ref = refOf(arg);
    if (!ref.tab) return;
    await knowTab(ref.tab, ref.workspace);
    chat.selectTab(ref.tab);
    await reveal();
  });
  register("malac.tab.newAgent", async (arg: unknown) => {
    const ref = refOf(arg);
    if (!ref.tab) return;
    await knowTab(ref.tab, ref.workspace);
    chat.newAgent(ref.tab);
    await reveal();
  });
  register("malac.tab.close", async (arg: unknown) => {
    const ref = refOf(arg);
    if (!ref.tab) return;
    const view = store.tab(ref.tab)?.tab;
    const running = view ? store.agentsOfTab(view).filter((a) => a.live && a.status === "running").length : 0;
    let force = false;
    if (running) {
      const choice = await vscode.window.showWarningMessage(`${running} agents are running in ${view?.name}. Closing the tab stops them.`, { modal: true }, "Close anyway");
      if (!choice) return;
      force = true;
    }
    await client.request({ type: "tab_close", tab: ref.tab, force });
  });
  register("malac.workspace.open", async (arg: unknown) => {
    const ref = refOf(arg);
    if (!ref.workspace) return;
    const reply = await client.request({ type: "workspace_open", workspace: ref.workspace });
    const view = reply.workspace as WorkspaceView;
    const tab = view.active_tab ?? view.tabs[0]?.id;
    if (tab) chat.selectTab(tab);
  });
  register("malac.job.log", async (arg: unknown) => {
    const ref = refOf(arg);
    const job = ref.job ? store.jobs.get(ref.job) : undefined;
    if (!job) return;
    const document = await vscode.workspace.openTextDocument(jobUri(job.id, job.command));
    await vscode.languages.setTextDocumentLanguage(document, "log");
    await vscode.window.showTextDocument(document, { preview: true });
  });
  register("malac.job.kill", async (arg: unknown) => {
    const ref = refOf(arg);
    if (ref.job) await client.request({ type: "job_kill", job: ref.job });
  });
  register("malac.session.open", async (arg: unknown) => {
    const ref = refOf(arg);
    if (!ref.session) return;
    await whenOnline();
    const reply = await client.request({ type: "agent_open", agent: ref.session });
    const tab = reply.tab as string;
    await knowTab(tab, reply.workspace as string | undefined);
    chat.selectTab(tab, (reply.agent as string) ?? ref.session);
    await reveal();
  });

  const setSessionScope = (scope: "folder" | "all") => {
    sessionsTree.scope = scope;
    void vscode.commands.executeCommand("setContext", "malac.sessionsScope", scope);
    sessionsView.description = scope === "folder" ? "this folder" : "all folders";
    sessionsTree.refresh();
  };
  register("malac.sessions.showAll", () => setSessionScope("all"));
  register("malac.sessions.showFolder", () => setSessionScope("folder"));
  register("malac.sessions.refresh", () => sessionsTree.refresh());
  register("malac.workspaces.refresh", () => client.send({ type: "snapshot" }));
  register("malac.jobs.refresh", () => client.send({ type: "job_list" }));
  register("malac.usage.refresh", () => usageView.refresh());
  setSessionScope("folder");

  // Views opening means the user wants malac: start the daemon if needed.
  for (const view of [workspacesView, jobsView, sessionsView]) {
    context.subscriptions.push(view.onDidChangeVisibility((event) => event.visible && client.ensureStarted()));
  }

  void (async () => {
    cliVersion = await detectVersion(command(), log);
    const visible = workspacesView.visible || jobsView.visible || sessionsView.visible;
    client.start({ autoStart: visible || config().get<boolean>("startDaemonOnStartup", false) });
    // The chat view resolving later also starts it.
  })();
  chatView.onResolve = () => client.ensureStarted();
}

export function deactivate(): void {}

/** `malac` from PATH, else the usual install locations (GUI apps can miss shell PATH entries). */
function resolveCommand(setting: string): string {
  const value = setting.trim() || "malac";
  if (value !== "malac") return value.startsWith("~/") ? path.join(os.homedir(), value.slice(2)) : value;
  const onPath = (process.env.PATH ?? "").split(path.delimiter).some((dir) => dir && fs.existsSync(path.join(dir, "malac")));
  if (onPath) return value;
  const candidates = [
    path.join(os.homedir(), ".local", "bin", "malac"),
    path.join(os.homedir(), ".cargo", "bin", "malac"),
    "/opt/homebrew/bin/malac",
    "/usr/local/bin/malac",
  ];
  return candidates.find((candidate) => fs.existsSync(candidate)) ?? value;
}

async function detectVersion(command: string, log: (line: string) => void): Promise<string | undefined> {
  try {
    return parseVersion(await runMalac(command, ["--version"], undefined, 5000)) ?? undefined;
  } catch (error) {
    log(error instanceof MissingBinaryError ? error.message : `cannot read the malac version: ${String(error)}`);
    return undefined;
  }
}

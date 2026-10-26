// The tree views as plain data: Workspaces → Tabs → Agents, Jobs and
// Sessions. The vscode providers only map these nodes to TreeItems, so the
// shape, labels, icons and context values are tested without VS Code.

import type { JobView, SessionSummary, TabView, WorkspaceSummary } from "../protocol";
import { baseName, formatDuration, relativeTime, tildePath, truncate } from "./format";
import { AgentState, DaemonStore, DisplayStatus, displayName } from "./store";

export interface TreeIcon {
  /** A codicon id, e.g. `sync~spin`. */
  id: string;
  /** A theme color id, e.g. `charts.green`. */
  color?: string;
}

export interface TreeNode {
  id: string;
  kind: "workspace" | "tab" | "agent" | "job" | "session" | "group" | "info";
  label: string;
  description?: string;
  tooltip?: string;
  icon?: TreeIcon;
  /** Matched by `viewItem` in package.json menus. */
  contextValue?: string;
  collapsed?: boolean;
  children?: TreeNode[];
  /** The command run on click: its id and arguments. */
  command?: { id: string; args: unknown[] };
  /** What the node stands for, passed to context-menu commands. */
  ref: NodeRef;
}

export interface NodeRef {
  workspace?: string;
  tab?: string;
  agent?: string;
  job?: string;
  session?: string;
  cwd?: string;
}

export const STATUS_ICONS: Record<DisplayStatus, TreeIcon> = {
  running: { id: "sync~spin", color: "charts.green" },
  waiting: { id: "bell-dot", color: "charts.yellow" },
  completed: { id: "pass", color: "testing.iconPassed" },
  failed: { id: "error", color: "errorForeground" },
  cancelled: { id: "circle-slash" },
  idle: { id: "circle-outline" },
  saved: { id: "history" },
};

export const STATUS_TEXT: Record<DisplayStatus, string> = {
  running: "Running",
  waiting: "Awaiting approval",
  completed: "Done",
  failed: "Failed",
  cancelled: "Stopped",
  idle: "Idle",
  saved: "Saved",
};

export interface WorkspaceTreeOptions {
  /** Workspaces whose tabs are being fetched. */
  loading?: Set<string>;
  /** The chat's tab and agent, marked in the tree. */
  chatTab?: string | null;
  chatAgent?: string | null;
}

export function workspaceTree(store: DaemonStore, options: WorkspaceTreeOptions = {}): TreeNode[] {
  if (!store.loaded) return [];
  const summaries = [...store.workspaces.values()].sort(
    (a, b) => Number(b.id === store.current) - Number(a.id === store.current) || String(b.updated).localeCompare(String(a.updated)),
  );
  if (!summaries.length) {
    return [{ id: "info:empty", kind: "info", label: "No workspaces yet", description: "open a tab to start", ref: {} }];
  }
  return summaries.map((summary) => workspaceNode(store, summary, options));
}

function workspaceNode(store: DaemonStore, summary: WorkspaceSummary, options: WorkspaceTreeOptions): TreeNode {
  const view = store.views.get(summary.id);
  const current = summary.id === store.current;
  const parts = [`${summary.tabs} ${summary.tabs === 1 ? "tab" : "tabs"}`];
  if (summary.running) parts.push(`${summary.running} running`);
  if (current) parts.unshift("current");
  let children: TreeNode[];
  if (view) {
    children = view.tabs.map((tab) => tabNode(store, summary.id, tab, view.active_tab === tab.id, options));
    if (!children.length) children = [{ id: `info:${summary.id}:none`, kind: "info", label: "No tabs", ref: { workspace: summary.id } }];
  } else {
    children = options.loading?.has(summary.id)
      ? [{ id: `info:${summary.id}:loading`, kind: "info", label: "Loading…", ref: { workspace: summary.id } }]
      : [];
  }
  return {
    id: `workspace:${summary.id}`,
    kind: "workspace",
    label: summary.name,
    description: parts.join(" · "),
    tooltip: `Workspace ${summary.name}\n${parts.join(" · ")}\nUpdated ${relativeTime(summary.updated)}`,
    icon: { id: current ? "layers-active" : "layers" },
    contextValue: current ? "workspace.current" : "workspace",
    collapsed: !current,
    children: view ? children : children.length ? children : undefined,
    ref: { workspace: summary.id },
  };
}

function tabNode(store: DaemonStore, workspace: string, tab: TabView, active: boolean, options: WorkspaceTreeOptions): TreeNode {
  const status = store.tabStatus(tab);
  const top = tab.agents.map((id) => store.agents.get(id)).filter((a): a is AgentState => Boolean(a));
  const home = store.daemon?.user_home ?? null;
  const description = [tildePath(tab.cwd, home), tab.mode !== "ask" ? tab.mode : null].filter(Boolean).join(" · ");
  const inChat = options.chatTab === tab.id;
  return {
    id: `tab:${tab.id}`,
    kind: "tab",
    label: tab.name,
    description,
    tooltip: [`Tab ${tab.name}${inChat ? " (in the chat)" : ""}`, tab.cwd, `Mode: ${tab.mode}`, `${STATUS_TEXT[status]}`, active ? "Active tab of its workspace" : null]
      .filter(Boolean)
      .join("\n"),
    icon: status === "idle" || status === "saved" ? { id: inChat ? "folder-opened" : "folder" } : STATUS_ICONS[status],
    contextValue: "tab",
    collapsed: false,
    children: top.length
      ? top.map((agent) => agentNode(store, agent, options))
      : [{ id: `info:${tab.id}:empty`, kind: "info", label: "No agents", description: "send a prompt to start one", ref: { tab: tab.id } }],
    command: { id: "malac.tab.open", args: [{ tab: tab.id, workspace }] },
    ref: { workspace, tab: tab.id, cwd: tab.cwd },
  };
}

export function agentNode(store: DaemonStore, agent: AgentState, options: WorkspaceTreeOptions = {}, depth = 0): TreeNode {
  const status = store.agentStatus(agent);
  const children = depth < 8 ? store.childrenOf(agent.id).map((child) => agentNode(store, child, options, depth + 1)) : [];
  const running = status === "running" || status === "waiting";
  const description = [running && agent.activity ? truncate(agent.activity, 48) : STATUS_TEXT[status], agent.model ? baseName(agent.model) : null]
    .filter(Boolean)
    .join(" · ");
  const inChat = options.chatAgent === agent.id;
  return {
    id: `agent:${agent.id}`,
    kind: "agent",
    label: displayName(agent) || "agent",
    description,
    tooltip: [
      `${agent.parent ? "Subagent" : "Agent"} ${agent.label}${inChat ? " (in the chat)" : ""}`,
      agent.title && agent.title !== agent.label ? agent.title : null,
      `${STATUS_TEXT[status]}${agent.activity && running ? `: ${agent.activity}` : ""}`,
      agent.model ? `Model: ${agent.model}` : null,
      agent.live ? null : "Saved session; prompting it resumes it",
    ]
      .filter(Boolean)
      .join("\n"),
    icon: STATUS_ICONS[status],
    contextValue: `agent.${running ? "running" : "idle"}${agent.parent ? ".sub" : ""}`,
    collapsed: !running,
    children: children.length ? children : undefined,
    command: { id: "malac.agent.open", args: [{ agent: agent.id, tab: agent.tab }] },
    ref: { agent: agent.id, tab: agent.tab ?? undefined },
  };
}

export function jobNodes(jobs: Iterable<JobView>, now = Date.now()): TreeNode[] {
  const list = [...jobs].sort((a, b) => Number(b.status === "running") - Number(a.status === "running") || String(b.started).localeCompare(String(a.started)));
  if (!list.length) return [{ id: "info:jobs", kind: "info", label: "No background jobs", ref: {} }];
  return list.map((job) => {
    const running = job.status === "running";
    const ended = job.finished ? Date.parse(job.finished) : now;
    const elapsed = formatDuration(ended - Date.parse(job.started));
    const state = running ? `running ${elapsed}` : job.status === "exited" ? `exited ${job.exit_code ?? "?"}` : job.status;
    const icon: TreeIcon = running
      ? { id: "sync~spin", color: "charts.green" }
      : job.status === "killed"
        ? { id: "circle-slash" }
        : job.status === "exited" && job.exit_code === 0
          ? { id: "pass", color: "testing.iconPassed" }
          : job.status === "exited"
            ? { id: "error", color: "errorForeground" }
            : { id: "question" };
    return {
      id: `job:${job.id}`,
      kind: "job" as const,
      label: truncate(job.command.split("\n")[0], 80),
      description: `${state} · ${job.id}`,
      tooltip: [job.command, job.cwd, `Job ${job.id}, ${state}`, job.pid ? `pid ${job.pid}` : null].filter(Boolean).join("\n"),
      icon,
      contextValue: running ? "job.running" : "job",
      command: { id: "malac.job.log", args: [{ job: job.id }] },
      ref: { job: job.id, tab: job.tab ?? undefined },
    };
  });
}

/** Sessions grouped by day: Today, Yesterday, This week, Older. */
export function sessionNodes(sessions: SessionSummary[], now = new Date(), live: Set<string> = new Set()): TreeNode[] {
  if (!sessions.length) return [{ id: "info:sessions", kind: "info", label: "No saved sessions", ref: {} }];
  const groups = new Map<string, TreeNode[]>();
  for (const session of sessions) {
    const group = dayGroup(session.updated, now);
    const node: TreeNode = {
      id: `session:${session.id}`,
      kind: "session",
      label: session.title || session.label || session.id.slice(0, 8),
      description: [relativeTime(session.updated, now.getTime()), baseName(session.cwd)].filter(Boolean).join(" · "),
      tooltip: [session.title, session.cwd, `Model: ${session.model}`, `Started ${session.created}`, live.has(session.id) ? "Open in a tab" : null]
        .filter(Boolean)
        .join("\n"),
      icon: { id: live.has(session.id) ? "comment-discussion" : "comment" },
      contextValue: "session",
      command: { id: "malac.session.open", args: [{ session: session.id }] },
      ref: { session: session.id, cwd: session.cwd },
    };
    if (!groups.has(group)) groups.set(group, []);
    groups.get(group)!.push(node);
  }
  return [...groups].map(([label, children], index) => ({
    id: `group:${label}`,
    kind: "group" as const,
    label,
    description: String(children.length),
    collapsed: index > 1,
    children,
    ref: {},
  }));
}

export function dayGroup(iso: string, now: Date): string {
  const date = new Date(iso);
  const start = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  const day = 86_400_000;
  const time = date.getTime();
  if (time >= start) return "Today";
  if (time >= start - day) return "Yesterday";
  if (time >= start - 6 * day) return "This week";
  return "Older";
}

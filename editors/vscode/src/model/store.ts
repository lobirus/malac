// Client state kept current from the daemon: the snapshot first, then
// broadcasts (deltas and agent events). Shared by the chat, the trees, the
// status bar and the notifications. Free of the vscode module.

import {
  AgentStatus,
  AgentView,
  DaemonStatus,
  emptyUsage,
  Envelope,
  JobView,
  Mode,
  PendingApproval,
  SayMessage,
  ServerMessage,
  Snapshot,
  TabView,
  Usage,
  VoiceStatus,
  WorkspaceSummary,
  WorkspaceView,
} from "../protocol";
import { Emitter } from "./emitter";

/** Agent events kept per agent for the chat, after merging deltas. */
const EVENT_LIMIT = 4000;

export interface AgentState {
  id: string;
  label: string;
  title: string;
  parent: string | null;
  tab: string | null;
  model: string;
  cwd: string;
  created: string | null;
  status: AgentStatus;
  activity: string;
  usage: Usage;
  contextTokens: number;
  report: string | null;
  readOnly: boolean;
  live: boolean;
  /** Events since this connection's snapshot, each numbered by arrival. */
  events: { n: number; env: Envelope }[];
  /** Whether old events were dropped from `events`. */
  trimmed: boolean;
  running: Map<string, string>;
  turnStarted: string | null;
}

export interface Change {
  /** A snapshot replaced everything. */
  snapshot?: boolean;
  workspaces?: boolean;
  tabs?: boolean;
  agents?: Set<string>;
  pending?: boolean;
  jobs?: boolean;
  /** New events, by agent. */
  events?: Map<string, Envelope[]>;
  /** Approvals that just arrived. */
  requested?: PendingApproval[];
  /** A notice to show. */
  notice?: { level: string; message: string; tab: string | null };
  shutdown?: string;
  /** The tab and agent of a closed tab or agent, so views can move away. */
  closedTab?: string;
  mode?: boolean;
  finished?: { agent: string; status: string }[];
  /** The voice settings or the models that can do them changed. */
  voice?: boolean;
  /** The configured models or the default model changed (`models` broadcast). */
  models?: boolean;
  /** A sentence to read aloud (for whichever connection's listener it names). */
  say?: SayMessage;
  /** `say_cancelled`: sentences to stop and drop. */
  sayCancelled?: string[];
}

export type DisplayStatus = "waiting" | "running" | "failed" | "completed" | "cancelled" | "idle" | "saved";

export class DaemonStore {
  daemon: DaemonStatus | null = null;
  workspaces = new Map<string, WorkspaceSummary>();
  /** Workspaces whose tabs are known: the connection's plus any fetched. */
  views = new Map<string, WorkspaceView>();
  /** The connection's workspace. */
  current: string | null = null;
  agents = new Map<string, AgentState>();
  pending = new Map<string, PendingApproval>();
  jobs = new Map<string, JobView>();
  models: string[] = [];
  defaultModel: string | null = null;
  /** Null until a daemon with voice support reports it. */
  voice: VoiceStatus | null = null;
  loaded = false;
  /** Bumped by every snapshot; transcripts loaded before it are stale. */
  generation = 0;
  /** Counts events as they arrive. */
  received = 0;

  readonly onChange = new Emitter<Change>();

  apply(message: ServerMessage): void {
    const change = this.reduce(message);
    if (change) this.onChange.fire(change);
  }

  /** Adds a workspace fetched over REST (`/api/snapshot?workspace=`). */
  addView(view: WorkspaceView): void {
    this.setView(view);
    this.onChange.fire({ tabs: true, workspaces: true });
  }

  /** Fills in titles of live agents from saved session summaries (the snapshot has them only for saved ones). */
  applyTitles(sessions: { id: string; title: string }[]): void {
    const changed = new Set<string>();
    for (const session of sessions) {
      const agent = this.agents.get(session.id);
      if (agent && !agent.title && session.title && !session.title.startsWith("<attachment ")) {
        agent.title = session.title;
        changed.add(agent.id);
      }
    }
    if (changed.size) this.onChange.fire({ agents: changed, tabs: true });
  }

  /** Voice settings from `PUT /api/voice` (the `voice` broadcast follows, but the reply is first). */
  setVoice(voice: VoiceStatus): void {
    this.voice = voice;
    this.onChange.fire({ voice: true });
  }

  reset(): void {
    this.loaded = false;
    this.onChange.fire({ snapshot: true });
  }

  private reduce(message: ServerMessage): Change | null {
    switch (message.type) {
      case "snapshot":
        return this.applySnapshot(message as unknown as Snapshot);
      case "event":
        return this.applyEvent(message.tab as string | null, message.envelope as Envelope);
      case "workspace_opened":
      case "launched": {
        const view = message.workspace as WorkspaceView;
        this.setView(view);
        this.current = view.id;
        return { tabs: true, workspaces: true };
      }
      case "workspace_created":
      case "workspace_updated": {
        const summary = message.workspace as WorkspaceSummary;
        this.workspaces.set(summary.id, { ...this.workspaces.get(summary.id), ...summary });
        const view = this.views.get(summary.id);
        if (view) Object.assign(view, { name: summary.name, active_tab: summary.active_tab, updated: summary.updated });
        return { workspaces: true, tabs: true };
      }
      case "workspace_deleted": {
        const id = message.workspace as string;
        this.workspaces.delete(id);
        this.views.delete(id);
        if (this.current === id) this.current = null;
        return { workspaces: true, tabs: true };
      }
      case "tab_created": {
        const view = this.views.get(message.workspace as string);
        const tab = message.tab as TabView;
        if (view && !view.tabs.some((t) => t.id === tab.id)) view.tabs.splice(Math.min(Number(message.index), view.tabs.length), 0, tab);
        this.applyTab(tab);
        return { tabs: true, agents: new Set(tab.snapshots.map((s) => s.id)) };
      }
      case "tab_updated": {
        const view = this.views.get(message.workspace as string);
        const tab = message.tab as TabView;
        if (view) {
          const index = view.tabs.findIndex((t) => t.id === tab.id);
          if (index >= 0) view.tabs[index] = tab;
          else view.tabs.push(tab);
        }
        this.applyTab(tab);
        return { tabs: true, mode: true, agents: new Set(tab.snapshots.map((s) => s.id)) };
      }
      case "tab_closed": {
        const view = this.views.get(message.workspace as string);
        const id = message.tab as string;
        if (view) view.tabs = view.tabs.filter((t) => t.id !== id);
        return { tabs: true, closedTab: id };
      }
      case "tabs_reordered": {
        const view = this.views.get(message.workspace as string);
        if (view) {
          const byId = new Map(view.tabs.map((t) => [t.id, t]));
          view.tabs = (message.tabs as string[]).map((id) => byId.get(id)).filter((t): t is TabView => Boolean(t));
        }
        return { tabs: true };
      }
      case "job_started":
      case "job_finished": {
        const job = message.job as JobView;
        this.jobs.set(job.id, job);
        return { jobs: true };
      }
      case "jobs": {
        this.jobs = new Map((message.jobs as JobView[]).map((j) => [j.id, j]));
        return { jobs: true };
      }
      case "mode": {
        const tab = message.tab as string | null;
        const workspace = message.workspace as string | null;
        for (const view of this.views.values()) {
          if (!tab && workspace && view.id !== workspace) continue;
          for (const t of view.tabs) if (!tab || t.id === tab) t.mode = message.mode as Mode;
        }
        return { mode: true, tabs: true };
      }
      case "clients":
        if (this.daemon) this.daemon.clients = Number(message.clients);
        return null;
      case "notice":
        return { notice: { level: String(message.level), message: String(message.message), tab: (message.tab as string) ?? null } };
      case "voice":
        this.voice = (message.voice as VoiceStatus) ?? null;
        return { voice: true };
      case "models":
        // The user's configuration changed: a provider added, a key stored, the default moved.
        this.models = Array.isArray(message.models) ? message.models.map(String) : [];
        this.defaultModel = typeof message.default_model === "string" ? message.default_model : null;
        return { models: true };
      case "say":
        return { say: message as unknown as SayMessage };
      case "say_cancelled":
        return { sayCancelled: Array.isArray(message.ids) ? message.ids.map(String) : [] };
      case "shutdown":
        return { shutdown: String(message.reason ?? "stopped") };
      default:
        return null;
    }
  }

  private applySnapshot(snapshot: Snapshot): Change {
    this.daemon = snapshot.daemon;
    this.workspaces = new Map((snapshot.workspaces ?? []).map((w) => [w.id, w]));
    this.pending = new Map((snapshot.pending ?? []).map((p) => [p.id, p]));
    this.jobs = new Map((snapshot.jobs ?? []).map((j) => [j.id, j]));
    this.models = snapshot.models ?? [];
    this.defaultModel = snapshot.default_model ?? null;
    this.voice = snapshot.voice ?? null;
    // Views of other workspaces are refetched on demand.
    this.views.clear();
    for (const agent of this.agents.values()) {
      agent.live = false;
      agent.events = [];
      agent.trimmed = false;
    }
    this.current = snapshot.workspace?.id ?? null;
    if (snapshot.workspace) this.setView(snapshot.workspace);
    this.loaded = true;
    this.generation += 1;
    return { snapshot: true, workspaces: true, tabs: true, pending: true, jobs: true, voice: true, models: true };
  }

  private setView(view: WorkspaceView): void {
    this.views.set(view.id, view);
    for (const tab of view.tabs) this.applyTab(tab);
    const running = view.tabs.flatMap((t) => t.snapshots).filter((a) => a.status === "running").length;
    const summary = this.workspaces.get(view.id);
    this.workspaces.set(view.id, {
      id: view.id,
      name: view.name,
      tabs: view.tabs.length,
      active_tab: view.active_tab,
      running: summary?.running ?? running,
      created: view.created,
      updated: view.updated,
    });
  }

  private applyTab(tab: TabView): void {
    for (const view of tab.snapshots ?? []) this.applyAgentView(view, tab.id);
  }

  private applyAgentView(view: AgentView, tab: string): void {
    const agent = this.ensureAgent(view.id);
    Object.assign(agent, {
      label: view.label,
      parent: view.parent,
      tab,
      model: view.model,
      cwd: view.cwd,
      created: view.created,
      status: view.status,
      activity: view.activity,
      usage: view.usage ?? agent.usage,
      report: view.report,
      readOnly: Boolean(view.read_only),
      live: Boolean(view.live),
    });
    if (view.title) agent.title = view.title;
  }

  ensureAgent(id: string): AgentState {
    let agent = this.agents.get(id);
    if (!agent) {
      agent = {
        id,
        label: "agent",
        title: "",
        parent: null,
        tab: null,
        model: "",
        cwd: "",
        created: null,
        status: "idle",
        activity: "",
        usage: emptyUsage(),
        contextTokens: 0,
        report: null,
        readOnly: false,
        live: false,
        events: [],
        trimmed: false,
        running: new Map(),
        turnStarted: null,
      };
      this.agents.set(id, agent);
    }
    return agent;
  }

  private applyEvent(tab: string | null, env: Envelope): Change | null {
    if (!env || env.type === "update") return null;
    const agent = this.ensureAgent(env.agent);
    if (tab) agent.tab = tab;
    const change: Change = { agents: new Set([agent.id]), events: new Map([[agent.id, [env]]]) };
    // The answer names only the request; the chat finds the tool card by its call.
    if (env.type === "permission_resolved" && !env.call_id) {
      const call = this.pending.get(String(env.request_id))?.call_id;
      if (call) env.call_id = call;
    }
    this.record(agent, env);
    this.track(agent, env, change);
    if (env.type === "permission_requested") {
      const request: PendingApproval = {
        id: String(env.request_id),
        call_id: String(env.call_id ?? ""),
        agent: agent.id,
        agent_label: agent.label,
        tool: String(env.tool ?? ""),
        summary: String(env.summary ?? ""),
        input: env.input,
        tab: tab ?? agent.tab,
      };
      this.pending.set(request.id, request);
      change.pending = true;
      change.requested = [request];
    } else if (env.type === "permission_resolved") {
      this.pending.delete(String(env.request_id));
      change.pending = true;
    }
    if (["user_message", "turn_finished", "agent_started", "agent_finished"].includes(env.type)) change.tabs = true;
    return change;
  }

  private record(agent: AgentState, env: Envelope): void {
    const n = ++this.received;
    const last = agent.events.at(-1);
    if (last && last.env.type === env.type && (env.type === "text_delta" || env.type === "thinking_delta")) {
      last.env = { ...last.env, text: String(last.env.text ?? "") + String(env.text ?? "") };
      last.n = n;
    } else {
      agent.events.push({ n, env });
    }
    if (agent.events.length > EVENT_LIMIT) {
      agent.events.splice(0, agent.events.length - EVENT_LIMIT);
      agent.trimmed = true;
    }
  }

  private track(agent: AgentState, env: Envelope, change: Change): void {
    switch (env.type) {
      case "agent_started":
        Object.assign(agent, {
          label: String(env.label ?? agent.label),
          parent: (env.parent as string | null) ?? null,
          model: String(env.model ?? agent.model),
          cwd: String(env.cwd ?? agent.cwd),
          created: env.at,
          status: "idle",
          live: true,
        });
        if (agent.parent && !agent.tab) agent.tab = this.agents.get(agent.parent)?.tab ?? null;
        break;
      case "user_message":
        agent.status = "running";
        agent.live = true;
        agent.running.clear();
        agent.turnStarted = env.at;
        if (!agent.title) agent.title = firstLine(String(env.text ?? "")).slice(0, 80);
        break;
      case "model_changed":
        agent.model = String(env.model ?? agent.model);
        break;
      case "tool_started":
        agent.running.set(String(env.call_id), `${env.tool} ${env.summary ?? ""}`.trim());
        break;
      case "tool_finished":
        agent.running.delete(String(env.call_id));
        break;
      case "usage":
        agent.usage = (env.total as Usage) ?? agent.usage;
        agent.contextTokens = Number(env.context_tokens) || agent.contextTokens;
        break;
      case "turn_finished":
        agent.status = env.status as AgentStatus;
        agent.report = String(env.text ?? "");
        agent.running.clear();
        agent.turnStarted = null;
        (change.finished ??= []).push({ agent: agent.id, status: String(env.status) });
        break;
      case "agent_finished":
        agent.status = env.status as AgentStatus;
        agent.report = String(env.report ?? "");
        break;
    }
    agent.activity = nextActivity(agent, env);
  }

  // Queries

  tab(id: string | null | undefined): { tab: TabView; workspace: WorkspaceView } | null {
    if (!id) return null;
    for (const workspace of this.views.values()) {
      const tab = workspace.tabs.find((t) => t.id === id);
      if (tab) return { tab, workspace };
    }
    return null;
  }

  /** Every tab known, the connection's workspace first. */
  allTabs(): { tab: TabView; workspace: WorkspaceView }[] {
    const views = [...this.views.values()].sort((a, b) => Number(b.id === this.current) - Number(a.id === this.current));
    return views.flatMap((workspace) => workspace.tabs.map((tab) => ({ tab, workspace })));
  }

  tabOfAgent(id: string): TabView | null {
    for (const { tab } of this.allTabs()) {
      if (tab.agents.includes(id) || tab.snapshots.some((s) => s.id === id)) return tab;
    }
    const agent = this.agents.get(id);
    if (agent?.tab) return this.tab(agent.tab)?.tab ?? null;
    for (let current = agent; current?.parent; current = this.agents.get(current.parent)) {
      const tab = this.allTabs().find(({ tab }) => tab.agents.includes(current!.parent!));
      if (tab) return tab.tab;
    }
    return null;
  }

  /** A tab of the connection's workspace (else any) whose directory is `cwd`. */
  tabForCwd(cwd: string): TabView | null {
    const want = normalizeDir(cwd);
    return this.allTabs().find(({ tab }) => normalizeDir(tab.cwd) === want)?.tab ?? null;
  }

  /** Top-level agents of a tab in order, then the live subagents under them. */
  agentsOfTab(tab: TabView): AgentState[] {
    const top = tab.agents.map((id) => this.agents.get(id)).filter((a): a is AgentState => Boolean(a));
    const ids = new Set(top.map((a) => a.id));
    const subagents = [...this.agents.values()].filter((a) => a.parent && !ids.has(a.id) && this.rootOf(a)?.tab === tab.id && ids.has(this.rootOf(a)!.id));
    return [...top, ...subagents.sort((a, b) => String(a.created ?? "").localeCompare(String(b.created ?? "")))];
  }

  childrenOf(id: string): AgentState[] {
    return [...this.agents.values()]
      .filter((a) => a.parent === id)
      .sort((a, b) => String(a.created ?? "").localeCompare(String(b.created ?? "")));
  }

  rootOf(agent: AgentState): AgentState | null {
    let current: AgentState | undefined = agent;
    for (let depth = 0; current?.parent && depth < 32; depth += 1) current = this.agents.get(current.parent);
    return current ?? null;
  }

  isWaiting(agentId: string): boolean {
    for (const request of this.pending.values()) if (request.agent === agentId) return true;
    return false;
  }

  agentStatus(agent: AgentState): DisplayStatus {
    if (this.isWaiting(agent.id)) return "waiting";
    if (!agent.live) return agent.status === "failed" ? "failed" : "saved";
    if (agent.status === "running") return "running";
    if (agent.status === "failed") return "failed";
    if (agent.status === "completed") return "completed";
    if (agent.status === "cancelled") return "cancelled";
    return "idle";
  }

  /** Waiting or running when any agent is; otherwise how its active agent ended. */
  tabStatus(tab: TabView): DisplayStatus {
    const agents = this.agentsOfTab(tab);
    const statuses = agents.map((a) => this.agentStatus(a));
    if ([...this.pending.values()].some((r) => r.tab === tab.id) || statuses.includes("waiting")) return "waiting";
    if (statuses.includes("running")) return "running";
    const active = this.agents.get(tab.active_agent ?? tab.agents.at(-1) ?? "");
    const status = active ? this.agentStatus(active) : "idle";
    return status === "failed" || status === "completed" ? status : "idle";
  }

  runningAgents(): AgentState[] {
    return [...this.agents.values()].filter((a) => a.live && a.status === "running");
  }
}

function nextActivity(agent: AgentState, env: Envelope): string {
  switch (env.type) {
    case "user_message":
    case "thinking_delta":
      return "thinking";
    case "text_delta":
      return "writing";
    case "tool_call_streaming":
      return `preparing ${env.tool}`;
    case "tool_started":
    case "tool_finished":
    case "permission_resolved":
      return [...agent.running.values()].at(-1) ?? "thinking";
    case "permission_requested":
      return `waiting for approval: ${env.tool}`;
    case "turn_finished":
      return "";
    default:
      return agent.activity;
  }
}

export function firstLine(text: string): string {
  return (text.split("\n").find((line) => line.trim()) ?? "").trim();
}

export function normalizeDir(dir: string): string {
  return dir.replace(/[\\/]+$/, "") || dir;
}

export function displayName(agent: Pick<AgentState, "title" | "label" | "parent">): string {
  return (!agent.parent && agent.title) || agent.label;
}

// The chat's logic, independent of VS Code: which tab and agent it shows,
// the state the webview renders, transcripts and live events, and what the
// webview's messages do. The webview view (and the browser harness) supply a
// ChatHost for everything editor-specific.

import type { DaemonClient } from "../connection/client";
import { HttpError } from "../connection/client";
import { ContextChip, chipFor, contextPayload, EditorSnapshot, withContext } from "../model/context";
import { Disposable, Emitter } from "../model/emitter";
import type { FileHit } from "../model/files";
import { AgentState, Change, DaemonStore, displayName } from "../model/store";
import type { AttachmentReport, Envelope, ImageInput, Message, Mode } from "../protocol";
import type { AgentSummary, Banner, ChatState, FromWebview, ToWebview } from "./messages";
import { RecordedFilter, replayEvents } from "./replay";

export interface ChatHost {
  post(message: ToWebview): void;
  /** The active editor, for the context chip. */
  editor(): EditorSnapshot | null;
  findFiles(query: string): Promise<FileHit[]>;
  openFile(path: string, line?: number): void;
  /** Saves a dropped file that has no path and returns where it went. */
  saveUpload(name: string, data: Buffer): Promise<string>;
  runCommand(command: string, args: unknown[]): void;
  remember(key: string, value: unknown): void;
  recall<T>(key: string): T | undefined;
  log(line: string): void;
  /** The malac version the extension expects, for the mismatch banner. */
  clientVersion(): string | undefined;
  /** How to drop files into the chat in this host. */
  dropHint: string;
}

const STATE_DEBOUNCE_MS = 40;
const EVENT_FLUSH_MS = 30;
/** Commands the webview may run through banner buttons. */
const ALLOWED_COMMANDS = /^(malac\.[\w.]+|workbench\.action\.openSettings)$/;

export class ChatController implements Disposable {
  private tabId: string | null = null;
  /** The agent shown per tab; null is a new agent. */
  private agentByTab = new Map<string, string | null>();
  private newModel: string | null = null;
  private contextEnabled: boolean;
  private externalBanners = new Map<string, Banner[]>();
  private ready = false;
  private stateTimer: ReturnType<typeof setTimeout> | undefined;
  private eventTimer: ReturnType<typeof setTimeout> | undefined;
  private outbox: Envelope[] = [];
  /** The agent whose transcript the webview has (or is loading). */
  private shown: string | null | undefined = undefined;
  private loading = false;
  private loadToken = 0;
  /** Drops live events that repeat the transcript just sent. */
  private recorded: RecordedFilter | null = null;
  /** Prompts with images, matched to their user_message events by text. */
  private outgoing: { text: string; images: ImageInput[] }[] = [];
  private disposables: Disposable[] = [];

  readonly onSelection = new Emitter<{ tab: string | null; agent: string | null }>();

  constructor(
    private readonly store: DaemonStore,
    private readonly client: DaemonClient,
    private readonly host: ChatHost,
    { includeCurrentFile = true }: { includeCurrentFile?: boolean } = {},
  ) {
    this.contextEnabled = host.recall<boolean>("chat.context") ?? includeCurrentFile;
    this.tabId = host.recall<string>("chat.tab") ?? null;
    this.disposables.push(
      store.onChange.event((change) => this.onStoreChange(change)),
      client.onStatus.event(() => this.scheduleState()),
    );
  }

  dispose(): void {
    clearTimeout(this.stateTimer);
    clearTimeout(this.eventTimer);
    for (const d of this.disposables) d.dispose();
    this.onSelection.dispose();
  }

  // Selection

  get currentTab(): string | null {
    return this.store.tab(this.tabId) ? this.tabId : null;
  }

  get currentAgent(): string | null {
    const tab = this.currentTab;
    if (!tab) return null;
    return this.agentByTab.has(tab) ? (this.agentByTab.get(tab) ?? null) : this.defaultAgent(tab);
  }

  /** Shows a tab, and an agent in it (`null` for a new agent; omitted keeps the tab's last one). */
  selectTab(tab: string | null, agent?: string | null): void {
    this.tabId = tab;
    if (tab && agent !== undefined) this.agentByTab.set(tab, agent);
    this.host.remember("chat.tab", tab);
    if (tab && this.store.current && this.store.tab(tab)?.workspace.id === this.store.current) {
      this.client.send({ type: "tab_activate", tab });
    }
    this.selectionChanged();
  }

  /** Shows an agent wherever it lives. */
  selectAgent(agent: string): void {
    const tab = this.store.tabOfAgent(agent);
    if (tab) {
      this.selectTab(tab.id, agent);
      const top = tab.agents.includes(agent);
      if (top && tab.active_agent !== agent) this.client.send({ type: "agent_activate", tab: tab.id, agent });
    }
  }

  /** The next prompt starts a new agent in `tab` (default: the chat's). */
  newAgent(tab?: string): void {
    const target = tab ?? this.currentTab;
    if (!target) return;
    this.selectTab(target, null);
    this.focus();
  }

  focus(text?: string): void {
    this.host.post({ type: "focus", text });
  }

  addAttachments(paths: { path: string; label: string }[], images: ImageInput[] = []): void {
    if (paths.length || images.length) this.host.post({ type: "attach", paths, images });
  }

  setContextEnabled(enabled: boolean): void {
    this.contextEnabled = enabled;
    this.host.remember("chat.context", enabled);
    this.scheduleState();
  }

  /** Banners from elsewhere in the extension (the folder offer, say). */
  setBanners(source: string, banners: Banner[]): void {
    if (banners.length) this.externalBanners.set(source, banners);
    else this.externalBanners.delete(source);
    this.scheduleState();
  }

  /** The active editor or its selection changed. */
  editorChanged(): void {
    this.scheduleState();
  }

  stop(agent = this.currentAgent): boolean {
    if (!agent) return false;
    return this.client.send({ type: "cancel", agent });
  }

  private defaultAgent(tab: string): string | null {
    const view = this.store.tab(tab)?.tab;
    if (!view) return null;
    return view.active_agent ?? view.agents.at(-1) ?? null;
  }

  private selectionChanged(): void {
    this.onSelection.fire({ tab: this.currentTab, agent: this.currentAgent });
    this.syncTranscript();
    this.scheduleState();
  }

  // Webview

  receive(message: FromWebview): void {
    switch (message.type) {
      case "ready":
        this.ready = true;
        this.shown = undefined;
        this.pushState();
        this.syncTranscript();
        return;
      case "submit":
        void this.submit(message.text, message.attachments, message.images);
        return;
      case "stop":
        this.stop();
        return;
      case "selectAgent":
        if (message.agent) this.selectAgent(message.agent);
        else this.newAgent();
        return;
      case "selectTab":
        this.selectTab(message.tab);
        return;
      case "setModel":
        this.setModel(message.model);
        return;
      case "setMode":
        this.setMode(message.mode);
        return;
      case "toggleContext":
        this.setContextEnabled(message.enabled);
        return;
      case "searchFiles":
        void this.host
          .findFiles(message.query)
          .then((files) => this.host.post({ type: "files", id: message.id, files }))
          .catch(() => this.host.post({ type: "files", id: message.id, files: [] }));
        return;
      case "upload":
        void this.upload(message.files);
        return;
      case "approve":
        this.client
          .request({ type: "approve", request_id: message.request, decision: message.decision })
          .catch((error: Error) => this.toast("warning", error.message));
        return;
      case "openFile":
        this.host.openFile(message.path, message.line);
        return;
      case "openAgent":
        this.selectAgent(message.agent);
        return;
      case "command":
        if (ALLOWED_COMMANDS.test(message.command)) this.host.runCommand(message.command, message.args ?? []);
        return;
    }
  }

  private toast(level: "info" | "warning" | "error", message: string): void {
    this.host.post({ type: "toast", level, message });
  }

  private setModel(model: string): void {
    const agent = this.currentAgent;
    if (agent) {
      this.client.send({ type: "model", agent, model });
      const state = this.store.agents.get(agent);
      if (state) state.model = model;
    } else {
      this.newModel = model;
    }
    this.scheduleState();
  }

  private setMode(mode: Mode): void {
    const tab = this.currentTab;
    if (!tab) return;
    if (this.client.send({ type: "mode", mode, tab })) {
      const view = this.store.tab(tab)?.tab;
      if (view) view.mode = mode;
    }
    this.scheduleState();
  }

  async submit(text: string, attachments: { path: string }[], images: ImageInput[]): Promise<void> {
    try {
      const tab = this.currentTab;
      if (!tab) throw new Error("Open a malac tab for this folder first.");
      const editor = this.host.editor();
      const chip = chipFor(editor, this.contextEnabled);
      const payload = contextPayload(chip, editor);
      const paths = [...new Set([...attachments.map((a) => a.path), ...payload.attachments.map((a) => a.path)])];
      const prompt = withContext(text.trim(), payload.inline);
      if (!prompt && !paths.length && !images.length) return;
      const agent = this.currentAgent;
      const command: Record<string, unknown> = { type: "prompt", text: prompt, images, attachments: paths.map((path) => ({ path })) };
      if (agent) command.agent = agent;
      else {
        command.tab = tab;
        const model = this.newModel ?? this.store.tab(tab)?.tab.options.model ?? null;
        if (model) command.model = model;
      }
      if (images.length) this.outgoing.push({ text: prompt, images });
      const reply = await this.client.request(command);
      const notes = ((reply.attachments as AttachmentReport[] | undefined) ?? [])
        .filter((a) => a.note)
        .map((a) => `${a.path.split(/[\\/]/).at(-1)}: ${a.note}`);
      this.host.post({ type: "sent", ok: true, notes });
      const replyAgent = reply.agent as string | undefined;
      const replyTab = (reply.tab as string | undefined) ?? tab;
      if (replyAgent && replyAgent !== agent) {
        this.newModel = null;
        this.selectTab(replyTab, replyAgent);
      }
    } catch (error) {
      this.host.post({ type: "sent", ok: false, error: error instanceof Error ? error.message : String(error) });
    }
  }

  private async upload(files: { name: string; type: string; data: string }[]): Promise<void> {
    const paths: { path: string; label: string }[] = [];
    for (const file of files) {
      try {
        const path = await this.host.saveUpload(file.name, Buffer.from(file.data, "base64"));
        paths.push({ path, label: file.name });
      } catch (error) {
        this.toast("error", `Could not attach ${file.name}: ${error instanceof Error ? error.message : String(error)}`);
      }
    }
    this.addAttachments(paths);
  }

  // State

  private onStoreChange(change: Change): void {
    if (change.snapshot) {
      this.shown = undefined;
      this.outbox = [];
      void this.client
        .get<{ sessions: { id: string; title: string }[] }>("/api/sessions?limit=200&subagents=true")
        .then((data) => this.store.applyTitles(data.sessions ?? []))
        .catch(() => undefined);
    }
    if (change.closedTab && change.closedTab === this.tabId) {
      const next = this.store.allTabs()[0]?.tab.id ?? null;
      this.selectTab(next);
    }
    if (change.events) {
      for (const [agent, events] of change.events) {
        for (const env of events) if (env.type === "user_message") this.claimImages(env);
        if (agent === this.shown && !this.loading && this.ready) this.queueEvents(events);
      }
    }
    // A selection that went away (a closed agent, say) falls back to the tab's default.
    const tab = this.currentTab;
    if (tab && this.agentByTab.has(tab)) {
      const chosen = this.agentByTab.get(tab);
      if (chosen && !this.store.agents.has(chosen)) this.agentByTab.delete(tab);
      else if (chosen && change.tabs) {
        const view = this.store.tab(tab)?.tab;
        const known = view?.agents.includes(chosen) || this.store.agents.get(chosen)?.parent;
        if (view && !known) this.agentByTab.delete(tab);
      }
    }
    if (!this.tabId && this.store.loaded) {
      const fallback = this.store.views.get(this.store.current ?? "")?.active_tab ?? this.store.allTabs()[0]?.tab.id ?? null;
      if (fallback) this.selectTab(fallback);
    }
    if (change.snapshot || change.tabs || change.agents) this.syncTranscript();
    this.scheduleState();
  }

  private claimImages(env: Envelope): void {
    const text = String(env.text ?? "");
    const index = this.outgoing.findIndex((item) => text.startsWith(item.text));
    if (index >= 0) env.images = this.outgoing.splice(index, 1)[0].images;
  }

  private queueEvents(events: Envelope[]): void {
    for (const env of events) {
      const passed = this.recorded ? this.recorded.pass(env) : env;
      if (passed) this.outbox.push(passed);
    }
    this.eventTimer ??= setTimeout(() => this.flushEvents(), EVENT_FLUSH_MS);
  }

  private flushEvents(): void {
    this.eventTimer = undefined;
    const events = this.outbox.splice(0);
    if (events.length && this.shown) this.host.post({ type: "events", agent: this.shown, events });
  }

  /** Loads the shown agent's transcript when the selection changed. */
  private syncTranscript(): void {
    if (!this.ready || !this.store.loaded) return;
    const agent = this.currentAgent;
    if (agent === this.shown) return;
    this.shown = agent;
    this.outbox = [];
    clearTimeout(this.eventTimer);
    this.eventTimer = undefined;
    void this.loadTranscript(agent);
  }

  /** Shows the transcript again (after a reconnect, say). */
  reloadTranscript(): void {
    this.shown = undefined;
    this.syncTranscript();
  }

  private async loadTranscript(agent: string | null): Promise<void> {
    const token = ++this.loadToken;
    if (!agent) {
      this.loading = false;
      this.recorded = null;
      this.host.post({ type: "transcript", agent: null, history: [], events: [] });
      return;
    }
    this.loading = true;
    const cutoff = this.store.received;
    let history: Message[] = [];
    let error: string | undefined;
    try {
      const data = await this.client.get<{ history?: Message[] }>(`/api/sessions/${encodeURIComponent(agent)}`);
      history = data.history ?? [];
    } catch (failure) {
      // A brand-new agent has no transcript file until its first message.
      const state = this.store.agents.get(agent);
      const missing = failure instanceof HttpError && failure.status === 404;
      if (!(missing && state?.live)) error = failure instanceof Error ? failure.message : String(failure);
    }
    if (token !== this.loadToken) return;
    const state = this.store.agents.get(agent);
    const events = state ? replayEvents(state.events, cutoff, history) : [];
    this.loading = false;
    this.recorded = new RecordedFilter(history.slice(-4));
    this.host.post({ type: "transcript", agent, history, events, error });
  }

  private scheduleState(): void {
    if (!this.ready) return;
    this.stateTimer ??= setTimeout(() => {
      this.stateTimer = undefined;
      this.pushState();
    }, STATE_DEBOUNCE_MS);
  }

  private pushState(): void {
    clearTimeout(this.stateTimer);
    this.stateTimer = undefined;
    if (this.ready) this.host.post({ type: "state", state: this.state() });
  }

  state(): ChatState {
    const store = this.store;
    const entry = store.tab(this.currentTab);
    const agentId = this.currentAgent;
    const agents = entry ? store.agentsOfTab(entry.tab).map((a) => this.summary(a)) : [];
    const current = agentId ? store.agents.get(agentId) : undefined;
    const tabAgents = new Set(agents.map((a) => a.id));
    const pending = [...store.pending.values()]
      .filter((r) => (entry && r.tab === entry.tab.id) || tabAgents.has(r.agent))
      .map((r) => ({ id: r.id, callId: r.call_id, agent: r.agent, agentLabel: r.agent_label, tool: r.tool, summary: r.summary, input: r.input }));
    return {
      connection: { state: this.client.connection.state, detail: this.client.connection.detail },
      banners: this.banners(),
      tab: entry ? { id: entry.tab.id, name: entry.tab.name, cwd: entry.tab.cwd, mode: entry.tab.mode, workspace: entry.workspace.id } : null,
      tabs: store.allTabs().map(({ tab, workspace }) => ({ id: tab.id, name: tab.name, cwd: tab.cwd, workspace: workspace.id, status: store.tabStatus(tab) })),
      agents,
      agent: current
        ? {
            ...this.summary(current),
            usage: current.usage,
            contextTokens: current.contextTokens,
            readOnly: current.readOnly,
            running: current.live && current.status === "running",
          }
        : null,
      models: store.models,
      defaultModel: store.defaultModel,
      newModel: this.newModel ?? entry?.tab.options.model ?? null,
      pending,
      context: this.chip(),
      home: store.daemon?.user_home ?? null,
      dropHint: this.host.dropHint,
    };
  }

  private chip(): ContextChip | null {
    return chipFor(this.host.editor(), this.contextEnabled);
  }

  private summary(agent: AgentState): AgentSummary {
    let depth = 0;
    for (let a: AgentState | undefined = agent; a?.parent && depth < 16; a = this.store.agents.get(a.parent)) depth += 1;
    return {
      id: agent.id,
      name: displayName(agent) || "agent",
      label: agent.label,
      parent: agent.parent,
      depth,
      status: this.store.agentStatus(agent),
      live: agent.live,
      model: agent.model,
      activity: agent.activity,
    };
  }

  private banners(): Banner[] {
    const banners: Banner[] = [];
    const connection = this.client.connection;
    if (connection.state === "connecting" && !this.store.loaded) {
      banners.push({ id: "connecting", level: "info", text: "Connecting to malac…", actions: [] });
    } else if (connection.state === "offline") {
      banners.push({
        id: "offline",
        level: "warning",
        text: `Reconnecting to malac${connection.detail ? `: ${connection.detail}` : ""}`,
        actions: [{ label: "Retry now", command: "malac.reconnect" }],
      });
    } else if (connection.state === "stopped") {
      banners.push({
        id: "stopped",
        level: "warning",
        text: `The malac daemon stopped${connection.detail ? ` (${connection.detail.replace(/^the daemon stopped \(|\)$/g, "")})` : ""}.`,
        actions: [{ label: "Start daemon", command: "malac.reconnect", primary: true }],
      });
    } else if (connection.state === "failed") {
      banners.push({
        id: "failed",
        level: "error",
        text: connection.detail || "malac cannot connect.",
        actions: [
          { label: "Open settings", command: "workbench.action.openSettings", args: ["malac."] },
          { label: "Show log", command: "malac.showLog" },
        ],
      });
    }
    const daemon = this.store.daemon;
    if (connection.state === "online" && daemon?.version_mismatch) {
      const mine = this.host.clientVersion();
      banners.push({
        id: "mismatch",
        level: "warning",
        text: `The daemon runs malac ${daemon.version}${mine ? `, but this malac is ${mine}` : ""}. Restart it to switch.`,
        actions: [{ label: "Restart daemon", command: "malac.restartDaemon" }],
      });
    }
    for (const list of this.externalBanners.values()) banners.push(...list);
    return banners;
  }
}

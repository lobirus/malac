// Client state, kept current from the daemon: the first WebSocket message
// is a snapshot of the connection's workspace, then broadcasts (deltas and
// agent events) keep it up to date. Also cached REST resources and a
// per-frame render scheduler.

import { emptyUsage, firstLine, local, sessionStore, setUserHome, toast, truncate } from "./dom.js";

const TOKEN_KEY = "malac.token";
/** A paired browser's device token, kept across restarts (see remote.js). */
const DEVICE_KEY = "malac.device";
const RECONNECT_MIN_MS = 500;
const RECONNECT_MAX_MS = 10_000;
const NEW_AGENT_WINDOW_MS = 5_000;
const HISTORY_REFRESH_MS = 1500;
export const DEFAULT_CONTEXT_WINDOW = 200_000;
export const STATUS_TEXT = { idle: "Idle", running: "Running", completed: "Done", failed: "Failed", cancelled: "Stopped" };

export const params = new URLSearchParams(location.search);

export const state = {
  token: null,
  // Daemon and workspace, from the snapshot.
  daemon: null,
  version: "",
  pageVersion: null,
  workspaces: [],
  workspace: null,
  tab: null,
  jobs: new Map(),
  control: null,
  update: null,
  // Remote access and keep-awake, from the snapshot.
  remote: null,
  awake: null,
  remoteChanges: 0,
  // Dictation, speech and talk mode (VoiceStatus), from the snapshot.
  voice: null,
  models: [],
  defaultModel: null,
  shutdown: null,
  // Derived from the selected tab, for views that predate tabs.
  cwd: "",
  mode: "ask",
  // Saved sessions on disk, newest first.
  sessions: [],
  web: { remote: false, reloadable: true },
  loaded: false,
  agents: new Map(),
  pending: new Map(),
  route: { name: "home" },
  selected: null,
  views: new Set(),
  received: 0,
  generation: 0,
  socket: null,
  connection: "connecting",
  backoff: RECONNECT_MIN_MS,
  callbacks: new Map(),
  nextRef: 1,
  inbox: [],
  historyTimer: 0,
};

// HTTP

export class HttpError extends Error {
  constructor(status, message) {
    super(message);
    this.status = status;
  }
}

export function readToken() {
  const token = params.get("token");
  if (!token) return sessionStore.get(TOKEN_KEY) ?? local.get(DEVICE_KEY);
  sessionStore.set(TOKEN_KEY, token);
  params.delete("token");
  const query = params.toString();
  history.replaceState(null, "", `${location.pathname}${query ? `?${query}` : ""}${location.hash}`);
  return token;
}

/**
 * Calls the API; a `{error}` body becomes the thrown message. A Blob body
 * (a recording, say) is sent as it is, with its own type.
 */
export async function api(path, { method = "GET", body } = {}) {
  const headers = { Authorization: `Bearer ${state.token}` };
  const raw = body instanceof Blob;
  if (body !== undefined) headers["Content-Type"] = raw ? body.type : "application/json";
  const response = await fetch(path, { method, headers, body: body === undefined ? undefined : raw ? body : JSON.stringify(body) });
  const text = await response.text();
  let data = null;
  try {
    data = text ? JSON.parse(text) : null;
  } catch {
    data = null;
  }
  if (!response.ok) throw new HttpError(response.status, data?.error ?? (text || `${response.status} ${response.statusText}`));
  return data;
}

/** Fetches a file from the API (a shared file, speech audio) as a Blob. */
export async function apiBlob(path) {
  const response = await fetch(path, { headers: { Authorization: `Bearer ${state.token}` } });
  if (!response.ok) {
    const text = await response.text().catch(() => "");
    let message = text;
    try {
      message = JSON.parse(text).error ?? text;
    } catch {
      // A plain-text error.
    }
    throw new HttpError(response.status, message || `${response.status} ${response.statusText}`);
  }
  return response.blob();
}

// Rendering is batched per animation frame; regions register a renderer.

const renderers = new Map();
const dirty = new Set();
let frame = 0;

export function registerRenderer(region, render) {
  renderers.set(region, render);
}

export function invalidate(...regions) {
  for (const region of regions) dirty.add(region);
  frame ||= requestAnimationFrame(flush);
}

function flush() {
  frame = 0;
  const regions = [...dirty];
  dirty.clear();
  for (const region of regions) {
    try {
      renderers.get(region)?.();
    } catch (error) {
      console.error(`render ${region}`, error);
    }
  }
}

const keys = new Map();

/** Runs `render` only when `key` changed since the last call for `slot`. */
export function renderKeyed(slot, key, render) {
  if (keys.get(slot) === key) return;
  keys.set(slot, key);
  render();
}

const ALL_REGIONS = ["sidebar", "tabs", "header", "permissions", "composer", "home", "agents", "jobs", "banners", "title", "update", "mode", "voice"];

export function invalidateAll() {
  invalidate(...ALL_REGIONS);
}

// Navigation is owned by the app shell; other modules go through here.

let navigator = () => {};

export function setNavigator(fn) {
  navigator = fn;
}

export function navigate(route) {
  navigator(route);
}

/** Shows an agent's transcript, in its tab when it has one. */
export function openAgent(id) {
  if (!id) return navigate({ name: "home" });
  const tab = tabOfAgent(id);
  navigate(tab ? { name: "tab", tab: tab.id, agent: id } : { name: "agent", id });
}

/** Opens an agent that may not be known yet (a finished subagent, say). */
export function viewAgent(id, defaults) {
  if (!state.agents.has(id)) Object.assign(ensureAgent(id), defaults, { live: false });
  openAgent(id);
}

// WebSocket

/** Forgets the token this page used, after the daemon refused it. */
export function forgetToken() {
  if (sessionStore.get(TOKEN_KEY) === state.token) sessionStore.set(TOKEN_KEY, null);
  if (local.get(DEVICE_KEY) === state.token) local.set(DEVICE_KEY, null);
  state.token = null;
  state.socket?.close();
}

let onAuthFailed = () => {};

/** Called when the daemon no longer accepts this page's token, e.g. revoked. */
export function setAuthFailedHandler(fn) {
  onAuthFailed = fn;
}

async function tokenRefused() {
  try {
    const response = await fetch("/api/daemon", { headers: { Authorization: `Bearer ${state.token}` } });
    return response.status === 401;
  } catch {
    return false;
  }
}

export function connect() {
  if (!state.token) return;
  const scheme = location.protocol === "https:" ? "wss" : "ws";
  const version = state.pageVersion ? `&version=${encodeURIComponent(state.pageVersion)}` : "";
  const socket = new WebSocket(`${scheme}://${location.host}/api/ws?token=${encodeURIComponent(state.token)}${version}`);
  state.socket = socket;
  setConnection("connecting");
  socket.addEventListener("open", () => {
    state.backoff = RECONNECT_MIN_MS;
    setConnection("online");
    // The snapshot that follows replaces everything buffered so far.
    state.generation += 1;
    for (const agent of state.agents.values()) {
      agent.events = [];
      agent.history = null;
    }
  });
  socket.addEventListener("message", (message) => receive(JSON.parse(message.data)));
  socket.addEventListener("close", () => {
    state.socket = null;
    for (const { reject } of state.callbacks.values()) reject(new Error("Disconnected from malac"));
    state.callbacks.clear();
    setConnection("offline");
    tokenRefused().then((refused) => {
      if (refused) return onAuthFailed();
      setTimeout(connect, state.backoff);
      state.backoff = Math.min(state.backoff * 2, RECONNECT_MAX_MS);
    });
  });
}

export function reconnectNow() {
  state.backoff = RECONNECT_MIN_MS;
  if (state.socket) state.socket.close();
  else connect();
}

function setConnection(status) {
  state.connection = status;
  invalidate("connection", "banners");
}

/** Sends a command; `onAck` gets the agent id of an `ack` (old API). */
export function send(message, onAck) {
  if (state.socket?.readyState !== WebSocket.OPEN) {
    toast("Not connected to malac", "error");
    return false;
  }
  if (onAck) {
    message.client_ref = `r${state.nextRef++}`;
    state.callbacks.set(message.client_ref, {
      resolve: (reply) => onAck(reply.agent, reply),
      reject: (error) => toast(error.message, "error"),
    });
  }
  state.socket.send(JSON.stringify(message));
  return true;
}

/** Sends a command and resolves with its reply; an `error` reply rejects. */
export function request(message) {
  return new Promise((resolve, reject) => {
    if (state.socket?.readyState !== WebSocket.OPEN) {
      reject(new Error("Not connected to malac"));
      return;
    }
    const ref = `r${state.nextRef++}`;
    state.callbacks.set(ref, { resolve, reject });
    state.socket.send(JSON.stringify({ ...message, client_ref: ref }));
  });
}

function receive(message) {
  if (message.client_ref && state.callbacks.has(message.client_ref)) {
    const { resolve, reject } = state.callbacks.get(message.client_ref);
    state.callbacks.delete(message.client_ref);
    if (message.type === "error") reject(new Error(message.message));
    else resolve(message);
    // Replies that carry state are applied below as well.
    if (!["snapshot", "workspace_opened", "control", "launched"].includes(message.type)) return;
  }
  switch (message.type) {
    case "snapshot":
      return applySnapshot(message);
    case "event":
      return onEvent(message);
    case "workspace_opened":
    case "launched":
      return setWorkspace(message.workspace);
    case "workspace_created":
      return upsertSummary(message.workspace);
    case "workspace_updated":
      return workspaceUpdated(message.workspace);
    case "workspace_deleted":
      return workspaceDeleted(message.workspace);
    case "tab_created":
      return tabCreated(message);
    case "tab_updated":
      return tabUpdated(message);
    case "tab_closed":
      return tabClosed(message);
    case "tabs_reordered":
      return tabsReordered(message);
    case "job_started":
    case "job_finished":
      return jobChanged(message.job);
    case "control":
      state.control = message.control;
      return invalidate("home", "sidebar", "settings");
    case "mode":
      return modeChanged(message);
    case "clients":
      if (state.daemon) state.daemon.clients = message.clients;
      return invalidate("settings", "home");
    case "remote":
      state.remote = message.remote;
      state.remoteChanges += 1;
      return invalidate("settings");
    case "awake":
      state.awake = message.awake;
      return invalidate("settings", "header");
    case "voice":
      state.voice = message.voice;
      return invalidate("voice", "settings");
    case "say":
    case "say_cancelled":
      return onSay(message);
    case "notice":
      return toast(message.message, message.level === "error" ? "error" : message.level === "warning" ? "warning" : "info");
    case "reloaded":
      return onReloaded();
    case "shutdown":
      state.shutdown = message.reason || "stopped";
      return invalidate("banners");
    case "lagged":
      toast(`Missed ${message.skipped} updates, reloading`, "warning");
      return send({ type: "snapshot" });
    case "error":
      return toast(message.message, "error");
  }
}

// Snapshot and deltas

let onResync = () => {};

export function setResyncHandler(fn) {
  onResync = fn;
}

let onSay = () => {};

/** Called with every `say` and `say_cancelled` broadcast; the page plays the sentences for its own listener id. */
export function setSayHandler(fn) {
  onSay = fn;
}

function applySnapshot(snapshot) {
  state.daemon = snapshot.daemon;
  setUserHome(snapshot.daemon.user_home);
  state.version = snapshot.daemon.version;
  state.pageVersion ??= snapshot.daemon.version;
  state.web = { remote: snapshot.daemon.remote, reloadable: !snapshot.daemon.attached };
  state.workspaces = snapshot.workspaces ?? [];
  state.pending = new Map((snapshot.pending ?? []).map((request) => [request.id, request]));
  state.jobs = new Map((snapshot.jobs ?? []).map((job) => [job.id, job]));
  state.update = snapshot.update ?? state.update;
  state.control = snapshot.control ?? null;
  state.remote = snapshot.remote ?? null;
  state.awake = snapshot.awake ?? null;
  state.voice = snapshot.voice ?? null;
  state.models = snapshot.models ?? [];
  state.defaultModel = snapshot.default_model ?? null;
  state.shutdown = null;
  state.loaded = true;
  setWorkspace(snapshot.workspace, { resync: true });
  loadHistoryList();
}

/** Makes `view` the shown workspace; selects its active tab unless the selected one is still there. */
function setWorkspace(view, { resync = false } = {}) {
  const switched = view?.id !== state.workspace?.id;
  state.workspace = view ?? null;
  for (const agent of state.agents.values()) agent.live = false;
  for (const tab of view?.tabs ?? []) applyTab(tab);
  if (view) upsertSummary(summaryOf(view), { quiet: true });
  if (switched || !view?.tabs.some((t) => t.id === state.tab)) selectTab(view?.active_tab ?? view?.tabs[0]?.id ?? null);
  else syncTabState();
  invalidateAll();
  if (resync || switched) onResync({ switched });
}

function summaryOf(view) {
  const running = view.tabs.flatMap((t) => t.snapshots).filter((a) => a.status === "running").length;
  return { id: view.id, name: view.name, tabs: view.tabs.length, active_tab: view.active_tab, running, created: view.created, updated: view.updated };
}

function upsertSummary(summary, { quiet = false } = {}) {
  const index = state.workspaces.findIndex((w) => w.id === summary.id);
  if (index >= 0) state.workspaces[index] = { ...state.workspaces[index], ...summary };
  else state.workspaces.push(summary);
  if (!quiet) invalidate("sidebar", "home");
}

function workspaceUpdated(summary) {
  upsertSummary(summary);
  if (state.workspace?.id === summary.id) {
    Object.assign(state.workspace, { name: summary.name, active_tab: summary.active_tab, updated: summary.updated });
    invalidate("tabs", "title", "home");
  }
}

function workspaceDeleted(id) {
  state.workspaces = state.workspaces.filter((w) => w.id !== id);
  if (state.workspace?.id === id) {
    state.workspace = null;
    selectTab(null);
    send({ type: "snapshot" });
  }
  invalidateAll();
}

function currentWorkspace(id) {
  return state.workspace && state.workspace.id === id ? state.workspace : null;
}

function tabCreated({ workspace, index, tab }) {
  const view = currentWorkspace(workspace);
  if (!view) return;
  if (!view.tabs.some((t) => t.id === tab.id)) view.tabs.splice(Math.min(index, view.tabs.length), 0, tab);
  applyTab(tab);
  tabWaiters.get(tab.id)?.();
  invalidateAll();
}

function tabUpdated({ workspace, tab }) {
  const view = currentWorkspace(workspace);
  if (!view) return;
  const index = view.tabs.findIndex((t) => t.id === tab.id);
  if (index >= 0) view.tabs[index] = tab;
  else view.tabs.push(tab);
  applyTab(tab);
  if (tab.id === state.tab) syncTabState();
  invalidateAll();
}

function tabClosed({ workspace, tab }) {
  const view = currentWorkspace(workspace);
  if (!view) return;
  const index = view.tabs.findIndex((t) => t.id === tab);
  if (index < 0) return;
  view.tabs.splice(index, 1);
  if (state.tab === tab) {
    const next = view.tabs[Math.min(index, view.tabs.length - 1)] ?? null;
    selectTab(next?.id ?? null);
    tabSelectionLost(tab);
  }
  invalidateAll();
}

function tabsReordered({ workspace, tabs }) {
  const view = currentWorkspace(workspace);
  if (!view) return;
  const byId = new Map(view.tabs.map((t) => [t.id, t]));
  view.tabs = tabs.map((id) => byId.get(id)).filter(Boolean);
  invalidate("tabs");
}

function applyTab(tab) {
  for (const view of tab.snapshots ?? []) applyAgentView(view, tab.id);
}

function jobChanged(job) {
  state.jobs.set(job.id, job);
  invalidate("jobs", "sidebar", "tabs");
}

function modeChanged({ mode, tab, workspace }) {
  // A mode for every tab names its workspace; it may be another one.
  if (!tab && workspace && workspace !== state.workspace?.id) return;
  for (const t of state.workspace?.tabs ?? []) {
    if (!tab || t.id === tab) t.mode = mode;
  }
  syncTabState();
  invalidate("mode", "header", "home");
}

function onReloaded() {
  for (const name of resources.keys()) markStale(name);
  // The settings page reports its own saves.
  if (state.route.name !== "settings") toast("Settings applied", "success");
  document.dispatchEvent(new CustomEvent("malac:reloaded"));
  invalidate("resources");
}

let onTabLost = () => {};

export function setTabLostHandler(fn) {
  onTabLost = fn;
}

function tabSelectionLost(tab) {
  onTabLost(tab);
}

// Tabs

export function tabById(id) {
  return state.workspace?.tabs.find((t) => t.id === id) ?? null;
}

export function currentTab() {
  return tabById(state.tab);
}

export function tabOfAgent(id) {
  const tabs = state.workspace?.tabs ?? [];
  const direct = tabs.find((t) => t.agents.includes(id) || t.snapshots?.some((s) => s.id === id));
  if (direct) return direct;
  const agent = state.agents.get(id);
  if (agent?.tab) return tabById(agent.tab);
  for (let current = agent; current?.parent; current = state.agents.get(current.parent)) {
    const parentTab = tabs.find((t) => t.agents.includes(current.parent));
    if (parentTab) return parentTab;
  }
  return null;
}

/** Top-level agents of a tab, in order, followed by nothing else. */
export function tabAgents(tab) {
  return (tab?.agents ?? []).map((id) => state.agents.get(id)).filter(Boolean);
}

/** Running, waiting or done, for tab and workspace badges. */
export function tabStatus(tab) {
  const ids = new Set([...(tab?.agents ?? []), ...(tab?.snapshots ?? []).map((s) => s.id)]);
  const agents = [...state.agents.values()].filter((a) => ids.has(a.id) || (a.tab === tab?.id && a.live));
  const waiting = [...state.pending.values()].some((r) => r.tab === tab?.id || ids.has(r.agent));
  if (waiting) return "waiting";
  if (agents.some((a) => a.live && a.status === "running")) return "running";
  if (agents.some((a) => a.status === "failed")) return "failed";
  if (agents.some((a) => a.status === "completed")) return "completed";
  return "idle";
}

export function selectTab(id) {
  state.tab = id;
  syncTabState();
}

function syncTabState() {
  const tab = currentTab();
  state.cwd = tab?.cwd ?? "";
  state.mode = tab?.mode ?? state.mode;
}

// Saved sessions, for the history list.

export async function loadHistoryList() {
  try {
    const data = await api("/api/sessions?limit=100");
    state.sessions = data.sessions ?? [];
    for (const session of state.sessions) {
      session.title = cleanTitle(session.title);
      const agent = state.agents.get(session.id);
      if (agent && !agent.title && session.title) agent.title = session.title;
    }
    invalidate("sidebar", "home");
  } catch {
    // The list is a convenience; the snapshot has everything else.
  }
}

function scheduleHistoryRefresh() {
  clearTimeout(state.historyTimer);
  state.historyTimer = setTimeout(loadHistoryList, HISTORY_REFRESH_MS);
}

// Agents

export function ensureAgent(id) {
  let agent = state.agents.get(id);
  if (!agent) {
    agent = {
      id,
      label: "agent",
      title: cleanTitle(state.sessions.find((session) => session.id === id)?.title),
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
      todos: null,
      readOnly: false,
      live: false,
      events: [],
      history: null,
      running: new Map(),
      outgoing: [],
      lastEvent: null,
    };
    state.agents.set(id, agent);
  }
  return agent;
}

function applyAgentView(view, tab) {
  const agent = ensureAgent(view.id);
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
  if (cleanTitle(view.title)) agent.title = view.title;
}

export function applyMeta(agent, meta) {
  Object.assign(agent, {
    label: meta.label,
    parent: meta.parent,
    model: agent.model || meta.model,
    cwd: meta.cwd,
    created: agent.created ?? meta.created,
  });
  if (cleanTitle(meta.title) && !agent.title) agent.title = meta.title;
}

export function displayName(agent) {
  return (!agent.parent && agent.title) || agent.label;
}

/** A session title, unless it is attached content rather than typed text. */
export function cleanTitle(title) {
  const text = String(title ?? "");
  return text.startsWith("<attachment ") ? "" : text;
}

export function selectedAgent() {
  return state.selected ? state.agents.get(state.selected) : null;
}

/** Every agent of the shown workspace: the tabs' agents and their live subagents. */
export function workspaceAgents() {
  const tabs = state.workspace?.tabs ?? [];
  const ids = new Set(tabs.flatMap((t) => [...t.agents, ...(t.snapshots ?? []).map((s) => s.id)]));
  return [...state.agents.values()].filter((a) => ids.has(a.id) || (a.live && tabs.some((t) => t.id === a.tab)));
}

export function byCreated(a, b) {
  return String(a.created ?? "").localeCompare(String(b.created ?? ""));
}

export function waitingAgents() {
  return new Set([...state.pending.values()].map((request) => request.agent));
}

export function todoProgress(todos) {
  if (!todos?.length) return null;
  return { done: todos.filter((todo) => todo.status === "completed").length, total: todos.length };
}

export function lineage(agent) {
  const chain = [];
  for (let current = agent; current && chain.length < 32; current = state.agents.get(current.parent)) {
    chain.unshift(current);
  }
  return chain;
}

export function childrenOf(id) {
  return [...state.agents.values()].filter((agent) => agent.parent === id).sort(byCreated);
}

// Agent ids are UUIDv7, whose first 48 bits are the creation time in milliseconds.
function isNewAgent(id, at) {
  const created = parseInt(id.replace(/-/g, "").slice(0, 12), 16);
  return Math.abs(Date.parse(at) - created) < NEW_AGENT_WINDOW_MS;
}

function onEvent({ tab, envelope }) {
  if (envelope.type === "update") {
    state.update = envelope.status;
    invalidate("update");
    return;
  }
  const agent = ensureAgent(envelope.agent);
  if (tab) agent.tab = tab;
  const entry = { n: ++state.received, env: envelope };
  agent.lastEvent = envelope.at;
  if (envelope.type === "agent_started") agentStarted(agent, envelope, entry.n);
  if (envelope.type === "user_message") claimImages(agent, envelope);
  record(agent, entry);
  track(agent, envelope);
  trackPermission(agent, envelope, tab);
  for (const view of state.views) {
    if (view.agent === agent && view.ready) view.apply(envelope);
    view.refreshChild(agent.id);
  }
  invalidate("sidebar", "tabs", "header", "agents");
  if (agent.id === state.selected) invalidate("composer");
}

function agentStarted(agent, envelope, n) {
  Object.assign(agent, {
    label: envelope.label,
    parent: envelope.parent,
    model: envelope.model,
    cwd: envelope.cwd,
    created: envelope.at,
    status: "idle",
    live: true,
  });
  if (!agent.history && isNewAgent(agent.id, envelope.at)) agent.history = { messages: [], cutoff: 0 };
  if (!envelope.parent) return;
  const parent = ensureAgent(envelope.parent);
  agent.tab ??= parent.tab;
  const link = { type: "child_started", agent: parent.id, child: agent.id, at: envelope.at };
  record(parent, { n, env: link });
  for (const view of state.views) {
    if (view.agent === parent && view.ready) view.apply(link);
  }
}

// Consecutive deltas are merged so long streams stay cheap to replay.
function record(agent, entry) {
  const last = agent.events.at(-1);
  const type = entry.env.type;
  if (last && last.env.type === type && (type === "text_delta" || type === "thinking_delta")) {
    agent.events[agent.events.length - 1] = { n: entry.n, env: { ...last.env, text: last.env.text + entry.env.text } };
  } else {
    agent.events.push(entry);
  }
}

function track(agent, envelope) {
  switch (envelope.type) {
    case "user_message":
      agent.status = "running";
      agent.live = true;
      agent.running.clear();
      agent.turnStarted = envelope.at;
      if (!agent.title) agent.title = truncate(firstLine(envelope.text), 80);
      break;
    case "model_changed":
      agent.model = envelope.model;
      break;
    case "tool_started":
      agent.running.set(envelope.call_id, `${envelope.tool} ${envelope.summary}`.trim());
      break;
    case "tool_finished":
      agent.running.delete(envelope.call_id);
      break;
    case "usage":
      agent.usage = envelope.total;
      agent.contextTokens = envelope.context_tokens;
      break;
    case "todos":
      agent.todos = envelope.items;
      break;
    case "turn_finished":
      agent.status = envelope.status;
      agent.report = envelope.text;
      agent.running.clear();
      agent.turnStarted = null;
      scheduleHistoryRefresh();
      break;
    case "agent_finished":
      agent.status = envelope.status;
      agent.report = envelope.report;
      break;
  }
  agent.activity = nextActivity(agent, envelope);
}

function nextActivity(agent, envelope) {
  switch (envelope.type) {
    case "user_message":
    case "thinking_delta":
      return "thinking";
    case "text_delta":
      return "writing";
    case "tool_call_streaming":
      return `preparing ${envelope.tool}`;
    case "tool_started":
    case "tool_finished":
    case "permission_resolved":
      return [...agent.running.values()].at(-1) ?? "thinking";
    case "permission_requested":
      return `waiting for approval: ${envelope.tool}`;
    case "turn_finished":
      return "";
    default:
      return agent.activity;
  }
}

function trackPermission(agent, envelope, tab) {
  if (envelope.type === "permission_requested") {
    const { request_id: id, call_id, tool, summary, input } = envelope;
    state.pending.set(id, { id, call_id, agent: agent.id, agent_label: agent.label, tool, summary, input, tab: tab ?? agent.tab });
  } else if (envelope.type === "permission_resolved") {
    state.pending.delete(envelope.request_id);
  } else {
    return;
  }
  invalidate("permissions", "tabs", "title");
}

export function decide(requestId, decision) {
  return send({ type: "approve", request_id: requestId, decision });
}

// The server's user_message event carries only text, so images sent from
// this page are matched back to it by text.
function claimImages(agent, envelope) {
  const index = agent.outgoing.findIndex((item) => envelope.text.startsWith(item.text));
  if (index >= 0) envelope.images = agent.outgoing.splice(index, 1)[0].images;
}

export function queueImages(agent, text, images) {
  if (!images.length) return;
  const sent = agent.events.find((entry) => entry.env.type === "user_message" && entry.env.text.startsWith(text));
  if (sent) sent.env.images = images;
  else agent.outgoing.push({ text, images });
}

export async function loadHistory(agent) {
  const generation = state.generation;
  const cutoff = state.received;
  const data = await api(`/api/sessions/${encodeURIComponent(agent.id)}`);
  const saved = { messages: data.history ?? [], cutoff };
  if (generation === state.generation) {
    applyMeta(agent, data.meta);
    if (!agent.title) {
      // The first typed prompt names the conversation.
      const first = saved.messages.find((m) => m.role === "user" && m.content.some((b) => b.type === "text" && !b.text.startsWith("<")));
      const text = first?.content.find((b) => b.type === "text" && !b.text.startsWith("<"))?.text;
      if (text) agent.title = truncate(firstLine(text), 80);
    }
    if (!agent.live) agent.usage = data.usage;
    agent.history = saved;
    invalidate("sidebar", "header", "title", "tabs", "agents");
  }
  return saved;
}

// Agent commands

/**
 * Prompts `agent`, or starts a new top-level agent in `tab`. Resolves with
 * the ack (`agent`, `tab`, `attachments`).
 */
export async function submitPrompt({ text, images = [], attachments = [], agent: agentId = null, tab = null, model }) {
  const agent = agentId ? ensureAgent(agentId) : null;
  const message = { type: "prompt", text, images, attachments };
  if (agent) message.agent = agent.id;
  if (tab) message.tab = tab;
  if (!agent && model) message.model = model;
  if (agent && images.length) agent.outgoing.push({ text, images });
  const reply = await request(message);
  const target = ensureAgent(reply.agent);
  if (reply.tab) target.tab = reply.tab;
  if (!agent) {
    target.history ??= { messages: [], cutoff: 0 };
    queueImages(target, text, images);
  }
  return reply;
}

/** Shows a saved session: the daemon puts it in its tab (or one for its folder). */
export async function resumeSession(session, tab) {
  const live = state.agents.get(session.id);
  if (live?.live) return openAgent(session.id);
  try {
    const reply = await request({ type: "agent_open", agent: session.id, ...(tab ? { tab } : {}) });
    await waitForTab(reply.tab);
    const agent = ensureAgent(reply.agent);
    applyMeta(agent, session);
    agent.tab = reply.tab;
    navigate({ name: "tab", tab: reply.tab, agent: reply.agent });
  } catch (error) {
    toast(error.message, "error");
  }
}

export async function startAgent(tab, model) {
  const reply = await request({ type: "agent_start", tab, ...(model ? { model } : {}) });
  ensureAgent(reply.agent).tab = reply.tab;
  return reply;
}

/** Makes a top-level agent its tab's active one (subagents are not tab agents). */
export function activateAgent(tab, agent) {
  const view = tabById(tab);
  if (view && agent && view.agents.includes(agent) && view.active_agent !== agent) {
    view.active_agent = agent;
    send({ type: "agent_activate", tab, agent });
  }
}

export async function closeAgent(tab, agent) {
  await request({ type: "agent_close", tab, agent });
}

export function stopAgent(agent) {
  if (agent) send({ type: "cancel", agent: agent.id });
}

export function compactAgent(agent) {
  if (send({ type: "compact", agent: agent.id })) toast(`Compacting the conversation of ${agent.label}`);
}

export function changeModel(agent, model) {
  if (!model || model === agent.model) return;
  if (!send({ type: "model", agent: agent.id, model })) return;
  agent.model = model;
  invalidate("header", "agents");
}

export function setMode(mode) {
  if (!mode) return;
  state.mode = mode;
  invalidate("mode", "header", "home");
}

/** Changes the selected tab's mode (every tab when there is none). */
export function requestMode(mode) {
  if (mode === state.mode) return;
  const tab = currentTab();
  if (!send({ type: "mode", mode, ...(tab ? { tab: tab.id } : {}) })) return;
  if (tab) tab.mode = mode;
  setMode(mode);
}

export async function deleteSession(id) {
  await api(`/api/sessions/${encodeURIComponent(id)}`, { method: "DELETE" });
  state.sessions = state.sessions.filter((session) => session.id !== id);
  const agent = state.agents.get(id);
  if (agent && !agent.live) state.agents.delete(id);
  invalidate("sidebar", "home");
}

// Workspace and tab commands

export async function createWorkspace(name) {
  const reply = await request({ type: "workspace_create", ...(name ? { name } : {}) });
  return reply.workspace;
}

export async function openWorkspace(id) {
  const reply = await request({ type: "workspace_open", workspace: id });
  return reply.workspace;
}

export async function renameWorkspace(id, name) {
  await request({ type: "workspace_rename", workspace: id, name });
}

export async function deleteWorkspace(id, force = false) {
  await request({ type: "workspace_delete", workspace: id, force });
}

/** Opens a tab for `cwd`; with `reuse`, an existing tab for it is activated instead. */
export async function createTab(cwd, name, { reuse = false } = {}) {
  // A tab needs a workspace this connection has open: the latest one, or a new one.
  if (!state.workspace) {
    const latest = [...state.workspaces].sort((a, b) => String(b.updated).localeCompare(String(a.updated)))[0];
    if (latest) await openWorkspace(latest.id);
    else await createWorkspace("default");
  }
  const spec = { type: "tab_create", cwd };
  if (name) spec.name = name;
  if (reuse) spec.reuse = true;
  if (state.workspace) spec.workspace = state.workspace.id;
  const reply = await request(spec);
  await waitForTab(reply.tab);
  return reply.tab;
}

// The ack can arrive before the tab_created broadcast; wait for the tab
// so navigating to it right away works.
const tabWaiters = new Map();

function waitForTab(id, timeout = 3000) {
  if (!id || tabById(id)) return Promise.resolve();
  return new Promise((resolve) => {
    const done = () => {
      tabWaiters.delete(id);
      resolve();
    };
    tabWaiters.set(id, done);
    setTimeout(done, timeout);
  });
}

export async function closeTab(id, force = false) {
  await request({ type: "tab_close", tab: id, force });
}

export async function renameTab(id, name) {
  await request({ type: "tab_rename", tab: id, name });
}

export function moveTab(id, index) {
  const tabs = state.workspace?.tabs ?? [];
  const from = tabs.findIndex((t) => t.id === id);
  if (from < 0 || index < 0 || index >= tabs.length || index === from) return;
  // Reorder locally right away; tabs_reordered confirms it.
  const [tab] = tabs.splice(from, 1);
  tabs.splice(index, 0, tab);
  invalidate("tabs");
  send({ type: "tab_move", tab: id, index });
}

/** Tells the daemon which tab this page works in (settings and `prompt` defaults follow it). */
export function activateTab(id) {
  if (id && state.workspace?.active_tab !== id) send({ type: "tab_activate", tab: id });
}

// Jobs and the control service

export async function jobOutput(id, tail = 200) {
  const reply = await request({ type: "job_output", job: id, tail });
  return reply.output;
}

export async function killJob(id) {
  await request({ type: "job_kill", job: id });
}

export async function checkControl(requestPermissions = false) {
  const reply = await request({ type: "control_permissions", request: requestPermissions });
  state.control = reply.control;
  invalidate("home", "sidebar");
  return reply.control;
}

// Self-update. Installing replaces the binaries on disk; the daemon keeps
// running the old version until it is restarted.

export async function checkForUpdate() {
  state.update = await api("/api/update/check", { method: "POST" });
  invalidate("update");
  return state.update;
}

export async function installUpdate() {
  state.update = await api("/api/update/install", { method: "POST" });
  invalidate("update");
  return state.update;
}

// Cached API resources: fetched on demand, refreshed after settings change.

const RESOURCE_PATHS = { settings: "/api/settings", models: "/api/models", tools: "/api/tools", memory: "/api/memory", daemon: "/api/daemon" };
const resources = new Map();

export function resource(name) {
  return resources.get(name) ?? { data: null, error: null, loading: false };
}

export function ensure(name) {
  const entry = resource(name);
  if (!entry.loading && (entry.stale || (!entry.data && !entry.error))) refresh(name);
  return entry;
}

export function refresh(name) {
  const previous = resource(name);
  const entry = { ...previous, loading: true, stale: false };
  resources.set(name, entry);
  invalidate("resources");
  entry.promise = api(RESOURCE_PATHS[name])
    .then((data) => {
      resources.set(name, { data, error: null, loading: false, at: Date.now() });
      return data;
    })
    .catch((error) => {
      resources.set(name, { data: previous.data, error: error.message, loading: false, at: Date.now() });
      return null;
    })
    .finally(() => invalidate("resources"));
  return entry.promise;
}

function markStale(name) {
  const entry = resources.get(name);
  if (entry) entry.stale = true;
}

export function markAllStale() {
  for (const name of resources.keys()) markStale(name);
}

/** Context window for a `provider/model`, from the effective configuration. */
export function contextWindow(spec) {
  const models = resource("settings").data?.effective?.models ?? {};
  return models[spec]?.context_window ?? DEFAULT_CONTEXT_WINDOW;
}

/**
 * Models to offer in pickers, grouped by provider: the configured ones
 * first, then what each provider reports as available.
 */
export function modelGroups() {
  const groups = new Map();
  const group = (provider) => {
    if (!groups.has(provider)) groups.set(provider, { provider, models: new Map(), hasKey: null, error: null, kind: "" });
    return groups.get(provider);
  };
  for (const spec of state.models) {
    const [provider, ...rest] = spec.split("/");
    group(provider).models.set(spec, { spec, name: rest.join("/"), configured: true });
  }
  const discovered = resource("models").data?.providers ?? [];
  for (const entry of discovered) {
    const target = group(entry.provider);
    Object.assign(target, { hasKey: entry.has_key, error: entry.error, kind: entry.kind });
    for (const name of entry.configured ?? []) {
      const spec = `${entry.provider}/${name}`;
      if (!target.models.has(spec)) target.models.set(spec, { spec, name, configured: true });
    }
    for (const name of entry.available ?? []) {
      const spec = `${entry.provider}/${name}`;
      if (!target.models.has(spec)) target.models.set(spec, { spec, name, configured: false });
    }
  }
  return [...groups.values()]
    .map((g) => ({ ...g, models: [...g.models.values()] }))
    .filter((g) => g.models.length > 0 || g.error == null);
}

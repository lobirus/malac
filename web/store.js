// Client state: the server connection, live agents built from the event
// stream, cached API resources and a per-frame render scheduler.

import { emptyUsage, firstLine, sessionStore, toast, truncate } from "./dom.js";

const TOKEN_KEY = "malac.token";
const RECONNECT_MIN_MS = 500;
const RECONNECT_MAX_MS = 10_000;
const NEW_AGENT_WINDOW_MS = 5_000;
const STATE_REFRESH_MS = 800;
export const DEFAULT_CONTEXT_WINDOW = 200_000;
export const STATUS_TEXT = { idle: "Idle", running: "Running", completed: "Done", failed: "Failed", cancelled: "Stopped" };

export const params = new URLSearchParams(location.search);

export const state = {
  token: null,
  version: "",
  cwd: "",
  mode: "ask",
  models: [],
  defaultModel: null,
  servers: [],
  sessions: [],
  web: { remote: false, reloadable: false },
  update: null,
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
  refreshTimer: 0,
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
  if (!token) return sessionStore.get(TOKEN_KEY);
  sessionStore.set(TOKEN_KEY, token);
  params.delete("token");
  const query = params.toString();
  history.replaceState(null, "", `${location.pathname}${query ? `?${query}` : ""}${location.hash}`);
  return token;
}

/** Calls the API; a 400 `{error}` body becomes the thrown message. */
export async function api(path, { method = "GET", body } = {}) {
  const headers = { Authorization: `Bearer ${state.token}` };
  if (body !== undefined) headers["Content-Type"] = "application/json";
  const response = await fetch(path, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
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

// Navigation is owned by the app shell; other modules go through here.

let navigator = () => {};

export function setNavigator(fn) {
  navigator = fn;
}

export function navigate(route) {
  navigator(route);
}

export function openAgent(id) {
  navigate(id ? { name: "agent", id } : { name: "home" });
}

/** Opens an agent that may not be known yet (a finished subagent, say). */
export function viewAgent(id, defaults) {
  if (!state.agents.has(id)) Object.assign(ensureAgent(id), defaults, { live: false });
  openAgent(id);
}

// WebSocket

export function connect() {
  const scheme = location.protocol === "https:" ? "wss" : "ws";
  const socket = new WebSocket(`${scheme}://${location.host}/api/ws?token=${encodeURIComponent(state.token)}`);
  state.socket = socket;
  setConnection("connecting");
  socket.addEventListener("open", () => {
    state.backoff = RECONNECT_MIN_MS;
    setConnection("online");
    resync();
  });
  socket.addEventListener("message", (message) => receive(JSON.parse(message.data)));
  socket.addEventListener("close", () => {
    state.socket = null;
    state.callbacks.clear();
    setConnection("offline");
    setTimeout(connect, state.backoff);
    state.backoff = Math.min(state.backoff * 2, RECONNECT_MAX_MS);
  });
}

function setConnection(status) {
  state.connection = status;
  invalidate("connection");
}

export function send(message, onAck) {
  if (state.socket?.readyState !== WebSocket.OPEN) {
    toast("Not connected to malac", "error");
    return false;
  }
  if (onAck) {
    message.client_ref = `r${state.nextRef++}`;
    state.callbacks.set(message.client_ref, onAck);
  }
  state.socket.send(JSON.stringify(message));
  return true;
}

function receive(message) {
  switch (message.type) {
    case "event":
      return onEnvelope(message.envelope);
    case "ack":
      return onAck(message);
    case "mode":
      return setMode(message.mode);
    case "reloaded":
      return onReloaded();
    case "error":
      return toast(message.message, "error");
    case "lagged":
      toast(`Missed ${message.skipped} events, reloading`, "warning");
      return resync();
  }
}

function onAck({ client_ref: ref, agent }) {
  const callback = state.callbacks.get(ref);
  state.callbacks.delete(ref);
  callback?.(agent);
}

export function setMode(mode) {
  if (!mode) return;
  state.mode = mode;
  invalidate("mode", "header", "home");
}

export function requestMode(mode) {
  if (mode !== state.mode && send({ type: "mode", mode })) setMode(mode);
}

function onReloaded() {
  for (const name of resources.keys()) markStale(name);
  // The settings page reports its own saves.
  if (state.route.name !== "settings") toast("Settings applied", "success");
  document.dispatchEvent(new CustomEvent("malac:reloaded"));
  loadState().catch((error) => toast(error.message, "error"));
  invalidate("resources");
}

export async function loadState() {
  const data = await api("/api/state");
  Object.assign(state, {
    version: data.version,
    cwd: data.cwd,
    models: data.models ?? [],
    defaultModel: data.default_model,
    servers: data.servers ?? [],
    sessions: data.sessions ?? [],
    web: data.web ?? state.web,
    update: data.update ?? state.update,
    loaded: true,
  });
  invalidate("update");
  setMode(data.mode);
  const live = new Set(data.agents.map((snapshot) => snapshot.id));
  for (const agent of state.agents.values()) {
    if (!live.has(agent.id)) agent.live = false;
  }
  data.agents.forEach(applySnapshot);
  for (const session of state.sessions) {
    const agent = state.agents.get(session.id);
    if (agent && !agent.title) agent.title = session.title;
  }
  state.pending = new Map(data.pending.map((request) => [request.id, request]));
  invalidate("sidebar", "header", "permissions", "composer", "home", "agents");
}

export function scheduleStateRefresh() {
  clearTimeout(state.refreshTimer);
  state.refreshTimer = setTimeout(() => loadState().catch(() => {}), STATE_REFRESH_MS);
}

let onResync = () => {};

export function setResyncHandler(fn) {
  onResync = fn;
}

// Every buffered event and history is dropped and rebuilt from the server.
export async function resync() {
  state.generation += 1;
  for (const agent of state.agents.values()) {
    agent.events = [];
    agent.history = null;
  }
  try {
    await loadState();
  } catch (error) {
    toast(error.message, "error");
    return;
  }
  onResync();
}

// Agents

export function ensureAgent(id) {
  let agent = state.agents.get(id);
  if (!agent) {
    agent = {
      id,
      label: "agent",
      title: state.sessions.find((session) => session.id === id)?.title ?? "",
      parent: null,
      model: "",
      cwd: "",
      created: null,
      status: "idle",
      activity: "",
      usage: emptyUsage(),
      contextTokens: 0,
      report: null,
      todos: null,
      live: true,
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

function applySnapshot(snapshot) {
  const agent = ensureAgent(snapshot.id);
  Object.assign(agent, {
    label: snapshot.label,
    parent: snapshot.parent,
    model: snapshot.model,
    cwd: snapshot.cwd,
    created: snapshot.created,
    status: snapshot.status,
    activity: snapshot.activity,
    usage: snapshot.usage,
    report: snapshot.report,
    live: true,
  });
}

export function applyMeta(agent, meta) {
  Object.assign(agent, {
    label: meta.label,
    parent: meta.parent,
    model: agent.model || meta.model,
    cwd: meta.cwd,
    created: agent.created ?? meta.created,
  });
  if (meta.title && !agent.title) agent.title = meta.title;
}

export function displayName(agent) {
  return (!agent.parent && agent.title) || agent.label;
}

export function selectedAgent() {
  return state.selected ? state.agents.get(state.selected) : null;
}

export function liveAgents() {
  return [...state.agents.values()].filter((agent) => agent.live);
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

/** Every live agent below `id`, depth first. */
export function descendants(id) {
  return childrenOf(id).flatMap((child) => [child, ...descendants(child.id)]);
}

// Agent ids are UUIDv7, whose first 48 bits are the creation time in milliseconds.
function isNewAgent(id, at) {
  const created = parseInt(id.replace(/-/g, "").slice(0, 12), 16);
  return Math.abs(Date.parse(at) - created) < NEW_AGENT_WINDOW_MS;
}

function onEnvelope(envelope) {
  if (envelope.type === "update") {
    state.update = envelope.status;
    invalidate("update");
    return;
  }
  const agent = ensureAgent(envelope.agent);
  const entry = { n: ++state.received, env: envelope };
  agent.lastEvent = envelope.at;
  if (envelope.type === "agent_started") agentStarted(agent, envelope, entry.n);
  if (envelope.type === "user_message") claimImages(agent, envelope);
  record(agent, entry);
  track(agent, envelope);
  trackPermission(agent, envelope);
  for (const view of state.views) {
    if (view.agent === agent && view.ready) view.apply(envelope);
    view.refreshChild(agent.id);
  }
  invalidate("sidebar", "header", "agents");
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
      scheduleStateRefresh();
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

function trackPermission(agent, envelope) {
  if (envelope.type === "permission_requested") {
    const { request_id: id, call_id, tool, summary, input } = envelope;
    state.pending.set(id, { id, call_id, agent: agent.id, agent_label: agent.label, tool, summary, input });
  } else if (envelope.type === "permission_resolved") {
    state.pending.delete(envelope.request_id);
  } else {
    return;
  }
  invalidate("permissions");
}

export function decide(requestId, decision) {
  return send({ type: "approve", request_id: requestId, decision });
}

// The server's user_message event carries only text, so images sent from
// this page are matched back to it by text.
function claimImages(agent, envelope) {
  const index = agent.outgoing.findIndex((item) => item.text === envelope.text);
  if (index >= 0) envelope.images = agent.outgoing.splice(index, 1)[0].images;
}

export function queueImages(agent, text, images) {
  if (!images.length) return;
  const sent = agent.events.find((entry) => entry.env.type === "user_message" && entry.env.text === text);
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
    if (!agent.live) agent.usage = data.usage;
    agent.history = saved;
    invalidate("sidebar", "header");
  }
  return saved;
}

// Commands

export function submitPrompt(text, images, agentId, model) {
  const agent = agentId ? state.agents.get(agentId) : null;
  if (!agent) return startSession(text, images, model);
  return promptAgent(agent, text, images);
}

function startSession(text, images, model) {
  return send({ type: "prompt", agent: null, text, images, model: model || undefined }, (id) => {
    const agent = ensureAgent(id);
    agent.history ??= { messages: [], cutoff: 0 };
    queueImages(agent, text, images);
    openAgent(id);
  });
}

// Prompting a saved session resumes it on the server.
function promptAgent(agent, text, images) {
  const sent = send({ type: "prompt", agent: agent.id, text, images }, (id) => {
    const resumed = ensureAgent(id);
    if (!resumed.live) {
      resumed.live = true;
      invalidate("sidebar", "header", "composer");
    }
  });
  if (sent && images.length) agent.outgoing.push({ text, images });
  return sent;
}

export function resumeSession(session) {
  if (state.agents.get(session.id)?.live) {
    openAgent(session.id);
    return;
  }
  const agent = ensureAgent(session.id);
  applyMeta(agent, session);
  agent.live = false;
  openAgent(session.id);
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

export async function deleteSession(id) {
  await api(`/api/sessions/${encodeURIComponent(id)}`, { method: "DELETE" });
  state.sessions = state.sessions.filter((session) => session.id !== id);
  const agent = state.agents.get(id);
  if (agent && !agent.live) state.agents.delete(id);
  invalidate("sidebar", "home");
}

// Self-update. Installing replaces the binaries on disk; this server keeps
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

const RESOURCE_PATHS = { settings: "/api/settings", models: "/api/models", tools: "/api/tools", memory: "/api/memory" };
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

const TOKEN_KEY = "malac.token";
const IMAGE_TYPES = new Set(["image/png", "image/jpeg", "image/gif", "image/webp"]);
const MAX_IMAGE_BYTES = 8 * 1024 * 1024;
const RECONNECT_MIN_MS = 500;
const RECONNECT_MAX_MS = 10_000;
const NEW_AGENT_WINDOW_MS = 5_000;
const STATE_REFRESH_MS = 800;
const PIN_THRESHOLD_PX = 48;
const TOAST_MS = 6_000;
const SUMMARY_KEYS = ["command", "path", "pattern", "url", "query", "name", "id", "message"];
const STATUS_TEXT = { idle: "idle", running: "running", completed: "done", failed: "failed", cancelled: "stopped" };

const params = new URLSearchParams(location.search);
const narrowScreen = window.matchMedia("(max-width: 760px)");

const state = {
  token: null,
  version: "",
  cwd: "",
  mode: "ask",
  models: [],
  defaultModel: null,
  servers: [],
  sessions: [],
  agents: new Map(),
  pending: new Map(),
  selected: null,
  view: null,
  received: 0,
  generation: 0,
  socket: null,
  backoff: RECONNECT_MIN_MS,
  callbacks: new Map(),
  nextRef: 1,
  collapsed: new Set(),
  pinned: true,
  images: [],
  inbox: [],
  dirty: new Set(),
  frame: 0,
  followFrame: 0,
  refreshTimer: 0,
};

const $ = (id) => document.getElementById(id);
const els = {
  app: $("app"),
  backdrop: $("backdrop"),
  toggleSidebar: $("toggle-sidebar"),
  connectionDot: $("connection-dot"),
  version: $("version"),
  cwd: $("cwd"),
  newSession: $("new-session"),
  mode: $("mode"),
  tree: $("agent-tree"),
  agentsEmpty: $("agents-empty"),
  sessions: $("sessions"),
  sessionsEmpty: $("sessions-empty"),
  servers: $("servers"),
  breadcrumb: $("breadcrumb"),
  actions: $("actions"),
  status: $("status"),
  offline: $("offline"),
  permissions: $("permissions"),
  scroller: $("scroller"),
  transcript: $("transcript"),
  composer: $("composer"),
  attachments: $("attachments"),
  input: $("input"),
  model: $("model"),
  toasts: $("toasts"),
};

// Helpers

function h(tag, props, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(props ?? {})) {
    if (value == null || value === false) continue;
    if (key.startsWith("on")) node.addEventListener(key.slice(2), value);
    else if (key === "class") node.className = value;
    else node.setAttribute(key, value === true ? "" : String(value));
  }
  return appendChildren(node, children);
}

function appendChildren(node, children) {
  for (const child of children.flat()) {
    if (child != null && child !== false) node.append(child);
  }
  return node;
}

function setChildren(node, ...children) {
  node.replaceChildren();
  return appendChildren(node, children);
}

function shortPath(path) {
  const parts = String(path ?? "").split("/").filter(Boolean);
  return parts.length > 2 ? `…/${parts.slice(-2).join("/")}` : path;
}

const HTML_ESCAPES = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" };

function escapeHtml(text) {
  return String(text).replace(/[&<>"']/g, (c) => HTML_ESCAPES[c]);
}

function truncate(text, max) {
  return text.length > max ? `${text.slice(0, max - 1)}…` : text;
}

function firstLine(text) {
  return String(text ?? "").trim().split("\n", 1)[0];
}

function trimZero(number) {
  return number.replace(/\.0$/, "");
}

function formatTokens(n) {
  if (n < 1000) return String(n);
  if (n < 1_000_000) return `${trimZero((n / 1000).toFixed(1))}k`;
  return `${trimZero((n / 1_000_000).toFixed(1))}M`;
}

function formatCost(cost) {
  if (!cost) return "";
  return `$${cost < 0.01 ? cost.toFixed(4) : cost.toFixed(2)}`;
}

function formatDuration(ms) {
  if (ms < 1000) return `${ms}ms`;
  if (ms < 60_000) return `${trimZero((ms / 1000).toFixed(1))}s`;
  const seconds = Math.round(ms / 1000);
  return `${Math.floor(seconds / 60)}m ${String(seconds % 60).padStart(2, "0")}s`;
}

function relativeTime(iso) {
  const seconds = (Date.now() - Date.parse(iso)) / 1000;
  if (seconds < 60) return "now";
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`;
  if (seconds < 86_400) return `${Math.floor(seconds / 3600)}h`;
  if (seconds < 604_800) return `${Math.floor(seconds / 86_400)}d`;
  return new Date(iso).toLocaleDateString();
}

function emptyUsage() {
  return { input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_write_tokens: 0, cost: 0 };
}

function addUsage(total, usage) {
  for (const key of Object.keys(total)) total[key] += usage?.[key] ?? 0;
}

function usageText(usage, contextTokens) {
  if (!usage) return "";
  const parts = [`${formatTokens(usage.input_tokens)} in`, `${formatTokens(usage.output_tokens)} out`];
  if (usage.cache_read_tokens) parts.push(`${formatTokens(usage.cache_read_tokens)} cached`);
  if (usage.cost) parts.push(formatCost(usage.cost));
  if (contextTokens) parts.push(`${formatTokens(contextTokens)} context`);
  return parts.join(" · ");
}

function prettyJson(value) {
  return JSON.stringify(value ?? null, null, 2);
}

function imageNode(block) {
  if (!IMAGE_TYPES.has(block.media_type)) return null;
  return h("img", { class: "attachment", alt: "Attached image", src: `data:${block.media_type};base64,${block.data}` });
}

function resultText(content) {
  return (content ?? [])
    .map((block) => (block.type === "text" ? block.text : block.type === "image" ? `[${block.media_type} image]` : ""))
    .join("");
}

function summarizeInput(name, input) {
  if (!input || typeof input !== "object") return "";
  if (name === "agent_spawn") return `${input.label || input.agent || "agent"}: ${firstLine(input.task)}`;
  if (name === "todo") return `${input.items?.length ?? 0} items`;
  const key = SUMMARY_KEYS.find((k) => typeof input[k] === "string");
  return truncate(key ? firstLine(input[key]) : JSON.stringify(input), 160);
}

function parseAgentId(output) {
  return /\(id ([0-9a-f-]{36})\)/.exec(output ?? "")?.[1] ?? null;
}

function reportFrom(output) {
  const header = /^Report from `[^`]*` \(id [0-9a-f-]+\):\s*/.exec(output ?? "");
  return header ? output.slice(header[0].length) : "";
}

// Markdown: the source is escaped first, then only known tags are added.

const FENCE = /^ {0,3}(`{3,}|~{3,})\s*([^`\s]*)/;
const HEADING = /^ {0,3}(#{1,6})\s+(.*?)(?:\s+#+)?\s*$/;
const RULE = /^ {0,3}([-*_])(?:\s*\1){2,}\s*$/;
const QUOTE = /^ {0,3}> ?/;
const LIST_ITEM = /^( *)([-*+]|\d{1,9}[.)])\s+(.*)$/;
const TABLE_DIVIDER = /^\s*\|?\s*:?-+:?\s*(?:\|\s*:?-+:?\s*)*\|?\s*$/;
const CODE_SPAN = /(?<!`)(`+)(?!`)([\s\S]*?[^`])\1(?!`)/g;
const LINK = /!?\[([^\]]+)\]\(((?:[^()\s\u0000]|\([^()\s\u0000]*\))+)(?:\s+"[^"]*")?\)/g;
const BARE_URL = /(^|[\s(])(https?:\/\/[^\s<>"'`\u0000]+)/g;
const BLOCK_PARSERS = [fencedCode, heading, rule, blockquote, table, list, paragraph];

function renderMarkdown(source) {
  const lines = String(source).replace(/\r\n?/g, "\n").replace(/\u0000/g, "").split("\n");
  return renderBlocks(lines);
}

function markdownNode(source, className = "") {
  const node = h("div", { class: `markdown ${className}`.trim() });
  node.innerHTML = renderMarkdown(source);
  return node;
}

function renderBlocks(lines) {
  const out = [];
  let i = 0;
  while (i < lines.length) {
    if (isBlank(lines[i])) {
      i += 1;
      continue;
    }
    for (const parse of BLOCK_PARSERS) {
      const block = parse(lines, i);
      if (block) {
        out.push(block.html);
        i = block.next;
        break;
      }
    }
  }
  return out.join("");
}

function isBlank(line) {
  return line.trim() === "";
}

function indentOf(line) {
  return line.length - line.trimStart().length;
}

function startsBlock(line) {
  return FENCE.test(line) || HEADING.test(line) || RULE.test(line) || QUOTE.test(line) || LIST_ITEM.test(line);
}

function isTableStart(lines, i) {
  return lines[i].includes("|") && (lines[i + 1] ?? "").includes("|") && TABLE_DIVIDER.test(lines[i + 1]);
}

function fencedCode(lines, start) {
  const open = FENCE.exec(lines[start]);
  if (!open) return null;
  const close = new RegExp(`^ {0,3}${open[1][0] === "`" ? "`" : "~"}{${open[1].length},}\\s*$`);
  const body = [];
  let i = start + 1;
  while (i < lines.length && !close.test(lines[i])) body.push(lines[i++]);
  const lang = open[2].replace(/[^\w+#.-]/g, "");
  const attr = lang ? ` class="language-${lang}"` : "";
  return { html: `<pre><code${attr}>${escapeHtml(body.join("\n"))}</code></pre>`, next: i + 1 };
}

function heading(lines, i) {
  const match = HEADING.exec(lines[i]);
  if (!match) return null;
  const level = match[1].length;
  return { html: `<h${level}>${inline(match[2])}</h${level}>`, next: i + 1 };
}

function rule(lines, i) {
  return RULE.test(lines[i]) ? { html: "<hr>", next: i + 1 } : null;
}

function blockquote(lines, start) {
  if (!QUOTE.test(lines[start])) return null;
  const body = [];
  let i = start;
  while (i < lines.length && QUOTE.test(lines[i])) body.push(lines[i++].replace(QUOTE, ""));
  return { html: `<blockquote>${renderBlocks(body)}</blockquote>`, next: i };
}

function table(lines, start) {
  if (!isTableStart(lines, start)) return null;
  const head = splitRow(lines[start]);
  const aligns = splitRow(lines[start + 1]).map(alignment);
  const rows = [];
  let i = start + 2;
  while (i < lines.length && lines[i].includes("|") && !isBlank(lines[i])) rows.push(splitRow(lines[i++]));
  const cell = (tag, text, column) => {
    const style = aligns[column] ? ` style="text-align:${aligns[column]}"` : "";
    return `<${tag}${style}>${inline(text ?? "")}</${tag}>`;
  };
  const headHtml = head.map((text, c) => cell("th", text, c)).join("");
  const bodyHtml = rows.map((row) => `<tr>${head.map((_, c) => cell("td", row[c], c)).join("")}</tr>`).join("");
  return {
    html: `<div class="table"><table><thead><tr>${headHtml}</tr></thead><tbody>${bodyHtml}</tbody></table></div>`,
    next: i,
  };
}

function splitRow(line) {
  let row = line.trim();
  if (row.startsWith("|")) row = row.slice(1);
  if (row.endsWith("|") && !row.endsWith("\\|")) row = row.slice(0, -1);
  return row.split(/(?<!\\)\|/).map((cell) => cell.trim().replace(/\\\|/g, "|"));
}

function alignment(divider) {
  const left = divider.startsWith(":");
  const right = divider.endsWith(":");
  if (left && right) return "center";
  if (right) return "right";
  return left ? "left" : null;
}

function list(lines, start) {
  const first = LIST_ITEM.exec(lines[start]);
  if (!first) return null;
  const indent = first[1].length;
  const ordered = /\d/.test(first[2]);
  const sameList = (match) => match && match[1].length === indent && /\d/.test(match[2]) === ordered;
  const items = [];
  let loose = false;
  let i = start;
  while (i < lines.length) {
    const line = lines[i];
    const match = LIST_ITEM.exec(line);
    if (sameList(match)) {
      items.push({ lines: [match[3]], contentIndent: line.length - match[3].length });
      i += 1;
      continue;
    }
    if (match && indentOf(line) <= indent) break;
    const item = items[items.length - 1];
    if (isBlank(line)) {
      const next = lines[i + 1];
      if (next === undefined || (indentOf(next) <= indent && !sameList(LIST_ITEM.exec(next)))) break;
      item.lines.push("");
      loose = true;
    } else if (indentOf(line) > indent) {
      item.lines.push(line.slice(Math.min(indentOf(line), item.contentIndent)));
    } else if (startsBlock(line)) {
      break;
    } else {
      item.lines.push(line.trim());
    }
    i += 1;
  }
  const tag = ordered ? "ol" : "ul";
  const number = parseInt(first[2], 10);
  const startAttr = ordered && number !== 1 ? ` start="${number}"` : "";
  const body = items.map((item) => listItem(item, loose)).join("");
  return { html: `<${tag}${startAttr}>${body}</${tag}>`, next: i };
}

function listItem(item, loose) {
  const html = renderBlocks(item.lines);
  return `<li>${loose ? html : html.replace(/^<p>([\s\S]*?)<\/p>/, "$1")}</li>`;
}

function paragraph(lines, start) {
  const body = [lines[start].trim()];
  let i = start + 1;
  while (i < lines.length && !isBlank(lines[i]) && !startsBlock(lines[i]) && !isTableStart(lines, i)) {
    body.push(lines[i++].trim());
  }
  return { html: `<p>${inline(body.join("\n")).replace(/\n/g, "<br>")}</p>`, next: i };
}

// Code spans and links are swapped for placeholders before the rest of the
// text is escaped, so their contents are never parsed as emphasis.
function inline(text) {
  const stash = [];
  const keep = (html) => `\u0000${stash.push(html) - 1}\u0000`;
  let out = text.replace(CODE_SPAN, (_, _ticks, code) => keep(`<code>${escapeHtml(code.trim())}</code>`));
  out = out.replace(LINK, (_, label, href) => (isSafeHref(href) ? keep(anchor(href, emphasis(escapeHtml(label)))) : label));
  out = out.replace(BARE_URL, (_, before, raw) => {
    const url = raw.replace(/[.,:;!?)\]]+$/, "");
    return before + keep(anchor(url, escapeHtml(url))) + raw.slice(url.length);
  });
  const restore = (html) => html.replace(/\u0000(\d+)\u0000/g, (_, n) => restore(stash[Number(n)]));
  return restore(emphasis(escapeHtml(out)));
}

function anchor(href, html) {
  return `<a href="${escapeHtml(href)}" target="_blank" rel="noopener noreferrer">${html}</a>`;
}

function isSafeHref(href) {
  return /^(https?:\/\/|mailto:)/i.test(href);
}

function emphasis(html) {
  return html
    .replace(/\*\*(?=\S)([\s\S]*?\S)\*\*/g, "<strong>$1</strong>")
    .replace(/(^|[^\w])__(?=\S)([\s\S]*?\S)__(?!\w)/g, "$1<strong>$2</strong>")
    .replace(/(^|[^*\w])\*(?=[^\s*])([^*]*?[^\s*])\*(?!\*)/g, "$1<em>$2</em>")
    .replace(/(^|[^\w])_(?=[^\s_])([^_]*?[^\s_])_(?!\w)/g, "$1<em>$2</em>")
    .replace(/~~(?=\S)([\s\S]*?\S)~~/g, "<del>$1</del>");
}

// Server access

class HttpError extends Error {
  constructor(status, message) {
    super(message);
    this.status = status;
  }
}

const storage = {
  get(key) {
    try {
      return sessionStorage.getItem(key);
    } catch {
      return null;
    }
  },
  set(key, value) {
    try {
      sessionStorage.setItem(key, value);
    } catch {
      // Storage is unavailable in some sandboxed frames; the token then lives only in memory.
    }
  },
};

function readToken() {
  const token = params.get("token");
  if (!token) return storage.get(TOKEN_KEY);
  storage.set(TOKEN_KEY, token);
  params.delete("token");
  const query = params.toString();
  history.replaceState(null, "", `${location.pathname}${query ? `?${query}` : ""}${location.hash}`);
  return token;
}

async function api(path) {
  const response = await fetch(path, { headers: { Authorization: `Bearer ${state.token}` } });
  if (!response.ok) throw new HttpError(response.status, `${path}: ${response.status} ${await response.text()}`);
  return response.json();
}

function connect() {
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
  els.connectionDot.className = `connection-dot ${status}`;
  els.connectionDot.title = { online: "Connected", connecting: "Connecting", offline: "Disconnected" }[status];
  els.offline.hidden = status !== "offline";
}

function send(message, onAck) {
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

async function loadState() {
  const data = await api("/api/state");
  Object.assign(state, {
    version: data.version,
    cwd: data.cwd,
    models: data.models ?? [],
    defaultModel: data.default_model,
    servers: data.servers ?? [],
    sessions: data.sessions ?? [],
  });
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
  fillModels(els.model, els.model.value || state.defaultModel);
  els.version.textContent = state.version ? `v${state.version}` : "";
  els.cwd.textContent = shortPath(state.cwd);
  els.cwd.title = state.cwd;
  invalidate("sidebar", "header", "permissions", "composer");
}

function scheduleStateRefresh() {
  clearTimeout(state.refreshTimer);
  state.refreshTimer = setTimeout(() => loadState().catch(() => {}), STATE_REFRESH_MS);
}

// Every buffered event and history is dropped and rebuilt from the server.
async function resync() {
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
  openAgent(state.selected);
  deliverInbox();
}

// Agents

function ensureAgent(id) {
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

function applyMeta(agent, meta) {
  Object.assign(agent, {
    label: meta.label,
    parent: meta.parent,
    model: agent.model || meta.model,
    cwd: meta.cwd,
    created: agent.created ?? meta.created,
  });
  if (meta.title && !agent.title) agent.title = meta.title;
}

function displayName(agent) {
  return (!agent.parent && agent.title) || agent.label;
}

function selectedAgent() {
  return state.selected ? state.agents.get(state.selected) : null;
}

// Agent ids are UUIDv7, whose first 48 bits are the creation time in milliseconds.
function isNewAgent(id, at) {
  const created = parseInt(id.replace(/-/g, "").slice(0, 12), 16);
  return Math.abs(Date.parse(at) - created) < NEW_AGENT_WINDOW_MS;
}

function onEnvelope(envelope) {
  const agent = ensureAgent(envelope.agent);
  const entry = { n: ++state.received, env: envelope };
  if (envelope.type === "agent_started") agentStarted(agent, envelope, entry.n);
  if (envelope.type === "user_message") claimImages(agent, envelope);
  record(agent, entry);
  track(agent, envelope);
  trackPermission(agent, envelope);
  const view = state.view;
  if (view?.agent === agent && view.ready) view.apply(envelope);
  view?.refreshChild(agent.id);
  invalidate("sidebar", "header");
  if (envelope.type === "agent_started" && agent.id === state.selected) invalidate("composer");
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
  const link = { type: "child_started", agent: parent.id, child: agent.id };
  record(parent, { n, env: link });
  if (state.view?.agent === parent && state.view.ready) state.view.apply(link);
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
      if (!agent.title) agent.title = truncate(firstLine(envelope.text), 80);
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
      scheduleStateRefresh();
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
    const { request_id: id, tool, summary, input } = envelope;
    state.pending.set(id, { id, agent: agent.id, agent_label: agent.label, tool, summary, input });
  } else if (envelope.type === "permission_resolved") {
    state.pending.delete(envelope.request_id);
  } else {
    return;
  }
  invalidate("permissions");
}

// The server's user_message event carries only text, so images sent from
// this page are matched back to it by text.
function claimImages(agent, envelope) {
  const index = agent.outgoing.findIndex((item) => item.text === envelope.text);
  if (index >= 0) envelope.images = agent.outgoing.splice(index, 1)[0].images;
}

function queueImages(agent, text, images) {
  if (!images.length) return;
  const sent = agent.events.find((entry) => entry.env.type === "user_message" && entry.env.text === text);
  if (sent) sent.env.images = images;
  else agent.outgoing.push({ text, images });
}

async function loadHistory(agent) {
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

// Rendering outside the transcript is batched per animation frame and each
// region is redrawn only when its content key changes.

const renderers = {
  sidebar: renderSidebar,
  header: renderHeader,
  permissions: renderPermissions,
  composer: renderComposer,
};
const renderedKeys = new Map();

function invalidate(...regions) {
  for (const region of regions) state.dirty.add(region);
  state.frame ||= requestAnimationFrame(flush);
}

function flush() {
  state.frame = 0;
  const regions = [...state.dirty];
  state.dirty.clear();
  for (const region of regions) renderers[region]();
}

function renderKeyed(slot, key, render) {
  if (renderedKeys.get(slot) === key) return;
  renderedKeys.set(slot, key);
  render();
}

function renderSidebar() {
  renderModeButtons();
  renderAgentTree();
  renderSessions();
  renderServers();
}

function renderModeButtons() {
  for (const button of els.mode.querySelectorAll("button")) {
    button.setAttribute("aria-pressed", String(button.dataset.mode === state.mode));
  }
}

function liveAgents() {
  return [...state.agents.values()].filter((agent) => agent.live);
}

function byCreated(a, b) {
  return String(a.created ?? "").localeCompare(String(b.created ?? ""));
}

function waitingAgents() {
  return new Set([...state.pending.values()].map((request) => request.agent));
}

function renderAgentTree() {
  const agents = liveAgents();
  const waiting = waitingAgents();
  const key = JSON.stringify([
    state.selected,
    [...state.collapsed],
    [...waiting],
    agents.map((a) => [a.id, a.parent, displayName(a), a.status, a.activity, todoProgress(a.todos)]),
  ]);
  renderKeyed("tree", key, () => {
    const ids = new Set(agents.map((agent) => agent.id));
    const roots = agents.filter((agent) => !agent.parent || !ids.has(agent.parent)).sort(byCreated).reverse();
    setChildren(els.tree, ...roots.map((agent) => agentNode(agent, agents, waiting)));
    els.agentsEmpty.hidden = roots.length > 0;
  });
}

function agentNode(agent, agents, waiting) {
  const children = agents.filter((other) => other.parent === agent.id).sort(byCreated);
  const collapsed = state.collapsed.has(agent.id);
  return h(
    "li",
    {},
    agentRow(agent, children.length > 0, collapsed, waiting.has(agent.id)),
    children.length > 0 && !collapsed
      ? h("ul", { class: "tree" }, children.map((child) => agentNode(child, agents, waiting)))
      : null,
  );
}

function agentRow(agent, hasChildren, collapsed, waiting) {
  const progress = todoProgress(agent.todos);
  const twisty = hasChildren
    ? h("button", {
        class: `twisty${collapsed ? " collapsed" : ""}`,
        type: "button",
        "aria-label": collapsed ? "Show subagents" : "Hide subagents",
        onclick: () => toggleCollapsed(agent.id),
      })
    : h("span", { class: "twisty-space" });
  return h(
    "div",
    { class: `agent-row${agent.id === state.selected ? " selected" : ""}` },
    twisty,
    h(
      "button",
      { class: "agent-open", type: "button", title: `${agent.label} · ${agent.model}`, onclick: () => openAgent(agent.id) },
      h("span", { class: `dot ${agent.status}` }),
      h("span", { class: "agent-label" }, displayName(agent)),
      h(
        "span",
        { class: "agent-meta" },
        waiting ? h("span", { class: "badge warn" }, "approval") : null,
        progress ? h("span", { class: "progress" }, progress) : null,
      ),
      h("span", { class: "agent-activity" }, agent.activity || STATUS_TEXT[agent.status] || agent.status),
    ),
  );
}

function todoProgress(todos) {
  if (!todos?.length) return "";
  return `${todos.filter((todo) => todo.status === "completed").length}/${todos.length}`;
}

function toggleCollapsed(id) {
  if (!state.collapsed.delete(id)) state.collapsed.add(id);
  invalidate("sidebar");
}

function renderSessions() {
  const sessions = state.sessions.filter((session) => !state.agents.get(session.id)?.live);
  const key = JSON.stringify(sessions.map((s) => [s.id, s.title, s.updated, s.id === state.selected]));
  renderKeyed("sessions", key, () => {
    setChildren(els.sessions, ...sessions.map(sessionRow));
    els.sessionsEmpty.hidden = sessions.length > 0;
  });
}

function sessionRow(session) {
  const elsewhere = session.cwd !== state.cwd;
  return h(
    "li",
    {},
    h(
      "button",
      {
        class: `session-row${session.id === state.selected ? " selected" : ""}`,
        type: "button",
        title: `${session.cwd}\n${session.model}`,
        onclick: () => resumeSession(session),
      },
      h("span", { class: "session-title" }, session.title || session.label),
      h("span", { class: "session-time" }, relativeTime(session.updated)),
      elsewhere ? h("span", { class: "session-cwd" }, shortPath(session.cwd)) : null,
    ),
  );
}

function renderServers() {
  renderKeyed("servers", JSON.stringify(state.servers), () => {
    setChildren(
      els.servers,
      ...state.servers.map((server) =>
        h(
          "div",
          { class: `server${server.error ? " error" : ""}`, title: server.error ?? `${server.tools} tools` },
          h("span", { class: `dot ${server.error ? "failed" : "completed"}` }),
          h("span", {}, server.name),
          h("span", { class: "server-tools" }, server.error ? "unavailable" : `${server.tools} tools`),
        ),
      ),
    );
    els.servers.hidden = state.servers.length === 0;
  });
}

function renderHeader() {
  const agent = selectedAgent();
  renderBreadcrumb(agent);
  renderStatusLine(agent);
  renderActions(agent);
}

function lineage(agent) {
  const chain = [];
  for (let current = agent; current && chain.length < 32; current = state.agents.get(current.parent)) {
    chain.unshift(current);
  }
  return chain;
}

function renderBreadcrumb(agent) {
  const chain = agent ? lineage(agent) : [];
  const key = JSON.stringify(chain.map((a) => [a.id, displayName(a)]));
  renderKeyed("breadcrumb", key, () => {
    if (!agent) {
      setChildren(els.breadcrumb, h("span", { class: "crumb current" }, "New session"));
      return;
    }
    const orphan = chain[0].parent;
    const crumbs = chain.map((item, index) =>
      index === chain.length - 1
        ? h("span", { class: "crumb current", title: item.id }, displayName(item))
        : h("button", { class: "crumb", type: "button", onclick: () => openAgent(item.id) }, displayName(item)),
    );
    if (orphan) {
      crumbs.unshift(h("button", { class: "crumb", type: "button", onclick: () => viewAgent(orphan, {}) }, "parent"));
    }
    setChildren(els.breadcrumb, ...crumbs.flatMap((crumb, i) => (i ? [h("span", { class: "crumb-sep" }), crumb] : [crumb])));
  });
}

function renderStatusLine(agent) {
  if (!agent) {
    renderKeyed("status", state.cwd, () => setChildren(els.status, h("span", { class: "status-text" }, state.cwd)));
    return;
  }
  const dot = agent.live ? agent.status : "idle";
  const status = agent.live ? STATUS_TEXT[agent.status] ?? agent.status : "saved session";
  const activity = agent.live && agent.status === "running" ? agent.activity : "";
  const cwd = agent.cwd && agent.cwd !== state.cwd ? agent.cwd : "";
  const usage = usageText(agent.usage, agent.contextTokens);
  renderKeyed("status", JSON.stringify([dot, status, activity, cwd, usage]), () => {
    setChildren(
      els.status,
      h("span", { class: `dot ${dot}` }),
      h("span", { class: "status-text" }, status),
      activity ? h("span", { class: "status-activity" }, activity) : null,
      cwd ? h("span", { class: "status-cwd", title: cwd }, shortPath(cwd)) : null,
      h("span", { class: "status-usage" }, usage),
    );
  });
}

function renderActions(agent) {
  const running = agent?.live && agent.status === "running";
  const key = JSON.stringify([agent?.id, agent?.live, running, agent?.model, state.models, state.mode]);
  renderKeyed("actions", key, () => {
    const items = [];
    if (state.mode === "autopilot") {
      items.push(h("span", { class: "badge danger", title: "Tools run without asking" }, "autopilot"));
    }
    if (agent?.live) {
      items.push(modelPicker(agent));
      items.push(h("button", { class: "button", type: "button", disabled: running, onclick: () => compact(agent) }, "Compact"));
    }
    if (running) {
      items.push(h("button", { class: "button danger", type: "button", onclick: () => stop(agent) }, "Stop"));
    }
    setChildren(els.actions, ...items);
  });
}

function modelPicker(agent) {
  const select = h("select", {
    class: "select",
    "aria-label": "Model",
    title: "Model for the next turn",
    onchange: () => changeModel(agent, select.value),
  });
  fillModels(select, agent.model);
  return select;
}

function fillModels(select, current) {
  const models = !current || state.models.includes(current) ? state.models : [current, ...state.models];
  setChildren(select, ...models.map((model) => h("option", { value: model }, model)));
  if (current) select.value = current;
}

function renderPermissions() {
  const requests = [...state.pending.values()];
  renderKeyed("permissions", JSON.stringify(requests.map((r) => r.id)), () => {
    setChildren(els.permissions, ...requests.map(permissionCard));
  });
  document.title = requests.length ? `(${requests.length}) malac` : "malac";
}

function permissionCard(request) {
  const agent = state.agents.get(request.agent);
  const label = agent ? displayName(agent) : request.agent_label ?? "agent";
  const actions = h("div", { class: "permission-actions" });
  const decide = (decision) => () => {
    if (!send({ type: "approve", request_id: request.id, decision })) return;
    for (const button of actions.querySelectorAll("button")) button.disabled = true;
  };
  actions.append(
    h("button", { class: "button primary", type: "button", onclick: decide("allow") }, "Allow"),
    h(
      "button",
      { class: "button", type: "button", title: "Allow this and similar calls for the rest of the session", onclick: decide("allow_session") },
      "Always",
    ),
    h("button", { class: "button danger", type: "button", onclick: decide("deny") }, "Deny"),
  );
  return h(
    "div",
    { class: "permission" },
    h(
      "div",
      { class: "permission-head" },
      h("span", { class: "permission-title" }, "Approve ", h("code", {}, request.tool)),
      h("button", { class: "link", type: "button", onclick: () => openAgent(request.agent) }, label),
    ),
    request.summary ? h("pre", { class: "permission-summary" }, request.summary) : null,
    h("details", { class: "permission-input" }, h("summary", {}, "Input"), h("pre", {}, prettyJson(request.input))),
    actions,
  );
}

function renderComposer() {
  const agent = selectedAgent();
  els.model.hidden = Boolean(agent) || state.models.length === 0;
  if (!agent) els.input.placeholder = "Describe a task to start a new session";
  else if (agent.live) els.input.placeholder = `Message ${agent.label}`;
  else els.input.placeholder = "Send a message to resume this session";
}

function toast(message, level = "info") {
  const node = h("div", { class: `toast ${level}`, role: "status" }, message);
  els.toasts.append(node);
  setTimeout(() => node.remove(), TOAST_MS);
}

function fatal(message) {
  els.composer.hidden = true;
  setChildren(els.transcript, h("div", { class: "empty" }, h("p", { class: "empty-title" }, "Cannot connect"), h("p", {}, message)));
}

// Transcript

function follow() {
  if (!state.pinned || state.followFrame) return;
  state.followFrame = requestAnimationFrame(() => {
    state.followFrame = 0;
    els.scroller.scrollTop = els.scroller.scrollHeight;
  });
}

class TranscriptView {
  constructor(agent) {
    this.agent = agent;
    this.root = h("div", { class: "transcript" });
    this.cards = new Map();
    this.approvals = new Map();
    this.spawns = [];
    this.lastTodo = null;
    this.draft = null;
    this.turn = null;
    this.ready = false;
  }

  async load() {
    const agent = this.agent;
    let saved = agent.history;
    if (!saved) {
      const loading = h("div", { class: "loading" }, "Loading transcript");
      this.root.append(loading);
      try {
        saved = await loadHistory(agent);
      } catch (error) {
        saved = { messages: [], cutoff: 0, error: error.message };
      }
      loading.remove();
    }
    if (state.view !== this) return;
    if (saved.error) this.notice("error", `Could not load the saved transcript: ${saved.error}`);
    for (const message of saved.messages) this.message(message);
    for (const { n, env } of agent.events) {
      if (n > saved.cutoff) this.apply(env);
    }
    this.linkKnownChildren();
    this.ready = true;
    if (!this.root.childElementCount) this.root.append(emptyAgentState(agent));
    state.pinned = true;
    follow();
  }

  apply(env) {
    switch (env.type) {
      case "user_message":
        this.userMessage(env.text, env.images ?? []);
        this.turn = { started: Date.parse(env.at), usage: emptyUsage(), context: 0 };
        break;
      case "text_delta":
        this.draftText(env.text);
        break;
      case "thinking_delta":
        this.draftThinking(env.text);
        break;
      case "tool_call_streaming":
        this.draftTool(env.tool);
        break;
      case "assistant_message":
        this.clearDraft();
        this.message(env.message);
        break;
      case "tool_started":
        this.cards.get(env.call_id)?.start(env.summary);
        break;
      case "tool_finished":
        this.cards.get(env.call_id)?.finish(env.output, env.is_error, env.millis);
        break;
      case "permission_requested":
        this.permissionRequested(env);
        break;
      case "permission_resolved":
        this.permissionResolved(env);
        break;
      case "usage":
        if (this.turn) {
          addUsage(this.turn.usage, env.turn);
          this.turn.context = env.context_tokens;
        }
        break;
      case "compacted":
        this.append(h("details", { class: "compacted" }, h("summary", {}, "Conversation compacted"), markdownNode(env.summary)));
        break;
      case "notice":
        this.notice(env.level, env.message);
        break;
      case "turn_finished":
        this.turnFinished(env);
        break;
      case "child_started":
        this.childStarted(env.child);
        break;
    }
  }

  append(node) {
    this.root.querySelector(":scope > .empty")?.remove();
    if (this.draft && node !== this.draft.root) this.root.insertBefore(node, this.draft.root);
    else this.root.append(node);
    follow();
  }

  message(message) {
    if (message.role === "user") this.userBlocks(message.content);
    else this.assistantBlocks(message.content);
  }

  userBlocks(blocks) {
    const text = [];
    const images = [];
    for (const block of blocks) {
      if (block.type === "tool_result") this.cards.get(block.tool_use_id)?.result(block);
      else if (block.type === "image") images.push(block);
      else if (block.type === "text" && block.text.startsWith("<recalled-memories>")) this.append(recalledNode(block.text));
      else if (block.type === "text") text.push(block.text);
    }
    if (text.length || images.length) this.userMessage(text.join("\n"), images);
  }

  assistantBlocks(blocks) {
    for (const block of blocks) {
      if (block.type === "text" && block.text.trim()) this.append(markdownNode(block.text, "assistant"));
      else if (block.type === "thinking" && block.text.trim()) this.append(thinkingNode(block.text));
      else if (block.type === "tool_use") this.toolUse(block);
    }
  }

  userMessage(text, images) {
    this.clearDraft();
    this.append(
      h(
        "div",
        { class: "user" },
        images.length ? h("div", { class: "attachments-row" }, images.map(imageNode)) : null,
        text ? h("div", { class: "user-text" }, text) : null,
      ),
    );
  }

  toolUse(block) {
    const card = createCard(block);
    this.cards.set(block.id, card);
    this.append(card.el);
    if (card instanceof SpawnCard) this.spawns.push(card);
    if (card instanceof TodoCard) {
      this.lastTodo?.supersede();
      this.lastTodo = card;
    }
  }

  ensureDraft() {
    if (!this.draft) {
      this.draft = { root: h("div", { class: "draft" }), text: null, thinking: null, preparing: null };
      this.append(this.draft.root);
    }
    return this.draft;
  }

  draftText(text) {
    const draft = this.ensureDraft();
    if (!draft.text) {
      draft.text = document.createTextNode("");
      draft.root.insertBefore(h("div", { class: "streaming" }, draft.text), draft.preparing);
    }
    draft.text.appendData(text);
    follow();
  }

  draftThinking(text) {
    const draft = this.ensureDraft();
    if (!draft.thinking) {
      draft.thinking = document.createTextNode("");
      const node = h("details", { class: "thinking live" }, h("summary", {}, "Thinking"), h("div", { class: "thinking-body" }, draft.thinking));
      draft.root.insertBefore(node, draft.root.firstChild);
    }
    draft.thinking.appendData(text);
  }

  draftTool(tool) {
    const draft = this.ensureDraft();
    draft.preparing ??= draft.root.appendChild(h("div", { class: "preparing" }));
    draft.preparing.textContent = `Preparing ${tool}`;
    follow();
  }

  clearDraft() {
    this.draft?.root.remove();
    this.draft = null;
  }

  turnFinished(env) {
    if (this.draft) {
      this.draft.preparing?.remove();
      this.draft.root.classList.add("interrupted");
      this.draft = null;
    }
    for (const card of this.cards.values()) card.abandon();
    const parts = [];
    if (env.status !== "completed") parts.push(h("span", { class: `turn-status ${env.status}` }, STATUS_TEXT[env.status] ?? env.status));
    if (this.turn) {
      parts.push(h("span", {}, formatDuration(Date.parse(env.at) - this.turn.started)));
      parts.push(h("span", {}, usageText(this.turn.usage, this.turn.context)));
    }
    if (parts.length) this.append(h("div", { class: "turn-end" }, parts));
    this.turn = null;
  }

  notice(level, message) {
    this.append(h("div", { class: `notice ${level}` }, h("span", { class: "notice-level" }, level), message));
  }

  // Permission events carry no call id, so the card is found by tool and input.
  permissionRequested(env) {
    const input = JSON.stringify(env.input);
    for (const card of this.cards.values()) {
      if (card instanceof ToolCard && !card.done && card.name === env.tool && card.inputJson === input) {
        this.approvals.set(env.request_id, card);
        card.awaitApproval();
        return;
      }
    }
  }

  permissionResolved(env) {
    const card = this.approvals.get(env.request_id);
    this.approvals.delete(env.request_id);
    if (!card) return;
    if (env.allowed) card.approved();
    else card.finish("Denied", true);
  }

  childStarted(childId) {
    const child = state.agents.get(childId);
    const open = this.spawns.filter((card) => !card.child);
    const card =
      open.findLast((c) => c.started && c.label === child?.label) ?? open.findLast((c) => c.started) ?? open.at(-1);
    if (card) {
      card.link(childId);
      return;
    }
    const standalone = new SpawnCard({ input: { label: child?.label } });
    standalone.start();
    standalone.link(childId);
    this.spawns.push(standalone);
    this.append(standalone.el);
  }

  linkKnownChildren() {
    const linked = new Set(this.spawns.map((card) => card.child));
    const children = [...state.agents.values()]
      .filter((agent) => agent.parent === this.agent.id && !linked.has(agent.id))
      .sort(byCreated);
    for (const card of this.spawns.filter((c) => !c.child)) {
      const index = children.findIndex((agent) => agent.label === card.label);
      if (index >= 0) card.link(children.splice(index, 1)[0].id);
    }
  }

  refreshChild(id) {
    for (const card of this.spawns) {
      if (card.child === id) card.refresh();
    }
  }
}

function emptyAgentState(agent) {
  return h(
    "div",
    { class: "empty" },
    h("p", { class: "empty-title" }, displayName(agent)),
    h("p", {}, agent.live ? "Waiting for the first message." : "This session has no messages."),
  );
}

function newSessionState() {
  return h(
    "div",
    { class: "empty" },
    h("p", { class: "empty-title" }, "New session"),
    h("p", { class: "empty-cwd" }, state.cwd),
    h("p", {}, "Describe a task below. Paste or drop images to attach them."),
  );
}

function thinkingNode(text) {
  return h("details", { class: "thinking" }, h("summary", {}, "Thinking"), h("div", { class: "thinking-body" }, text));
}

function recalledNode(text) {
  return h("details", { class: "thinking" }, h("summary", {}, "Recalled memories"), h("div", { class: "thinking-body" }, text));
}

function createCard(use) {
  if (use.name === "agent_spawn") return new SpawnCard(use);
  if (use.name === "todo") return new TodoCard(use);
  return new ToolCard(use);
}

class ToolCard {
  constructor(use) {
    this.name = use.name;
    this.inputJson = JSON.stringify(use.input);
    this.done = false;
    this.summary = h("span", { class: "tool-summary" }, summarizeInput(use.name, use.input));
    this.time = h("span", { class: "tool-time" });
    this.errorLine = h("span", { class: "tool-error-line", hidden: true });
    this.output = h("pre", { class: "tool-output" });
    this.body = h(
      "div",
      { class: "tool-body" },
      h("div", { class: "tool-label" }, "Input"),
      h("pre", { class: "tool-input" }, prettyJson(use.input)),
      h("div", { class: "tool-label" }, "Output"),
      this.output,
    );
    this.el = h(
      "details",
      { class: "tool", "data-state": "pending" },
      h("summary", {}, h("span", { class: "tool-state" }), h("span", { class: "tool-name" }, use.name), this.summary, this.time, this.errorLine),
      this.body,
    );
  }

  start(summary) {
    if (summary) this.summary.textContent = summary;
    this.el.dataset.state = "running";
  }

  awaitApproval() {
    this.el.dataset.state = "approval";
  }

  approved() {
    if (this.el.dataset.state === "approval") this.el.dataset.state = "pending";
  }

  finish(output, isError, millis) {
    this.done = true;
    this.el.dataset.state = isError ? "error" : "done";
    this.output.textContent = output || "(no output)";
    if (millis != null) this.time.textContent = formatDuration(millis);
    if (isError) {
      this.errorLine.textContent = firstLine(output);
      this.errorLine.hidden = false;
    }
  }

  result(block) {
    this.finish(resultText(block.content), block.is_error);
    const images = (block.content ?? []).filter((item) => item.type === "image").map(imageNode).filter(Boolean);
    if (images.length) this.body.append(h("div", { class: "attachments-row" }, images));
  }

  abandon() {
    if (!this.done) this.el.dataset.state = "skipped";
  }
}

class SpawnCard {
  constructor(use) {
    this.input = use.input ?? {};
    this.label = this.input.label || this.input.agent || "agent";
    this.child = null;
    this.started = false;
    this.done = false;
    this.isError = false;
    this.output = "";
    this.millis = null;
    this.shownReport = null;
    this.dot = h("span", { class: "dot idle" });
    this.status = h("span", { class: "subagent-status" }, "queued");
    this.activity = h("div", { class: "subagent-activity", hidden: true });
    this.report = h("div", { class: "subagent-report", hidden: true });
    this.openButton = h("button", { class: "button small", type: "button", disabled: true, onclick: () => this.open() }, "Open");
    this.el = h(
      "div",
      { class: "subagent" },
      h(
        "div",
        { class: "subagent-head" },
        this.dot,
        h("span", { class: "subagent-label" }, this.label),
        h("span", { class: "tag" }, "subagent"),
        this.status,
        this.openButton,
      ),
      this.input.task ? h("div", { class: "subagent-task", title: this.input.task }, firstLine(this.input.task)) : null,
      this.activity,
      this.report,
    );
  }

  start() {
    this.started = true;
    this.refresh();
  }

  link(id) {
    this.child = id;
    this.openButton.disabled = false;
    this.refresh();
  }

  finish(output, isError, millis) {
    this.done = true;
    this.isError = isError;
    this.output = output ?? "";
    this.millis = millis ?? null;
    const id = parseAgentId(this.output);
    if (!this.child && id) this.link(id);
    else this.refresh();
  }

  result(block) {
    this.finish(resultText(block.content), block.is_error);
  }

  abandon() {
    if (!this.done && !this.child) this.status.textContent = "not started";
  }

  agent() {
    return this.child ? state.agents.get(this.child) : null;
  }

  currentStatus() {
    const agent = this.agent();
    if (agent?.live) return agent.status;
    if (!this.done) return this.started ? "running" : "idle";
    if (!this.isError) return "completed";
    return this.output.startsWith("cancelled") ? "cancelled" : "failed";
  }

  refresh() {
    const agent = this.agent();
    const status = this.currentStatus();
    this.dot.className = `dot ${status}`;
    const elapsed = this.millis != null && status !== "running" ? ` · ${formatDuration(this.millis)}` : "";
    this.status.textContent = `${STATUS_TEXT[status] ?? status}${elapsed}`;
    const activity = status === "running" ? agent?.activity ?? "" : "";
    if (this.activity.textContent !== activity) this.activity.textContent = activity;
    this.activity.hidden = !activity;
    this.showReport(agent?.report || (this.done ? reportFrom(this.output) || this.output : ""));
  }

  showReport(report) {
    if (report === this.shownReport) return;
    this.shownReport = report;
    setChildren(this.report, report ? markdownNode(report) : "");
    this.report.hidden = !report;
    follow();
  }

  open() {
    if (this.child) viewAgent(this.child, { label: this.label, parent: state.view?.agent.id ?? null });
  }
}

class TodoCard {
  constructor(use) {
    const items = Array.isArray(use.input?.items) ? use.input.items : [];
    this.errorLine = h("div", { class: "tool-error-line", hidden: true });
    this.el = h(
      "details",
      { class: "todos", open: true },
      h("summary", {}, h("span", { class: "todos-title" }, "Plan"), h("span", { class: "progress" }, todoProgress(items))),
      h("ul", {}, items.map((item) => h("li", { class: `todo ${item.status}` }, item.content))),
      this.errorLine,
    );
  }

  start() {}

  finish(output, isError) {
    if (!isError) return;
    this.errorLine.textContent = firstLine(output);
    this.errorLine.hidden = false;
  }

  result(block) {
    this.finish(resultText(block.content), block.is_error);
  }

  abandon() {}

  supersede() {
    this.el.open = false;
  }
}

// Navigation and commands

function openAgent(id) {
  if (id && !state.agents.has(id)) ensureAgent(id).live = false;
  state.selected = id ?? null;
  const agent = selectedAgent();
  state.view = agent ? new TranscriptView(agent) : null;
  setChildren(els.transcript, state.view ? state.view.root : newSessionState());
  history.replaceState(null, "", `${location.pathname}${location.search}${id ? `#agent=${id}` : ""}`);
  invalidate("sidebar", "header", "composer");
  if (narrowScreen.matches) setSidebar(false);
  state.view?.load();
}

function viewAgent(id, defaults) {
  if (!state.agents.has(id)) Object.assign(ensureAgent(id), defaults, { live: false });
  openAgent(id);
}

function resumeSession(session) {
  if (state.agents.get(session.id)?.live) {
    openAgent(session.id);
    return;
  }
  send({ type: "resume", session: session.id }, (id) => {
    const agent = ensureAgent(id);
    applyMeta(agent, session);
    agent.live = true;
    openAgent(id);
  });
}

function submitPrompt(text, images, agentId) {
  const agent = agentId ? state.agents.get(agentId) : null;
  if (!agent) return startSession(text, images);
  if (!agent.live) return resumeAndPrompt(agent, text, images);
  return promptAgent(agent, text, images);
}

function startSession(text, images) {
  const model = els.model.value || undefined;
  return send({ type: "prompt", agent: null, text, images, model }, (id) => {
    const agent = ensureAgent(id);
    agent.history ??= { messages: [], cutoff: 0 };
    queueImages(agent, text, images);
    openAgent(id);
  });
}

function promptAgent(agent, text, images) {
  const sent = send({ type: "prompt", agent: agent.id, text, images });
  if (sent && images.length) agent.outgoing.push({ text, images });
  return sent;
}

function resumeAndPrompt(agent, text, images) {
  return send({ type: "resume", session: agent.id }, (id) => {
    const resumed = ensureAgent(id);
    resumed.live = true;
    promptAgent(resumed, text, images);
    invalidate("sidebar", "header", "composer");
  });
}

function stop(agent) {
  send({ type: "cancel", agent: agent.id });
}

function compact(agent) {
  if (send({ type: "compact", agent: agent.id })) toast(`Compacting the conversation of ${agent.label}`);
}

function changeModel(agent, model) {
  if (!send({ type: "model", agent: agent.id, model })) return;
  agent.model = model;
  invalidate("header");
}

function setMode(mode) {
  state.mode = mode;
  els.app.classList.toggle("autopilot", mode === "autopilot");
  invalidate("sidebar", "header");
}

function requestMode(mode) {
  if (mode !== state.mode && send({ type: "mode", mode })) setMode(mode);
}

function setSidebar(open) {
  els.app.classList.toggle("sidebar-open", open);
}

// Composer

function submitComposer() {
  const text = els.input.value.trim();
  if (!text && !state.images.length) return;
  if (!submitPrompt(text, state.images, state.selected)) return;
  els.input.value = "";
  state.images = [];
  renderAttachments();
  autosize();
}

function autosize() {
  els.input.style.height = "auto";
  els.input.style.height = `${els.input.scrollHeight}px`;
}

function filesFrom(transfer) {
  return [...(transfer?.items ?? [])]
    .filter((item) => item.kind === "file")
    .map((item) => item.getAsFile())
    .filter(Boolean);
}

async function addImages(files) {
  for (const file of files) {
    if (!IMAGE_TYPES.has(file.type)) {
      toast(`Only PNG, JPEG, GIF and WebP images can be attached (${file.name})`, "warning");
    } else if (file.size > MAX_IMAGE_BYTES) {
      toast(`${file.name} is larger than ${MAX_IMAGE_BYTES / 1024 / 1024} MB`, "warning");
    } else {
      state.images.push({ media_type: file.type, data: await readBase64(file) });
    }
  }
  renderAttachments();
}

function readBase64(file) {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result).split(",", 2)[1]);
    reader.onerror = () => reject(reader.error);
    reader.readAsDataURL(file);
  });
}

function renderAttachments() {
  setChildren(
    els.attachments,
    ...state.images.map((image, index) =>
      h(
        "div",
        { class: "attachment-chip" },
        imageNode(image),
        h("button", { class: "remove", type: "button", "aria-label": "Remove image", onclick: () => removeImage(index) }),
      ),
    ),
  );
  els.attachments.hidden = state.images.length === 0;
}

function removeImage(index) {
  state.images.splice(index, 1);
  renderAttachments();
}

function bindComposer() {
  els.composer.addEventListener("submit", (event) => {
    event.preventDefault();
    submitComposer();
  });
  els.input.addEventListener("keydown", (event) => {
    if (event.key === "Enter" && !event.shiftKey && !event.isComposing) {
      event.preventDefault();
      submitComposer();
    }
  });
  els.input.addEventListener("input", autosize);
  els.input.addEventListener("paste", (event) => addImages(filesFrom(event.clipboardData)));
  els.composer.addEventListener("dragover", (event) => {
    if (!event.dataTransfer?.types.includes("Files")) return;
    event.preventDefault();
    els.composer.classList.add("dragging");
  });
  els.composer.addEventListener("dragleave", () => els.composer.classList.remove("dragging"));
  els.composer.addEventListener("drop", (event) => {
    event.preventDefault();
    els.composer.classList.remove("dragging");
    addImages(filesFrom(event.dataTransfer));
  });
}

// A VS Code webview hosting this page in an iframe can send prompts; they
// wait in the inbox until the socket is open.
function bindEditorBridge() {
  window.addEventListener("message", (event) => {
    if (typeof event.origin !== "string" || !event.origin.startsWith("vscode-webview://")) return;
    const { type, text, agent } = event.data ?? {};
    if (type !== "malac.prompt" || typeof text !== "string" || !text.trim()) return;
    state.inbox.push({ text, agent: typeof agent === "string" ? agent : null });
    deliverInbox();
  });
}

function deliverInbox() {
  if (state.socket?.readyState !== WebSocket.OPEN) return;
  for (const { text, agent } of state.inbox.splice(0)) submitPrompt(text, [], agent ?? state.selected);
}

function bindChrome() {
  els.toggleSidebar.addEventListener("click", () => setSidebar(!els.app.classList.contains("sidebar-open")));
  els.backdrop.addEventListener("click", () => setSidebar(false));
  els.newSession.addEventListener("click", () => {
    openAgent(null);
    els.input.focus();
  });
  els.mode.addEventListener("click", (event) => {
    const mode = event.target.closest("button")?.dataset.mode;
    if (mode) requestMode(mode);
  });
  els.scroller.addEventListener("scroll", () => {
    const distance = els.scroller.scrollHeight - els.scroller.scrollTop - els.scroller.clientHeight;
    state.pinned = distance < PIN_THRESHOLD_PX;
  });
  narrowScreen.addEventListener("change", (event) => {
    if (event.matches) setSidebar(false);
  });
}

async function init() {
  bindChrome();
  bindComposer();
  bindEditorBridge();
  setSidebar(params.get("embed") !== "1" && !narrowScreen.matches);
  state.selected = /^#agent=([\w-]+)$/.exec(location.hash)?.[1] ?? null;
  state.token = readToken();
  if (!state.token) {
    fatal("Open the address printed by `malac web`; it includes the access token.");
    return;
  }
  try {
    await loadState();
  } catch (error) {
    if (error.status === 401) {
      fatal("The access token was not accepted. Open the address printed by `malac web` again.");
      return;
    }
    toast(error.message, "error");
  }
  if (!state.selected) openAgent(null);
  connect();
}

init();

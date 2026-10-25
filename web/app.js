// App shell: routing, sidebar, top bar, composer, approvals and shortcuts.

import {
  baseName,
  closePopover,
  confirmDialog,
  dateGroup,
  dialog,
  fullTime,
  h,
  local,
  popover,
  relativeTime,
  setChildren,
  toast,
} from "./dom.js";
import { icon, pigLogo } from "./icons.js";
import {
  STATUS_TEXT,
  byCreated,
  changeModel,
  compactAgent,
  connect,
  decide,
  deleteSession,
  displayName,
  ensure,
  ensureAgent,
  invalidate,
  lineage,
  liveAgents,
  loadState,
  params,
  readToken,
  registerRenderer,
  renderKeyed,
  requestMode,
  resumeSession,
  selectedAgent,
  send,
  setNavigator,
  setResyncHandler,
  state,
  stopAgent,
  submitPrompt,
  todoProgress,
  waitingAgents,
} from "./store.js";
import { SettingsPage } from "./settings.js";
import { TranscriptView } from "./transcript.js";
import { TAGLINE, agentsView, homeView, refreshUpdatePanels, updatePanel } from "./views.js";
import { MODES, modeSwitch, modelPicker, statusDot, usageMeter } from "./widgets.js";

const IMAGE_TYPES = new Set(["image/png", "image/jpeg", "image/gif", "image/webp"]);
const MAX_IMAGE_BYTES = 8 * 1024 * 1024;
const HISTORY_LIMIT = 50;
const narrowScreen = window.matchMedia("(max-width: 900px)");
const wideMain = window.matchMedia("(min-width: 1180px)");
const darkScheme = window.matchMedia("(prefers-color-scheme: dark)");
const embedded = params.get("embed") === "1";

const $ = (id) => document.getElementById(id);
const els = {
  app: $("app"),
  sidebar: $("sidebar"),
  backdrop: $("backdrop"),
  brandLogo: $("brand-logo"),
  connection: $("connection"),
  collapseSidebar: $("collapse-sidebar"),
  toggleSidebar: $("toggle-sidebar"),
  newSession: $("new-session"),
  nav: [...document.querySelectorAll(".nav-link")],
  liveSection: $("live-section"),
  tree: $("agent-tree"),
  sessionScope: $("session-scope"),
  sessionFilter: $("session-filter"),
  sessions: $("sessions"),
  project: $("project"),
  version: $("version"),
  breadcrumb: $("breadcrumb"),
  topbarStatus: $("topbar-status"),
  topbarControls: $("topbar-controls"),
  updateButton: $("update-button"),
  themeToggle: $("theme-toggle"),
  helpButton: $("help-button"),
  settingsButton: $("settings-button"),
  offline: $("offline"),
  permissions: $("permissions"),
  content: $("content"),
  composerDock: $("composer-dock"),
  composer: $("composer"),
  attachments: $("attachments"),
  input: $("input"),
  attach: $("attach"),
  fileInput: $("file-input"),
  composerModel: $("composer-model"),
  stop: $("stop"),
  send: $("send"),
  jump: $("jump"),
};

const ui = {
  view: null,
  transcript: null,
  home: null,
  agents: null,
  settings: null,
  images: [],
  sent: [],
  recall: -1,
  collapsed: new Set(),
  sessionScope: local.get("malac.sessionScope") === "project" ? "project" : "all",
  sessionFilter: "",
  newModel: null,
  sidebarOpen: !embedded && local.get("malac.sidebar") !== "closed",
};

// Theme

function effectiveTheme() {
  return document.documentElement.dataset.theme ?? (darkScheme.matches ? "dark" : "light");
}

function applyTheme(preference) {
  if (preference === "light" || preference === "dark") document.documentElement.dataset.theme = preference;
  else delete document.documentElement.dataset.theme;
  local.set("malac.theme", preference === "light" || preference === "dark" ? preference : null);
  renderThemeButton();
}

function toggleTheme() {
  applyTheme(effectiveTheme() === "dark" ? "light" : "dark");
}

function renderThemeButton() {
  const dark = effectiveTheme() === "dark";
  setChildren(els.themeToggle, icon(dark ? "sun" : "moon", 16));
  els.themeToggle.setAttribute("aria-label", dark ? "Switch to light theme" : "Switch to dark theme");
}

// Routing

function routeFromHash() {
  const hash = location.hash.replace(/^#/, "");
  let match = /^agent=([\w-]+)$/.exec(hash);
  if (match) return { name: "agent", id: match[1] };
  if (hash === "agents") return { name: "agents" };
  match = /^settings(?:=([\w-]+))?$/.exec(hash);
  if (match) return { name: "settings", section: match[1] ?? "general" };
  return { name: "home" };
}

function hashFor(route) {
  switch (route.name) {
    case "agent":
      return `#agent=${route.id}`;
    case "agents":
      return "#agents";
    case "settings":
      return route.section && route.section !== "general" ? `#settings=${route.section}` : "#settings";
    default:
      return "";
  }
}

let currentHash = location.hash;

function go(route) {
  const hash = hashFor(route);
  if (hash === location.hash || (hash === "" && location.hash === "")) {
    show(route);
    return;
  }
  if (hash) location.hash = hash;
  else history.pushState(null, "", `${location.pathname}${location.search}`);
  onHashChange();
}

let confirmingLeave = false;

async function onHashChange() {
  const route = routeFromHash();
  if (confirmingLeave) return;
  if (location.hash === currentHash && ui.view === route.name && route.name !== "settings") return;
  if (state.route.name === "settings" && route.name !== "settings" && ui.settings?.isDirty()) {
    confirmingLeave = true;
    const ok = await confirmDialog({
      title: "Discard unsaved settings?",
      message: "You have changes that are not saved yet.",
      confirm: "Discard",
      danger: true,
    });
    confirmingLeave = false;
    if (!ok) {
      history.replaceState(null, "", `${location.pathname}${location.search}${currentHash}`);
      return;
    }
    ui.settings.discard();
  }
  currentHash = location.hash;
  show(route);
}

function show(route) {
  const sameView = ui.view === route.name && route.name !== "agent";
  const composerFocused = document.activeElement === els.input;
  if (!sameView) teardown();
  state.route = route;
  state.selected = route.name === "agent" ? route.id : null;
  ui.view = route.name;
  els.app.dataset.view = route.name;
  switch (route.name) {
    case "agent":
      showAgent(route.id);
      break;
    case "agents":
      if (!sameView) {
        ui.agents = agentsView({ isWide: () => wideMain.matches });
        mount(ui.agents.root);
      }
      ui.agents.render();
      break;
    case "settings":
      ui.settings ??= new SettingsPage({ onDirtyChange: () => invalidate("title") });
      if (!sameView) mount(ui.settings.root);
      ui.settings.show(route.section);
      break;
    default:
      if (!sameView) {
        ui.home = homeView({ composerSlot: h("div", { class: "home-composer" }), onQuickStart: quickStart });
        mount(h("div", { class: "view-scroll" }, ui.home.root));
        ui.home.root.querySelector(".home-composer").append(els.composer);
      }
      ui.home.render();
  }
  els.composerDock.hidden = route.name !== "agent";
  if (route.name !== "home" && els.composer.parentElement !== els.composerDock) els.composerDock.append(els.composer);
  autosize();
  // Moving the composer between the home screen and the dock drops focus.
  if (composerFocused && (route.name === "home" || route.name === "agent")) els.input.focus();
  if (narrowScreen.matches) setSidebar(false);
  invalidate("sidebar", "header", "composer", "permissions", "title", "jump");
}

function teardown() {
  ui.transcript?.dispose();
  ui.transcript = null;
  ui.agents?.dispose();
  ui.agents = null;
  ui.home = null;
}

function mount(node) {
  setChildren(els.content, node);
}

function showAgent(id) {
  ui.transcript?.dispose();
  if (!state.agents.has(id)) ensureAgent(id).live = false;
  const agent = state.agents.get(id);
  const scroller = h("div", { class: "view-scroll transcript-scroll" });
  ui.transcript = new TranscriptView(agent, { scroller });
  scroller.append(ui.transcript.root);
  mount(scroller);
  ui.transcript.load();
}

function newSession() {
  go({ name: "home" });
  requestAnimationFrame(() => els.input.focus());
}

function quickStart(text) {
  els.input.value = text;
  autosize();
  els.input.focus();
  els.input.setSelectionRange(text.length, text.length);
}

// Sidebar

function setSidebar(open, { remember = false } = {}) {
  ui.sidebarOpen = open;
  els.app.classList.toggle("sidebar-open", open);
  els.toggleSidebar.setAttribute("aria-expanded", String(open));
  if (remember && !narrowScreen.matches && !embedded) local.set("malac.sidebar", open ? null : "closed");
}

function renderSidebar() {
  renderNav();
  renderAgentTree();
  renderSessions();
  renderFooter();
  const busy = liveAgents().some((a) => a.status === "running");
  els.brandLogo.firstElementChild?.classList.toggle("busy", busy);
}

function renderNav() {
  const running = liveAgents().filter((a) => a.status === "running").length;
  const waiting = waitingAgents().size;
  renderKeyed("nav", JSON.stringify([state.route.name, running, waiting]), () => {
    const labels = { home: ["home", "Home"], agents: ["agents", "Agents"], settings: ["settings", "Settings"] };
    for (const link of els.nav) {
      const [iconName, label] = labels[link.dataset.route];
      const count =
        link.dataset.route === "agents" && (running || waiting)
          ? h("span", { class: `nav-count${waiting ? " warn" : ""}`, title: waiting ? `${waiting} waiting for approval` : `${running} running` }, String(waiting || running))
          : null;
      setChildren(link, icon(iconName, 16), h("span", {}, label), count);
      const current = state.route.name === link.dataset.route;
      if (current) link.setAttribute("aria-current", "page");
      else link.removeAttribute("aria-current");
    }
  });
}

function renderAgentTree() {
  const agents = liveAgents();
  const waiting = waitingAgents();
  const key = JSON.stringify([
    state.selected,
    [...ui.collapsed],
    [...waiting],
    agents.map((a) => [a.id, a.parent, displayName(a), a.status, a.activity, todoProgress(a.todos)]),
  ]);
  renderKeyed("tree", key, () => {
    const ids = new Set(agents.map((agent) => agent.id));
    const roots = agents.filter((agent) => !agent.parent || !ids.has(agent.parent)).sort(byCreated).reverse();
    setChildren(els.tree, roots.map((agent) => agentNode(agent, agents, waiting)));
    els.liveSection.hidden = roots.length === 0;
  });
}

function agentNode(agent, agents, waiting) {
  const children = agents.filter((other) => other.parent === agent.id).sort(byCreated);
  const collapsed = ui.collapsed.has(agent.id);
  return h(
    "li",
    {},
    agentRow(agent, children.length > 0, collapsed, waiting.has(agent.id)),
    children.length > 0 && !collapsed ? h("ul", { class: "tree" }, children.map((child) => agentNode(child, agents, waiting))) : null,
  );
}

function agentRow(agent, hasChildren, collapsed, waiting) {
  const progress = todoProgress(agent.todos);
  const running = agent.status === "running";
  const twisty = hasChildren
    ? h(
        "button",
        {
          class: "twisty",
          type: "button",
          "aria-expanded": String(!collapsed),
          "aria-label": collapsed ? `Show subagents of ${displayName(agent)}` : `Hide subagents of ${displayName(agent)}`,
          onclick: () => {
            if (!ui.collapsed.delete(agent.id)) ui.collapsed.add(agent.id);
            invalidate("sidebar");
          },
        },
        icon(collapsed ? "chevronRight" : "chevronDown", 14),
      )
    : h("span", { class: "twisty-space" });
  return h(
    "div",
    { class: `agent-row${agent.id === state.selected ? " selected" : ""}` },
    twisty,
    h(
      "a",
      {
        class: "agent-open",
        href: `#agent=${agent.id}`,
        title: `${displayName(agent)}\n${agent.model}`,
        "aria-current": agent.id === state.selected ? "page" : null,
      },
      statusDot(waiting ? "waiting" : agent.status, waiting ? "Waiting for approval" : STATUS_TEXT[agent.status]),
      h("span", { class: "agent-label" }, displayName(agent)),
      waiting ? h("span", { class: "badge warn" }, "Approval") : progress ? h("span", { class: "agent-progress tabular" }, `${progress.done}/${progress.total}`) : null,
      running && agent.activity ? h("span", { class: "agent-activity" }, agent.activity) : null,
    ),
  );
}

function renderSessions() {
  const filter = ui.sessionFilter.trim().toLowerCase();
  const sessions = state.sessions.filter((session) => {
    if (state.agents.get(session.id)?.live) return false;
    if (ui.sessionScope === "project" && session.cwd !== state.cwd) return false;
    if (!filter) return true;
    return [session.title, session.label, session.model, session.cwd].some((v) => String(v ?? "").toLowerCase().includes(filter));
  });
  const key = JSON.stringify([ui.sessionScope, filter, state.selected, sessions.map((s) => [s.id, s.title, s.model, s.updated])]);
  renderKeyed("sessions", key, () => {
    for (const button of els.sessionScope.querySelectorAll("button")) {
      button.setAttribute("aria-pressed", String(button.dataset.scope === ui.sessionScope));
    }
    if (!sessions.length) {
      setChildren(
        els.sessions,
        h(
          "p",
          { class: "sidebar-empty" },
          filter ? "No sessions match." : ui.sessionScope === "project" ? "No saved sessions in this project." : "No saved sessions.",
        ),
      );
      return;
    }
    const groups = new Map();
    for (const session of sessions) {
      const group = dateGroup(session.updated);
      if (!groups.has(group)) groups.set(group, []);
      groups.get(group).push(session);
    }
    setChildren(
      els.sessions,
      [...groups].map(([group, items]) =>
        h("div", { class: "session-group" }, h("h3", { class: "session-group-title" }, group), h("ul", { class: "session-list" }, items.map(sessionRow))),
      ),
    );
  });
}

function sessionRow(session) {
  const elsewhere = session.cwd !== state.cwd;
  const title = session.title || session.label;
  return h(
    "li",
    { class: `session-row${session.id === state.selected ? " selected" : ""}` },
    h(
      "a",
      {
        class: "session-open",
        href: `#agent=${session.id}`,
        title: `${title}\n${session.model}\n${session.cwd}\nUpdated ${fullTime(session.updated)}`,
        "aria-current": session.id === state.selected ? "page" : null,
        onclick: (event) => {
          event.preventDefault();
          resumeSession(session);
        },
      },
      h("span", { class: "session-title" }, title),
      h(
        "span",
        { class: "session-meta" },
        h("span", { class: "session-model" }, session.model),
        elsewhere ? h("span", { class: "session-project" }, baseName(session.cwd)) : null,
        h("span", { class: "session-time tabular" }, relativeTime(session.updated)),
      ),
    ),
    h(
      "button",
      { class: "icon-button small session-delete", type: "button", "aria-label": `Delete session ${title}`, title: "Delete session", onclick: () => removeSession(session) },
      icon("trash", 14),
    ),
  );
}

async function removeSession(session) {
  const ok = await confirmDialog({
    title: "Delete this session?",
    message: `“${session.title || session.label}” and its transcript are deleted from disk. This cannot be undone.`,
    confirm: "Delete",
    danger: true,
  });
  if (!ok) return;
  try {
    await deleteSession(session.id);
    if (state.selected === session.id) go({ name: "home" });
    toast("Session deleted");
  } catch (error) {
    toast(error.message, "error");
  }
}

function renderFooter() {
  renderKeyed("footer", JSON.stringify([state.cwd, state.version]), () => {
    setChildren(els.project, icon("folder", 14), h("span", { class: "ellipsis", title: state.cwd }, baseName(state.cwd) || "No project"));
    els.version.textContent = state.version ? `v${state.version}` : "";
  });
}

function renderConnection() {
  const text = { online: "Connected", connecting: "Connecting", offline: "Offline" }[state.connection];
  setChildren(els.connection, h("span", { class: `dot ${state.connection === "online" ? "ok" : state.connection === "offline" ? "failed" : "waiting"}` }));
  els.connection.title = text;
  els.connection.setAttribute("aria-label", text);
  els.offline.hidden = state.connection !== "offline";
}

// Top bar

function renderHeader() {
  const agent = selectedAgent();
  renderBreadcrumb(agent);
  renderTopbarStatus(agent);
  renderTopbarControls(agent);
}

function renderBreadcrumb(agent) {
  const chain = agent ? lineage(agent) : [];
  const key = JSON.stringify([state.route.name, state.route.section, chain.map((a) => [a.id, displayName(a)])]);
  renderKeyed("breadcrumb", key, () => {
    const crumb = (text) => h("span", { class: "crumb current", "aria-current": "page" }, text);
    if (!agent) {
      const titles = { home: "New session", agents: "Agents", settings: "Settings" };
      setChildren(els.breadcrumb, crumb(titles[state.route.name] ?? "malac"));
      return;
    }
    const crumbs = chain.map((item, index) =>
      index === chain.length - 1
        ? h("span", { class: "crumb current", title: item.id, "aria-current": "page" }, displayName(item))
        : h("a", { class: "crumb", href: `#agent=${item.id}` }, displayName(item)),
    );
    if (chain[0].parent) crumbs.unshift(h("a", { class: "crumb", href: `#agent=${chain[0].parent}` }, "Parent"));
    setChildren(
      els.breadcrumb,
      crumbs.flatMap((node, i) => (i ? [h("span", { class: "crumb-sep", "aria-hidden": "true" }, icon("chevronRight", 14)), node] : [node])),
    );
  });
}

function renderTopbarStatus(agent) {
  const status = !agent ? null : agent.live ? agent.status : "saved";
  const waiting = agent && waitingAgents().has(agent.id);
  const activity = agent?.live && agent.status === "running" && !waiting ? agent.activity : "";
  renderKeyed("status", JSON.stringify([status, activity, waiting]), () => {
    if (!agent) {
      setChildren(els.topbarStatus);
      return;
    }
    setChildren(
      els.topbarStatus,
      statusDot(waiting ? "waiting" : agent.live ? agent.status : "idle"),
      h("span", { class: "topbar-status-text" }, waiting ? "Waiting for approval" : agent.live ? STATUS_TEXT[agent.status] ?? agent.status : "Saved session"),
      activity ? h("span", { class: "topbar-activity", title: activity }, activity) : null,
    );
  });
}

function renderTopbarControls(agent) {
  const running = agent?.live && agent.status === "running";
  const key = JSON.stringify([
    state.route.name,
    agent?.id,
    agent?.live,
    running,
    agent?.model,
    agent?.usage,
    agent?.contextTokens,
    state.mode,
    state.models,
  ]);
  renderKeyed("controls", key, () => {
    const items = [];
    if (agent) {
      const picker = modelPicker({
        value: agent.model,
        title: "Model for the next turn",
        align: "end",
        className: "topbar-model",
        onChange: (spec) => spec && switchModel(agent, spec),
      });
      items.push(picker.el);
    }
    items.push(modeSwitch(requestMode, { compact: true }));
    if (agent) {
      items.push(usageMeter(agent));
      if (agent.live) {
        items.push(
          h(
            "button",
            { class: "icon-button", type: "button", disabled: running, "aria-label": "Compact the conversation", title: "Compact the conversation", onclick: () => compactAgent(agent) },
            icon("compress", 16),
          ),
        );
      }
    }
    setChildren(els.topbarControls, items);
  });
}

function switchModel(agent, spec) {
  if (agent.live) {
    changeModel(agent, spec);
    return;
  }
  send({ type: "resume", session: agent.id }, (id) => {
    const resumed = ensureAgent(id);
    resumed.live = true;
    changeModel(resumed, spec);
    invalidate("sidebar", "header", "composer");
  });
}

// Approvals not shown inline in a visible transcript get a banner.

function renderPermissions() {
  const requests = [...state.pending.values()].filter((r) => ![...state.views].some((view) => view.showsRequest(r.id)));
  renderKeyed("permissions", JSON.stringify(requests.map((r) => r.id)), () => {
    setChildren(els.permissions, requests.map(permissionCard));
  });
  invalidate("title");
}

function permissionCard(request) {
  const agent = state.agents.get(request.agent);
  const label = agent ? displayName(agent) : request.agent_label ?? "agent";
  const actions = h("div", { class: "approval-actions" });
  const choose = (decision) => () => {
    if (!decide(request.id, decision)) return;
    for (const button of actions.querySelectorAll("button")) button.disabled = true;
  };
  actions.append(
    h("button", { class: "button primary small", type: "button", onclick: choose("allow") }, "Allow"),
    h("button", { class: "button small", type: "button", title: "Allow this and similar calls for the rest of the session", onclick: choose("allow_session") }, "Always allow"),
    h("button", { class: "button small danger", type: "button", onclick: choose("deny") }, "Deny"),
  );
  return h(
    "div",
    { class: "permission", role: "alert" },
    h(
      "div",
      { class: "permission-text" },
      icon("shield", 16),
      h(
        "div",
        {},
        h("p", { class: "permission-title" }, h("a", { href: `#agent=${request.agent}` }, label), " wants to run ", h("strong", {}, request.tool)),
        request.summary ? h("p", { class: "permission-summary", title: request.summary }, request.summary) : null,
      ),
    ),
    actions,
  );
}

function renderTitle() {
  const agent = selectedAgent();
  const pending = state.pending.size;
  const base =
    state.route.name === "agent" && agent
      ? `${displayName(agent)} · malac`
      : state.route.name === "agents"
        ? "Agents · malac"
        : state.route.name === "settings"
          ? `Settings${ui.settings?.isDirty() ? " (unsaved)" : ""} · malac`
          : `malac · ${TAGLINE}`;
  document.title = pending ? `(${pending}) ${base}` : base;
}

// A quiet indicator when a newer malac is available or already installed.
function renderUpdate() {
  const u = state.update;
  const version = u?.installed ?? (u?.available ? u.latest : null);
  els.updateButton.hidden = !version;
  if (version) {
    const text = u.installed ? `Restart for ${version}` : `Update ${version}`;
    setChildren(els.updateButton, h("span", { class: "dot ok" }), h("span", { class: "update-chip-text" }, text));
    els.updateButton.setAttribute("aria-label", u.installed ? `Version ${version} installed; restart malac to use it` : `Update to version ${version}`);
    els.updateButton.title = els.updateButton.getAttribute("aria-label");
  }
  refreshUpdatePanels();
}

function openUpdatePopover() {
  if (els.updateButton.getAttribute("aria-expanded") === "true") {
    closePopover();
    return;
  }
  const panel = updatePanel();
  popover(els.updateButton, h("div", { class: "update-popover" }, h("h2", { class: "popover-title" }, "Software update"), panel.el), { align: "end" });
  (panel.el.querySelector(".button.primary") ?? panel.el.querySelector("button"))?.focus();
}

function renderJump() {
  els.jump.hidden = !(ui.transcript && !ui.transcript.pinned);
}

// Composer

function renderComposer() {
  const agent = selectedAgent();
  const running = agent?.live && agent.status === "running";
  els.stop.hidden = !running;
  if (!agent) els.input.placeholder = "Describe a task to start a new session";
  else if (agent.live) els.input.placeholder = agent.parent ? `Message ${agent.label}` : "Send a follow-up";
  else els.input.placeholder = "Send a message to continue this session";
  renderKeyed("composer-model", JSON.stringify([Boolean(agent), ui.newModel, state.defaultModel, state.models]), () => {
    if (agent) {
      setChildren(els.composerModel);
      return;
    }
    const picker = modelPicker({
      value: ui.newModel ?? state.defaultModel,
      title: "Model for the new session",
      onChange: (spec) => {
        ui.newModel = spec;
      },
    });
    setChildren(els.composerModel, picker.el);
  });
}

function submitComposer() {
  const text = els.input.value.trim();
  if (!text && !ui.images.length) return;
  const agentId = state.route.name === "agent" ? state.route.id : null;
  const model = agentId ? undefined : ui.newModel ?? state.defaultModel ?? undefined;
  if (!submitPrompt(text, ui.images, agentId, model)) return;
  if (text) {
    ui.sent = [text, ...ui.sent.filter((t) => t !== text)].slice(0, HISTORY_LIMIT);
    ui.recall = -1;
  }
  els.input.value = "";
  ui.images = [];
  renderAttachments();
  autosize();
  els.input.focus();
  ui.transcript?.jumpToEnd();
}

function autosize() {
  if (!els.input.isConnected) return;
  els.input.style.height = "auto";
  els.input.style.height = `${Math.min(els.input.scrollHeight, window.innerHeight * 0.4)}px`;
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
      ui.images.push({ media_type: file.type, data: await readBase64(file), name: file.name });
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
    ui.images.map((image, index) =>
      h(
        "div",
        { class: "attachment" },
        h("img", { src: `data:${image.media_type};base64,${image.data}`, alt: image.name ?? "Attached image" }),
        h(
          "button",
          {
            class: "attachment-remove",
            type: "button",
            "aria-label": `Remove ${image.name ?? "image"}`,
            onclick: () => {
              ui.images.splice(index, 1);
              renderAttachments();
              els.input.focus();
            },
          },
          icon("x", 12),
        ),
      ),
    ),
  );
  els.attachments.hidden = ui.images.length === 0;
}

function bindComposer() {
  setChildren(els.attach, icon("image", 16));
  setChildren(els.send, icon("send", 16));
  setChildren(els.stop, icon("stop", 14), h("span", {}, "Stop"));
  els.composer.addEventListener("submit", (event) => {
    event.preventDefault();
    submitComposer();
  });
  els.input.addEventListener("keydown", (event) => {
    if (event.isComposing) return;
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      submitComposer();
    } else if (event.key === "ArrowUp" && !els.input.value && ui.sent.length) {
      event.preventDefault();
      ui.recall = Math.min(ui.recall + 1, ui.sent.length - 1);
      els.input.value = ui.sent[ui.recall];
      autosize();
    } else if (event.key === "ArrowDown" && ui.recall >= 0 && els.input.selectionStart === els.input.value.length) {
      event.preventDefault();
      ui.recall -= 1;
      els.input.value = ui.recall >= 0 ? ui.sent[ui.recall] : "";
      autosize();
    } else if (event.key === "Escape") {
      const agent = selectedAgent();
      if (!els.input.value && agent?.live && agent.status === "running") {
        event.preventDefault();
        stopAgent(agent);
      } else {
        els.input.blur();
      }
    }
  });
  els.input.addEventListener("input", autosize);
  els.input.addEventListener("paste", (event) => {
    const files = filesFrom(event.clipboardData);
    if (files.length) addImages(files);
  });
  els.attach.addEventListener("click", () => els.fileInput.click());
  els.fileInput.addEventListener("change", () => {
    addImages([...els.fileInput.files]);
    els.fileInput.value = "";
  });
  els.stop.addEventListener("click", () => stopAgent(selectedAgent()));
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
  for (const { text, agent } of state.inbox.splice(0)) {
    const target = agent ?? (state.route.name === "agent" ? state.route.id : null);
    submitPrompt(text, [], target, target ? undefined : ui.newModel ?? state.defaultModel ?? undefined);
  }
}

// Keyboard shortcuts

const SHORTCUTS = [
  { keys: ["N"], alt: ["Ctrl", "Shift", "O"], label: "New session" },
  { keys: ["/"], label: "Focus the message box" },
  { keys: ["Esc"], label: "Stop the running agent (from an empty message box)" },
  { keys: ["A"], label: "Agents" },
  { keys: ["H"], label: "Home" },
  { keys: [","], alt: ["Ctrl", ","], label: "Settings" },
  { keys: ["M"], label: "Cycle the permission mode" },
  { keys: ["T"], label: "Switch between light and dark" },
  { keys: ["["], label: "Show or hide the sidebar" },
  { keys: ["Ctrl", "K"], label: "Filter sessions" },
  { keys: ["↑"], label: "Previous message (in an empty message box)" },
  { keys: ["?"], label: "This help" },
];

const isMac = /Mac|iPhone|iPad/.test(navigator.platform);

function keyCaps(keys) {
  return keys.map((key) => h("kbd", {}, key === "Ctrl" && isMac ? "⌘" : key));
}

function showHelp() {
  dialog({
    title: "Keyboard shortcuts",
    className: "help",
    body: h(
      "div",
      {},
      h(
        "div",
        { class: "about" },
        pigLogo(32),
        h("div", {}, h("p", { class: "about-name" }, "malac", state.version ? h("span", { class: "muted" }, ` v${state.version}`) : null), h("p", { class: "muted" }, TAGLINE)),
      ),
      h(
        "table",
        { class: "table compact shortcuts" },
        h(
          "tbody",
          {},
          SHORTCUTS.map((s) =>
            h("tr", {}, h("td", {}, s.label), h("td", { class: "num" }, keyCaps(s.keys), s.alt ? [h("span", { class: "muted" }, " or "), keyCaps(s.alt)] : null)),
          ),
        ),
      ),
      h("p", { class: "muted small" }, "Single-key shortcuts work when no text field has focus."),
    ),
    buttons: [{ label: "Close", value: null }],
  });
}

function typingIn(target) {
  return target instanceof HTMLElement && (target.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(target.tagName));
}

function cycleMode() {
  const index = MODES.findIndex((m) => m.mode === state.mode);
  const next = MODES[(index + 1) % MODES.length];
  requestMode(next.mode);
  toast(`Mode: ${next.label}. ${next.help}.`);
}

function bindShortcuts() {
  document.addEventListener("keydown", (event) => {
    if (event.defaultPrevented || event.isComposing) return;
    if (document.querySelector(".modal-backdrop")) return;
    const mod = event.ctrlKey || event.metaKey;
    if (mod && event.shiftKey && event.key.toLowerCase() === "o") {
      event.preventDefault();
      return newSession();
    }
    if (mod && event.key === ",") {
      event.preventDefault();
      return go({ name: "settings" });
    }
    if (mod && event.key.toLowerCase() === "k") {
      event.preventDefault();
      if (narrowScreen.matches || !ui.sidebarOpen) setSidebar(true);
      els.sessionFilter.focus();
      return;
    }
    if (mod || event.altKey || typingIn(event.target)) return;
    const actions = {
      n: newSession,
      "/": () => els.input.focus(),
      a: () => go({ name: "agents" }),
      h: () => go({ name: "home" }),
      ",": () => go({ name: "settings" }),
      m: cycleMode,
      t: toggleTheme,
      "[": () => setSidebar(!ui.sidebarOpen, { remember: true }),
      "?": showHelp,
    };
    const action = actions[event.key.toLowerCase()] ?? actions[event.key];
    if (action) {
      event.preventDefault();
      action();
      return;
    }
    if (event.key === "Escape") {
      const agent = selectedAgent();
      if (narrowScreen.matches && ui.sidebarOpen) {
        event.preventDefault();
        setSidebar(false);
        els.toggleSidebar.focus();
      } else if (agent?.live && agent.status === "running") {
        event.preventDefault();
        stopAgent(agent);
      }
    }
  });
}

// Wiring

function bindChrome() {
  setChildren(els.brandLogo, pigLogo(22));
  setChildren(els.collapseSidebar, icon("sidebar", 16));
  setChildren(els.toggleSidebar, icon("sidebar", 16));
  setChildren(els.newSession, icon("plus", 16), h("span", {}, "New session"), h("kbd", { class: "hint-key" }, "N"));
  setChildren(els.helpButton, icon("keyboard", 16));
  setChildren(els.settingsButton, icon("settings", 16));
  renderThemeButton();
  els.collapseSidebar.addEventListener("click", () => setSidebar(false, { remember: true }));
  els.toggleSidebar.addEventListener("click", () => setSidebar(!ui.sidebarOpen, { remember: true }));
  els.backdrop.addEventListener("click", () => setSidebar(false));
  els.newSession.addEventListener("click", newSession);
  els.themeToggle.addEventListener("click", toggleTheme);
  els.helpButton.addEventListener("click", showHelp);
  els.updateButton.addEventListener("click", openUpdatePopover);
  els.jump.addEventListener("click", () => ui.transcript?.jumpToEnd());
  els.sessionScope.addEventListener("click", (event) => {
    const scope = event.target.closest("button")?.dataset.scope;
    if (!scope) return;
    ui.sessionScope = scope;
    local.set("malac.sessionScope", scope === "project" ? "project" : null);
    invalidate("sidebar");
  });
  els.sessionFilter.addEventListener("input", () => {
    ui.sessionFilter = els.sessionFilter.value;
    invalidate("sidebar");
  });
  els.sessionFilter.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      els.sessionFilter.value = "";
      ui.sessionFilter = "";
      invalidate("sidebar");
      els.sessionFilter.blur();
    }
  });
  document.addEventListener("malac:theme", (event) => applyTheme(event.detail));
  darkScheme.addEventListener("change", renderThemeButton);
  narrowScreen.addEventListener("change", (event) => setSidebar(event.matches ? false : !embedded && local.get("malac.sidebar") !== "closed"));
  wideMain.addEventListener("change", () => invalidate("agents"));
  window.addEventListener("hashchange", onHashChange);
  window.addEventListener("beforeunload", (event) => {
    if (ui.settings?.isDirty()) event.preventDefault();
  });
}

function registerRenderers() {
  registerRenderer("sidebar", renderSidebar);
  registerRenderer("connection", renderConnection);
  registerRenderer("mode", () => {
    els.app.classList.toggle("autopilot", state.mode === "autopilot");
    invalidate("header");
  });
  registerRenderer("header", renderHeader);
  registerRenderer("permissions", renderPermissions);
  registerRenderer("composer", renderComposer);
  registerRenderer("title", renderTitle);
  registerRenderer("jump", renderJump);
  registerRenderer("update", renderUpdate);
  registerRenderer("home", () => ui.home?.render());
  registerRenderer("agents", () => ui.agents?.render());
  registerRenderer("resources", () => {
    ui.home?.render();
    // Refresh settings with new status, unless someone is typing there.
    const typing = ui.settings?.root.contains(document.activeElement) && typingIn(document.activeElement);
    if (state.route.name === "settings" && ui.settings && !ui.settings.isDirty() && !typing) ui.settings.render();
    invalidate("header");
  });
}

function fatal(message) {
  els.composerDock.hidden = true;
  setChildren(
    els.content,
    h(
      "div",
      { class: "view-scroll" },
      h("div", { class: "fatal" }, pigLogo(40), h("h1", { class: "page-title" }, "Cannot connect"), h("p", { class: "muted" }, message)),
    ),
  );
}

async function init() {
  registerRenderers();
  bindChrome();
  bindComposer();
  bindEditorBridge();
  bindShortcuts();
  els.app.classList.toggle("embed", embedded);
  setSidebar(!narrowScreen.matches && ui.sidebarOpen);
  setNavigator(go);
  state.token = readToken();
  if (!state.token) {
    fatal("Open the address printed by malac web; it includes the access token.");
    return;
  }
  try {
    await loadState();
  } catch (error) {
    if (error.status === 401) {
      fatal("The access token was not accepted. Open the address printed by malac web again.");
      return;
    }
    toast(error.message, "error");
  }
  ensure("settings");
  setResyncHandler(() => {
    if (state.route.name === "agent") showAgent(state.route.id);
    deliverInbox();
  });
  document.addEventListener("malac:reloaded", () => ui.settings?.reloaded());
  show(routeFromHash());
  currentHash = location.hash;
  // Focus the composer on load, except on touch screens where it would open the keyboard.
  if ((state.route.name === "home" || state.route.name === "agent") && !matchMedia("(pointer: coarse)").matches) els.input.focus();
  connect();
}

init();

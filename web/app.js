// App shell: routing, sidebar, tab bar, top bar, composer, approvals,
// banners and shortcuts. State comes from store.js (the daemon snapshot
// and its deltas); views render into #content.

import {
  baseName,
  closePopover,
  confirmDialog,
  dateGroup,
  dialog,
  fullTime,
  h,
  local,
  plural,
  popover,
  relativeTime,
  setChildren,
  toast,
} from "./dom.js";
import { icon, pigLogo } from "./icons.js";
import { jobsView } from "./jobs.js";
import { SettingsPage } from "./settings.js";
import {
  STATUS_TEXT,
  activateAgent,
  activateTab,
  byCreated,
  changeModel,
  closeAgent,
  compactAgent,
  connect,
  currentTab,
  decide,
  deleteSession,
  displayName,
  ensure,
  ensureAgent,
  invalidate,
  lineage,
  params,
  readToken,
  reconnectNow,
  registerRenderer,
  renderKeyed,
  requestMode,
  resumeSession,
  selectTab,
  selectedAgent,
  setNavigator,
  setResyncHandler,
  setTabLostHandler,
  state,
  startAgent,
  stopAgent,
  submitPrompt,
  tabAgents,
  tabById,
  tabOfAgent,
  todoProgress,
  waitingAgents,
} from "./store.js";
import { TranscriptView } from "./transcript.js";
import { TAGLINE, agentsView, controlSummary, homeView, refreshUpdatePanels, updatePanel } from "./views.js";
import { MODES, modeSwitch, modelPicker, statusDot, usageMeter } from "./widgets.js";
import { openAttachPicker, openFolderPicker, openWorkspaceMenu, renderTabBar, workspaceButton } from "./workspace.js";

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
  workspaceSlot: $("workspace-slot"),
  newAgent: $("new-agent"),
  newTab: $("new-tab"),
  nav: [...document.querySelectorAll(".nav-link")],
  tabSection: $("tab-section"),
  tabSectionTitle: $("tab-section-title"),
  tree: $("agent-tree"),
  sessionScope: $("session-scope"),
  sessionFilter: $("session-filter"),
  sessions: $("sessions"),
  daemonStatus: $("daemon-status"),
  version: $("version"),
  tabs: $("tabs"),
  breadcrumb: $("breadcrumb"),
  topbarStatus: $("topbar-status"),
  topbarControls: $("topbar-controls"),
  updateButton: $("update-button"),
  themeToggle: $("theme-toggle"),
  helpButton: $("help-button"),
  settingsButton: $("settings-button"),
  banners: $("banners"),
  permissions: $("permissions"),
  content: $("content"),
  composerDock: $("composer-dock"),
  composer: $("composer"),
  attachments: $("attachments"),
  input: $("input"),
  attach: $("attach"),
  attachPath: $("attach-path"),
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
  jobs: null,
  settings: null,
  images: [],
  paths: [],
  sent: [],
  recall: -1,
  collapsed: new Set(),
  sessionScope: local.get("malac.sessionScope") === "project" ? "project" : "all",
  sessionFilter: "",
  newModel: null,
  sidebarOpen: !embedded && local.get("malac.sidebar") !== "closed",
  dismissed: new Set(),
  sending: false,
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

// Routing. Hashes: #tab=<id>, #tab=<id>&agent=<id>, #agent=<id>, #agents,
// #jobs, #settings[=section]; an empty hash is the selected tab.

function routeFromHash() {
  const hash = location.hash.replace(/^#/, "");
  const query = new URLSearchParams(hash);
  if (query.has("tab")) return { name: "tab", tab: query.get("tab"), agent: query.get("agent") || null, fresh: query.has("new") };
  let match = /^agent=([\w-]+)$/.exec(hash);
  if (match) return { name: "agent", id: match[1] };
  if (hash === "agents") return { name: "agents" };
  if (hash === "jobs") return { name: "jobs" };
  match = /^settings(?:=([\w-]+))?$/.exec(hash);
  if (match) return { name: "settings", section: match[1] ?? "general" };
  return { name: "home" };
}

function hashFor(route) {
  switch (route.name) {
    case "tab":
      return route.agent ? `#tab=${route.tab}&agent=${route.agent}` : route.fresh ? `#tab=${route.tab}&new` : `#tab=${route.tab}`;
    case "agent":
      return `#agent=${route.id}`;
    case "agents":
      return "#agents";
    case "jobs":
      return "#jobs";
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
  // go() handles a change right away; the hashchange event that follows is a repeat.
  if (location.hash === currentHash) return;
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

/** Resolves a route against the workspace: home becomes the selected tab. */
function resolve(route) {
  if (route.name === "home") {
    const tab = currentTab();
    return tab ? { name: "tab", tab: tab.id, agent: defaultAgent(tab) } : route;
  }
  if (route.name === "tab") {
    const tab = tabById(route.tab);
    if (!tab) return null;
    // A tab opens on its active agent; "new" opens its home for a new agent.
    if (!route.agent && !route.fresh) return { ...route, agent: defaultAgent(tab) };
    return route;
  }
  if (route.name === "agent") {
    const tab = tabOfAgent(route.id);
    return tab ? { name: "tab", tab: tab.id, agent: route.id } : route;
  }
  return route;
}

// The tab's active agent, if it has said anything yet.
function defaultAgent(tab) {
  const id = tab.active_agent ?? tab.agents.at(-1);
  return id && tab.agents.includes(id) ? id : null;
}

function show(requested) {
  const route = resolve(requested);
  if (!route) {
    // A tab that is gone, e.g. closed elsewhere: go to the selected one.
    history.replaceState(null, "", `${location.pathname}${location.search}`);
    currentHash = "";
    show({ name: "home" });
    return;
  }
  // Keep the address canonical, so a reload comes back to the same place.
  if (hashFor(route) !== location.hash && (route.name === "tab" || requested.name === "agent")) {
    history.replaceState(null, "", `${location.pathname}${location.search}${hashFor(route)}`);
    currentHash = location.hash;
  }
  const kind = route.name === "tab" ? (route.agent ? "agent" : "home") : route.name;
  const sameView = ui.view === kind && kind !== "agent" && kind !== "home";
  const composerFocused = document.activeElement === els.input;
  if (!sameView) teardown();
  state.route = route;
  ui.view = kind;
  els.app.dataset.view = kind;
  if (route.name === "tab") {
    if (state.tab !== route.tab) {
      selectTab(route.tab);
      ui.paths = [];
      renderAttachments();
    }
    activateTab(route.tab);
    if (route.agent) activateAgent(route.tab, route.agent);
  }
  state.selected = kind === "agent" ? route.agent ?? route.id : null;
  switch (kind) {
    case "agent":
      showAgent(state.selected);
      break;
    case "agents":
      if (!sameView) {
        ui.agents = agentsView({ isWide: () => wideMain.matches });
        mount(ui.agents.root);
      }
      ui.agents.render();
      break;
    case "jobs":
      if (!sameView) {
        ui.jobs = jobsView();
        mount(ui.jobs.root);
      }
      ui.jobs.render();
      break;
    case "settings":
      ui.settings ??= new SettingsPage({ onDirtyChange: () => invalidate("title") });
      if (!sameView) mount(ui.settings.root);
      ui.settings.show(route.section);
      break;
    default:
      ui.home = homeView({ composerSlot: h("div", { class: "home-composer" }), onQuickStart: quickStart, onNewTab: newTab });
      mount(h("div", { class: "view-scroll" }, ui.home.root));
      ui.home.root.querySelector(".home-composer").append(els.composer);
      ui.home.render();
  }
  els.composerDock.hidden = kind !== "agent";
  if (kind !== "home" && els.composer.parentElement !== els.composerDock) els.composerDock.append(els.composer);
  autosize();
  // Moving the composer between the tab home and the dock drops focus.
  if (composerFocused && (kind === "home" || kind === "agent")) els.input.focus();
  if (narrowScreen.matches) setSidebar(false);
  invalidate("sidebar", "tabs", "header", "composer", "permissions", "title", "jump", "banners");
}

function teardown() {
  ui.transcript?.dispose();
  ui.transcript = null;
  ui.agents?.dispose();
  ui.agents = null;
  ui.jobs?.dispose();
  ui.jobs = null;
  ui.home = null;
}

function mount(node) {
  setChildren(els.content, node);
}

function showAgent(id) {
  ui.transcript?.dispose();
  const agent = ensureAgent(id);
  const scroller = h("div", { class: "view-scroll transcript-scroll" });
  ui.transcript = new TranscriptView(agent, { scroller });
  scroller.append(ui.transcript.root);
  mount(scroller);
  ui.transcript.load();
}

/** The tab home of the selected tab, with the composer ready for a new agent. */
function newAgent() {
  const tab = currentTab();
  if (!tab) return newTab();
  go({ name: "tab", tab: tab.id, fresh: true });
  requestAnimationFrame(() => els.input.focus());
}

/** Starts an empty agent in the tab (agent_start) and opens it, ready for a first message. */
async function startAgentHere(tab) {
  try {
    const reply = await startAgent(tab.id, ui.newModel ?? tab.options?.model ?? undefined);
    go({ name: "tab", tab: reply.tab ?? tab.id, agent: reply.agent });
    requestAnimationFrame(() => els.input.focus());
  } catch (error) {
    toast(error.message, "error");
  }
}

async function newTab() {
  const tab = await openFolderPicker();
  if (tab) {
    go({ name: "tab", tab, fresh: true });
    requestAnimationFrame(() => els.input.focus());
  }
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
  renderWorkspaceSlot();
  renderNav();
  renderAgentTree();
  renderSessions();
  renderFooter();
  const busy = [...state.agents.values()].some((a) => a.live && a.status === "running");
  els.brandLogo.firstElementChild?.classList.toggle("busy", busy);
  els.newAgent.disabled = !state.loaded;
  els.newTab.disabled = !state.loaded || state.daemon?.attached;
}

function renderWorkspaceSlot() {
  const ws = state.workspace;
  renderKeyed("workspace", JSON.stringify([ws?.id, ws?.name, state.daemon?.attached]), () => {
    setChildren(els.workspaceSlot, state.daemon?.attached ? null : workspaceButton());
  });
}

function renderNav() {
  const running = [...state.agents.values()].filter((a) => a.live && a.status === "running").length;
  const waiting = state.pending.size;
  const jobs = [...state.jobs.values()].filter((j) => j.status === "running").length;
  const current = ui.view === "home" ? "home" : ui.view === "agent" ? null : state.route.name;
  renderKeyed("nav", JSON.stringify([current, running, waiting, jobs, currentTab()?.name]), () => {
    const labels = {
      home: ["home", currentTab() ? "Tab home" : "Home"],
      agents: ["agents", "Agents"],
      jobs: ["terminal", "Jobs"],
      settings: ["settings", "Settings"],
    };
    for (const link of els.nav) {
      const route = link.dataset.route;
      const [iconName, label] = labels[route];
      let count = null;
      if (route === "agents" && (running || waiting)) {
        count = h("span", { class: `nav-count${waiting ? " warn" : ""}`, title: waiting ? `${waiting} waiting for approval` : `${running} running` }, String(waiting || running));
      } else if (route === "jobs" && jobs) {
        count = h("span", { class: "nav-count", title: `${plural(jobs, "job")} running` }, String(jobs));
      }
      if (route === "home") link.href = currentTab() ? `#tab=${currentTab().id}&new` : "#";
      setChildren(link, icon(iconName, 16), h("span", {}, label), count);
      if (current === route) link.setAttribute("aria-current", "page");
      else link.removeAttribute("aria-current");
    }
  });
}

function renderAgentTree() {
  const tab = currentTab();
  const waiting = waitingAgents();
  const top = tabAgents(tab);
  const all = [...state.agents.values()];
  const key = JSON.stringify([
    tab?.id,
    tab?.name,
    state.selected,
    [...ui.collapsed],
    [...waiting],
    top.map((a) => a.id),
    all.filter((a) => a.tab === tab?.id || top.includes(a)).map((a) => [a.id, a.parent, a.live, displayName(a), a.status, a.activity, todoProgress(a.todos)]),
  ]);
  renderKeyed("tree", key, () => {
    els.tabSection.hidden = !tab;
    if (!tab) return;
    els.tabSectionTitle.textContent = `In ${tab.name}`;
    setChildren(
      els.tree,
      top.length
        ? top.map((agent) => agentNode(agent, all, waiting, tab))
        : h("li", { class: "sidebar-empty" }, "No agents yet."),
      h(
        "li",
        {},
        h("button", { class: "tree-add", type: "button", onclick: () => startAgentHere(tab) }, icon("plus", 14), h("span", {}, "New agent in this tab")),
      ),
    );
  });
}

function agentNode(agent, all, waiting, tab) {
  const children = all.filter((other) => other.parent === agent.id && other.live).sort(byCreated);
  const collapsed = ui.collapsed.has(agent.id);
  return h(
    "li",
    {},
    agentRow(agent, children.length > 0, collapsed, waiting.has(agent.id), tab),
    children.length > 0 && !collapsed ? h("ul", { class: "tree" }, children.map((child) => agentNode(child, all, waiting, tab))) : null,
  );
}

function agentRow(agent, hasChildren, collapsed, waiting, tab) {
  const progress = todoProgress(agent.todos);
  const running = agent.live && agent.status === "running";
  const status = waiting ? "waiting" : agent.live ? agent.status : "idle";
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
  // The daemon keeps listing loaded agents in their tab, so only saved ones can be removed.
  const removable = !agent.parent && !agent.live && tab;
  return h(
    "div",
    { class: `agent-row${agent.id === state.selected ? " selected" : ""}${agent.live ? "" : " saved"}` },
    twisty,
    h(
      "a",
      {
        class: "agent-open",
        href: `#tab=${tab.id}&agent=${agent.id}`,
        title: `${displayName(agent)}\n${agent.model}${agent.live ? "" : "\nSaved; sending a message resumes it"}`,
        "aria-current": agent.id === state.selected ? "page" : null,
      },
      statusDot(status, waiting ? "Waiting for approval" : agent.live ? STATUS_TEXT[agent.status] : "Saved"),
      h("span", { class: "agent-label" }, displayName(agent)),
      waiting ? h("span", { class: "badge warn" }, "Approval") : progress ? h("span", { class: "agent-progress tabular" }, `${progress.done}/${progress.total}`) : null,
      running && agent.activity ? h("span", { class: "agent-activity" }, agent.activity) : null,
    ),
    removable
      ? h(
          "button",
          {
            class: "icon-button small row-remove",
            type: "button",
            "aria-label": `Remove ${displayName(agent)} from this tab`,
            title: "Remove from this tab (the transcript stays in History)",
            onclick: async () => {
              try {
                await closeAgent(tab.id, agent.id);
                if (state.selected === agent.id) go({ name: "tab", tab: tab.id });
              } catch (error) {
                toast(error.message, "error");
              }
            },
          },
          icon("x", 14),
        )
      : null,
  );
}

function renderSessions() {
  const filter = ui.sessionFilter.trim().toLowerCase();
  const inTabs = new Set((state.workspace?.tabs ?? []).flatMap((t) => t.agents));
  const cwd = currentTab()?.cwd;
  const sessions = state.sessions.filter((session) => {
    if (inTabs.has(session.id)) return false;
    if (ui.sessionScope === "project" && session.cwd !== cwd) return false;
    if (!filter) return true;
    return [session.title, session.label, session.model, session.cwd].some((v) => String(v ?? "").toLowerCase().includes(filter));
  });
  const key = JSON.stringify([ui.sessionScope, filter, cwd, sessions.map((s) => [s.id, s.title, s.model, s.updated])]);
  renderKeyed("sessions", key, () => {
    for (const button of els.sessionScope.querySelectorAll("button")) {
      button.setAttribute("aria-pressed", String(button.dataset.scope === ui.sessionScope));
    }
    if (!sessions.length) {
      setChildren(
        els.sessions,
        h("p", { class: "sidebar-empty" }, filter ? "No sessions match." : ui.sessionScope === "project" ? "No other sessions in this folder." : "No other saved sessions."),
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
  const elsewhere = session.cwd !== currentTab()?.cwd;
  const title = session.title || session.label;
  return h(
    "li",
    { class: "session-row" },
    h(
      "a",
      {
        class: "session-open",
        href: `#agent=${session.id}`,
        title: `${title}\n${session.model}\n${session.cwd}\nUpdated ${fullTime(session.updated)}\nOpens in the tab for its folder`,
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
    toast("Session deleted");
  } catch (error) {
    toast(error.message, "error");
  }
}

function renderFooter() {
  const d = state.daemon;
  const control = controlSummary(state.control);
  renderKeyed("footer", JSON.stringify([d?.version, d?.pid, control.text, control.dot]), () => {
    setChildren(
      els.daemonStatus,
      h(
        "a",
        { class: "daemon-link", href: "#settings=about", title: d ? `Daemon ${d.version}, pid ${d.pid}` : "Daemon" },
        icon("server", 14),
        h("span", { class: "ellipsis" }, "Daemon"),
      ),
      h(
        "a",
        { class: "daemon-link", href: "#settings=control", title: `Control service: ${control.text}${control.detail ? `\n${control.detail}` : ""}` },
        h("span", { class: `dot ${control.dot}` }),
        h("span", {}, "Control"),
      ),
    );
    els.version.textContent = d?.version ? `v${d.version}` : "";
  });
}

function renderConnection() {
  const text = { online: "Connected", connecting: "Connecting", offline: "Offline" }[state.connection];
  setChildren(els.connection, h("span", { class: `dot ${state.connection === "online" ? "ok" : state.connection === "offline" ? "failed" : "waiting"}` }));
  els.connection.title = text;
  els.connection.setAttribute("aria-label", text);
}

// Tab bar

function renderTabs() {
  const tabs = state.workspace?.tabs ?? [];
  const waiting = [...state.pending.values()];
  const key = JSON.stringify([
    state.tab,
    state.route.name,
    tabs.map((t) => [t.id, t.name, t.cwd, t.agents, t.active_agent]),
    waiting.map((r) => [r.id, r.tab, r.agent]),
    [...state.agents.values()].filter((a) => a.live).map((a) => [a.id, a.tab, a.status]),
    [...state.jobs.values()].map((j) => [j.id, j.tab, j.status]),
    state.daemon?.attached,
  ]);
  renderKeyed("tabbar", key, () => {
    if (state.daemon?.attached) {
      setChildren(els.tabs);
      return;
    }
    renderTabBar(els.tabs, { onNewTab: newTab });
  });
}

// Top bar

function renderHeader() {
  const agent = selectedAgent();
  renderBreadcrumb(agent);
  renderTopbarStatus(agent);
  renderTopbarControls(agent);
}

function renderBreadcrumb(agent) {
  const tab = state.route.name === "tab" ? currentTab() : null;
  const chain = agent ? lineage(agent) : [];
  const key = JSON.stringify([state.route.name, state.route.section, tab?.id, tab?.name, chain.map((a) => [a.id, displayName(a)])]);
  renderKeyed("breadcrumb", key, () => {
    const crumb = (text, title) => h("span", { class: "crumb current", "aria-current": "page", title }, text);
    const sep = () => h("span", { class: "crumb-sep", "aria-hidden": "true" }, icon("chevronRight", 14));
    if (!tab && !agent) {
      const titles = { home: "Welcome", agents: "Agents", jobs: "Jobs", settings: "Settings" };
      setChildren(els.breadcrumb, crumb(titles[state.route.name] ?? "malac"));
      return;
    }
    const parts = [];
    if (tab) parts.push(agent ? h("a", { class: "crumb", href: `#tab=${tab.id}`, title: tab.cwd }, tab.name) : crumb(tab.name, tab.cwd));
    chain.forEach((item, index) => {
      const last = index === chain.length - 1;
      const href = tab ? `#tab=${tab.id}&agent=${item.id}` : `#agent=${item.id}`;
      parts.push(last ? crumb(displayName(item), item.id) : h("a", { class: "crumb", href }, displayName(item)));
    });
    if (chain[0]?.parent) parts.splice(tab ? 1 : 0, 0, h("a", { class: "crumb", href: `#agent=${chain[0].parent}` }, "Parent"));
    if (tab && !agent) parts.push(h("span", { class: "crumb-note" }, "New agent"));
    setChildren(els.breadcrumb, parts.flatMap((node, i) => (i ? [sep(), node] : [node])));
  });
}

function renderTopbarStatus(agent) {
  const waiting = agent && waitingAgents().has(agent.id);
  const activity = agent?.live && agent.status === "running" && !waiting ? agent.activity : "";
  const status = !agent ? null : agent.live ? agent.status : "saved";
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
  const tab = currentTab();
  const key = JSON.stringify([state.route.name, agent?.id, agent?.live, running, agent?.model, agent?.usage, agent?.contextTokens, state.mode, tab?.id, state.models]);
  renderKeyed("controls", key, () => {
    const items = [];
    if (agent) {
      const picker = modelPicker({
        value: agent.model,
        title: "Model for the next turn",
        align: "end",
        className: "topbar-model",
        onChange: (spec) => spec && changeModel(agent, spec),
      });
      items.push(picker.el);
    }
    if (tab && (state.route.name === "tab" || agent)) items.push(modeSwitch(requestMode, { compact: true }));
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

// Banners: connection, daemon shutdown and version skew.

function renderBanners() {
  const d = state.daemon;
  const items = [];
  if (state.shutdown && state.connection !== "online") {
    items.push(
      banner("warning", `The daemon stopped (${state.shutdown}). This page reconnects when it runs again.`, {
        label: "Retry now",
        run: reconnectNow,
      }),
    );
  } else if (state.connection === "offline") {
    items.push(banner("warning", "Lost the connection to malac. Reconnecting…", { label: "Retry now", run: reconnectNow }));
  }
  // This page asks for the version it was loaded with. An older daemon
  // should be restarted when idle; a newer one serves a newer page.
  const skew = d?.version && state.pageVersion && d.version !== state.pageVersion ? compareVersions(d.version, state.pageVersion) : 0;
  if (skew > 0 && !ui.dismissed.has(`page:${d.version}`)) {
    items.push(
      banner("info", `malac ${d.version} is running now; this page is from ${state.pageVersion}. Reload to use its interface.`, { label: "Reload", run: () => location.reload() }, `page:${d.version}`),
    );
  } else if ((skew < 0 || d?.version_mismatch) && !ui.dismissed.has(`daemon:${d.version}`)) {
    items.push(banner("info", `Daemon ${d.version} is running; restart it when idle with malac daemon restart.`, null, `daemon:${d.version}`));
  }
  setChildren(els.banners, items);
}

/** Compares dotted versions: positive when `a` is newer. */
function compareVersions(a, b) {
  const pa = String(a).split(/[.-]/).map((n) => parseInt(n, 10) || 0);
  const pb = String(b).split(/[.-]/).map((n) => parseInt(n, 10) || 0);
  for (let i = 0; i < Math.max(pa.length, pb.length); i += 1) {
    if ((pa[i] ?? 0) !== (pb[i] ?? 0)) return (pa[i] ?? 0) - (pb[i] ?? 0);
  }
  return 0;
}

function banner(level, text, action, dismissKey) {
  return h(
    "div",
    { class: `top-banner ${level}`, role: level === "warning" ? "alert" : "status" },
    h("span", { class: `dot ${level === "warning" ? "waiting" : "idle"}` }),
    h("span", { class: "top-banner-text" }, text),
    action ? h("button", { class: "button small", type: "button", onclick: action.run }, action.label) : null,
    dismissKey
      ? h(
          "button",
          {
            class: "icon-button small",
            type: "button",
            "aria-label": "Dismiss",
            onclick: () => {
              ui.dismissed.add(dismissKey);
              renderBanners();
            },
          },
          icon("x", 14),
        )
      : null,
  );
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

// Approvals not shown inline in a visible transcript get a banner.

function renderPermissions() {
  const requests = [...state.pending.values()].filter((r) => ![...state.views].some((view) => view.showsRequest(r.id)));
  renderKeyed("permissions", JSON.stringify(requests.map((r) => [r.id, r.tab])), () => {
    setChildren(els.permissions, requests.map(permissionCard));
  });
  invalidate("title");
}

function permissionCard(request) {
  const agent = state.agents.get(request.agent);
  const label = agent ? displayName(agent) : request.agent_label ?? "agent";
  const tab = request.tab ? tabById(request.tab) : tabOfAgent(request.agent);
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
  const href = tab ? `#tab=${tab.id}&agent=${request.agent}` : `#agent=${request.agent}`;
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
        h(
          "p",
          { class: "permission-title" },
          h("a", { href }, label),
          tab ? h("span", { class: "muted" }, ` in ${tab.name}`) : null,
          " wants to run ",
          h("strong", {}, request.tool),
        ),
        request.summary ? h("p", { class: "permission-summary", title: request.summary }, request.summary) : null,
      ),
    ),
    actions,
  );
}

function renderTitle() {
  const agent = selectedAgent();
  const tab = state.route.name === "tab" ? currentTab() : null;
  const pending = state.pending.size;
  const base =
    agent
      ? `${displayName(agent)} · ${tab ? `${tab.name} · ` : ""}malac`
      : tab
        ? `${tab.name} · malac`
        : state.route.name === "agents"
          ? "Agents · malac"
          : state.route.name === "jobs"
            ? "Jobs · malac"
            : state.route.name === "settings"
              ? `Settings${ui.settings?.isDirty() ? " (unsaved)" : ""} · malac`
              : `malac · ${TAGLINE}`;
  document.title = pending ? `(${pending}) ${base}` : base;
}

function renderJump() {
  els.jump.hidden = !(ui.transcript && !ui.transcript.pinned);
}

// Composer

function renderComposer() {
  const agent = selectedAgent();
  const tab = currentTab();
  const running = agent?.live && agent.status === "running";
  els.stop.hidden = !running;
  els.attachPath.disabled = !tab;
  if (!agent) els.input.placeholder = tab ? `Describe a task for a new agent in ${tab.name}` : "Open a folder first";
  else if (agent.live && !agent.title && !agent.parent) els.input.placeholder = `Describe a task for this agent in ${tab?.name ?? "this tab"}`;
  else if (agent.live) els.input.placeholder = agent.parent ? `Message ${agent.label}` : "Send a follow-up";
  else els.input.placeholder = "Send a message to continue this session";
  const fallback = tab?.options?.model ?? state.defaultModel;
  renderKeyed("composer-model", JSON.stringify([Boolean(agent), ui.newModel, fallback, state.models]), () => {
    if (agent) {
      setChildren(els.composerModel);
      return;
    }
    const picker = modelPicker({
      value: ui.newModel ?? fallback,
      title: "Model for the new agent",
      onChange: (spec) => {
        ui.newModel = spec;
      },
    });
    setChildren(els.composerModel, picker.el);
  });
}

async function submitComposer() {
  const text = els.input.value.trim();
  if ((!text && !ui.images.length && !ui.paths.length) || ui.sending) return;
  const agent = selectedAgent();
  const tab = currentTab();
  const images = ui.images.map(({ media_type, data }) => ({ media_type, data }));
  const attachments = ui.paths.map((path) => ({ path }));
  const message = {
    text,
    images,
    attachments,
    agent: agent?.id ?? null,
    tab: tab?.id ?? null,
    model: agent ? undefined : ui.newModel ?? tab?.options?.model ?? state.defaultModel ?? undefined,
  };
  ui.sending = true;
  els.send.disabled = true;
  try {
    const reply = await submitPrompt(message);
    if (text) {
      ui.sent = [text, ...ui.sent.filter((t) => t !== text)].slice(0, HISTORY_LIMIT);
      ui.recall = -1;
    }
    const notes = (reply.attachments ?? []).filter((a) => a.note).map((a) => `${baseName(a.path)}: ${a.note}`);
    if (notes.length) toast(notes.join("; "), "warning");
    els.input.value = "";
    ui.images = [];
    ui.paths = [];
    renderAttachments();
    autosize();
    if (!agent || reply.agent !== agent.id) go({ name: "tab", tab: reply.tab ?? tab?.id, agent: reply.agent });
    els.input.focus();
    ui.transcript?.jumpToEnd();
  } catch (error) {
    toast(error.message, "error");
  } finally {
    ui.sending = false;
    els.send.disabled = false;
  }
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
      toast(`Only PNG, JPEG, GIF and WebP images can be attached here (${file.name}). Attach other files by path.`, "warning");
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
    ui.paths.map((path, index) =>
      h(
        "span",
        { class: "path-chip", title: path },
        icon(path.endsWith("/") || !/\.[\w]+$/.test(path) ? "folder" : "file", 14),
        h("span", { class: "ellipsis" }, path),
        h(
          "button",
          {
            class: "chip-remove",
            type: "button",
            "aria-label": `Remove ${path}`,
            onclick: () => {
              ui.paths.splice(index, 1);
              renderAttachments();
              els.input.focus();
            },
          },
          icon("x", 12),
        ),
      ),
    ),
  );
  els.attachments.hidden = ui.images.length === 0 && ui.paths.length === 0;
}

function bindComposer() {
  setChildren(els.attach, icon("image", 16));
  setChildren(els.attachPath, icon("filePlus", 16));
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
  els.attachPath.addEventListener("click", () => {
    const tab = currentTab();
    if (!tab) return;
    if (els.attachPath.getAttribute("aria-expanded") === "true") {
      closePopover();
      return;
    }
    openAttachPicker(els.attachPath, {
      cwd: tab.cwd,
      onAdd: (path) => {
        if (!ui.paths.includes(path)) ui.paths.push(path);
        renderAttachments();
      },
    });
  });
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
    if (!event.dataTransfer?.types.includes("Files")) return;
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

async function deliverInbox() {
  if (state.socket?.readyState !== WebSocket.OPEN || !state.loaded) return;
  for (const { text, agent } of state.inbox.splice(0)) {
    const target = agent ?? state.selected;
    try {
      const reply = await submitPrompt({ text, agent: target, tab: target ? null : state.tab });
      if (!target) go({ name: "tab", tab: reply.tab, agent: reply.agent });
    } catch (error) {
      toast(error.message, "error");
    }
  }
}

// Keyboard shortcuts

const SHORTCUTS = [
  { keys: ["N"], alt: ["Ctrl", "Shift", "O"], label: "New agent in this tab" },
  { keys: ["T"], label: "Open a folder in a new tab" },
  { keys: ["1"], to: ["9"], label: "Go to tab 1 to 9" },
  { keys: ["W"], label: "Switch workspace" },
  { keys: ["/"], label: "Focus the message box" },
  { keys: ["Esc"], label: "Leave the message box, or stop the running agent from an empty one" },
  { keys: ["A"], label: "Agents" },
  { keys: ["J"], label: "Jobs" },
  { keys: [","], alt: ["Ctrl", ","], label: "Settings" },
  { keys: ["M"], label: "Cycle this tab's permission mode" },
  { keys: ["D"], label: "Switch between light and dark" },
  { keys: ["["], label: "Show or hide the sidebar" },
  { keys: ["Ctrl", "K"], label: "Filter sessions" },
  { keys: ["↑"], label: "Previous message (in an empty message box)" },
  { keys: ["Alt", "Shift", "←"], label: "Move the focused tab (with →)" },
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
            h(
              "tr",
              {},
              h("td", {}, s.label),
              h("td", { class: "num" }, keyCaps(s.keys), s.to ? [h("span", { class: "muted" }, " to "), keyCaps(s.to)] : null, s.alt ? [h("span", { class: "muted" }, " or "), keyCaps(s.alt)] : null),
            ),
          ),
        ),
      ),
      h("p", { class: "muted small" }, "Single-key shortcuts work when no text field has focus. In the tab bar, arrow keys move between tabs, F2 renames and Delete closes."),
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
      return newAgent();
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
      n: newAgent,
      t: newTab,
      w: () => {
        if (narrowScreen.matches || !ui.sidebarOpen) setSidebar(true);
        const button = els.workspaceSlot.querySelector("button");
        if (button) openWorkspaceMenu(button);
      },
      "/": () => els.input.focus(),
      a: () => go({ name: "agents" }),
      j: () => go({ name: "jobs" }),
      ",": () => go({ name: "settings" }),
      m: cycleMode,
      d: toggleTheme,
      "[": () => setSidebar(!ui.sidebarOpen, { remember: true }),
      "?": showHelp,
    };
    if (/^[1-9]$/.test(event.key)) {
      const tab = state.workspace?.tabs[Number(event.key) - 1];
      if (tab) {
        event.preventDefault();
        go({ name: "tab", tab: tab.id });
      }
      return;
    }
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
  setChildren(els.newAgent, icon("plus", 16), h("span", {}, "New agent"));
  setChildren(els.newTab, icon("folder", 16), h("span", {}, "New tab"));
  setChildren(els.helpButton, icon("keyboard", 16));
  setChildren(els.settingsButton, icon("settings", 16));
  renderThemeButton();
  els.collapseSidebar.addEventListener("click", () => setSidebar(false, { remember: true }));
  els.toggleSidebar.addEventListener("click", () => setSidebar(!ui.sidebarOpen, { remember: true }));
  els.backdrop.addEventListener("click", () => setSidebar(false));
  els.newAgent.addEventListener("click", newAgent);
  els.newTab.addEventListener("click", newTab);
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
  document.addEventListener("malac:reloaded", () => ui.settings?.reloaded());
  document.addEventListener("malac:markers", () => ui.home?.render());
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
  registerRenderer("tabs", renderTabs);
  registerRenderer("mode", () => {
    els.app.classList.toggle("autopilot", state.mode === "autopilot");
    invalidate("header");
  });
  registerRenderer("header", renderHeader);
  registerRenderer("banners", renderBanners);
  registerRenderer("permissions", renderPermissions);
  registerRenderer("composer", renderComposer);
  registerRenderer("title", renderTitle);
  registerRenderer("jump", renderJump);
  registerRenderer("update", renderUpdate);
  registerRenderer("home", () => ui.home?.render());
  registerRenderer("agents", () => ui.agents?.render());
  registerRenderer("jobs", () => {
    ui.jobs?.render();
    ui.jobs?.jobChanged();
  });
  const refreshSettings = () => {
    // Refresh settings with new status, unless someone is typing there.
    const typing = ui.settings?.root.contains(document.activeElement) && typingIn(document.activeElement);
    if (state.route.name === "settings" && ui.settings && !ui.settings.isDirty() && !typing) ui.settings.render();
  };
  registerRenderer("settings", refreshSettings);
  registerRenderer("resources", () => {
    ui.home?.render();
    refreshSettings();
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

let shown = false;

function init() {
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
  // After every snapshot (first connect, reconnect, workspace switch) the
  // current route is shown again from fresh state.
  setResyncHandler(() => {
    show(routeFromHash());
    currentHash = location.hash;
    if (!shown) {
      shown = true;
      // Focus the composer on load, except on touch screens where it would open the keyboard.
      if (!matchMedia("(pointer: coarse)").matches) els.input.focus();
    }
    deliverInbox();
  });
  setTabLostHandler(() => {
    if (state.route.name === "tab") go({ name: "home" });
  });
  ensure("settings");
  api401Check();
  connect();
}

// A wrong token shows up as a failed upgrade; check it once over HTTP.
async function api401Check() {
  try {
    const response = await fetch("/api/daemon", { headers: { Authorization: `Bearer ${state.token}` } });
    if (response.status === 401) fatal("The access token was not accepted. Open the address printed by malac again.");
  } catch {
    // Offline; the socket keeps retrying.
  }
}

init();

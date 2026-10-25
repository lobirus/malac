// The home screen and the agents overview.

import {
  baseName,
  copyButton,
  formatCost,
  formatDuration,
  formatTokens,
  fullTime,
  h,
  plural,
  relativeTime,
  setChildren,
  shortPath,
  toast,
  totalTokens,
} from "./dom.js";
import { icon, pigLogo } from "./icons.js";
import {
  STATUS_TEXT,
  api,
  checkForUpdate,
  installUpdate,
  byCreated,
  contextWindow,
  displayName,
  ensure,
  liveAgents,
  navigate,
  openAgent,
  renderKeyed,
  resource,
  resumeSession,
  state,
  stopAgent,
  todoProgress,
  waitingAgents,
} from "./store.js";
import { TranscriptView } from "./transcript.js";
import { modelLabel, modelPicker, statusDot } from "./widgets.js";

export const TAGLINE = "Multi-Agent Loop for Autonomous Coding";

const QUICK_STARTS = [
  { label: "Explain this codebase", text: "Give me a tour of this codebase: what it does, how it is organized, and where to start reading." },
  { label: "Review my changes", text: "Review the uncommitted changes in this repository. Point out bugs, missing tests and anything unclear." },
  { label: "Find and fix a bug", text: "Run the test suite, find the first failing test, explain the cause and fix it." },
  { label: "Write tests", text: "Find the least tested module in this project and add focused unit tests for it." },
  { label: "Summarize recent work", text: "Summarize the last 20 commits: what changed, and what looks unfinished." },
];

// Home

let instances = 0;

export function homeView({ composerSlot, onQuickStart }) {
  const slot = `home${++instances}`;
  const cards = {
    sessions: h("section", { class: "card", "aria-labelledby": "home-sessions" }),
    models: h("section", { class: "card", "aria-labelledby": "home-models" }),
    services: h("section", { class: "card", "aria-labelledby": "home-services" }),
    memory: h("section", { class: "card", "aria-labelledby": "home-memory" }),
  };
  const project = h("div", { class: "home-project" });
  const agents = h("div", { class: "home-agents" });
  const root = h(
    "div",
    { class: "home" },
    h(
      "header",
      { class: "home-hero" },
      pigLogo(40, "home-logo"),
      h("h1", { class: "home-title" }, "malac"),
      h("p", { class: "home-tagline" }, TAGLINE),
      project,
    ),
    composerSlot,
    h(
      "div",
      { class: "quick-starts", role: "group", "aria-label": "Suggested tasks" },
      QUICK_STARTS.map((item) =>
        h("button", { class: "button quiet", type: "button", title: item.text, onclick: () => onQuickStart(item.text) }, item.label),
      ),
    ),
    agents,
    h("div", { class: "home-grid" }, cards.sessions, cards.models, cards.services, cards.memory),
  );
  return {
    root,
    render() {
      ensure("models");
      ensure("tools");
      ensure("memory");
      ensure("settings");
      renderProject(slot, project);
      renderLiveAgents(slot, agents);
      renderSessionsCard(slot, cards.sessions);
      renderModelsCard(slot, cards.models);
      renderServicesCard(slot, cards.services);
      renderMemoryCard(slot, cards.memory);
    },
  };
}

function renderProject(slot, node) {
  renderKeyed(`${slot}:project`, JSON.stringify([state.cwd, state.version]), () => {
    setChildren(
      node,
      icon("folder", 14),
      h("span", { class: "home-project-name" }, baseName(state.cwd) || "No project"),
      h("span", { class: "home-project-path", title: state.cwd }, shortPath(state.cwd, 3)),
    );
  });
}

function renderLiveAgents(slot, node) {
  const agents = liveAgents();
  const running = agents.filter((a) => a.status === "running");
  const waiting = waitingAgents();
  renderKeyed(`${slot}:agents`, JSON.stringify([running.length, agents.length, waiting.size]), () => {
    if (!running.length && !waiting.size) {
      setChildren(node);
      node.hidden = true;
      return;
    }
    node.hidden = false;
    setChildren(
      node,
      statusDot("running"),
      h("span", {}, `${plural(running.length, "agent")} working`, waiting.size ? `, ${waiting.size} waiting for approval` : ""),
      h("button", { class: "link", type: "button", onclick: () => navigate({ name: "agents" }) }, "Open agents"),
    );
  });
}

function cardHead(id, title, action) {
  return h("header", { class: "card-head" }, h("h2", { id, class: "card-title" }, title), action ?? null);
}

function emptyRow(text) {
  return h("p", { class: "empty-note" }, text);
}

function renderSessionsCard(slot, card) {
  const here = state.sessions.filter((s) => s.cwd === state.cwd);
  const elsewhere = here.length === 0;
  const sessions = (elsewhere ? state.sessions : here).slice(0, 8);
  renderKeyed(`${slot}:sessions`, JSON.stringify([elsewhere, sessions.map((s) => [s.id, s.title, s.model, s.updated])]), () => {
    setChildren(
      card,
      cardHead("home-sessions", elsewhere ? "Recent sessions" : "Recent sessions in this project"),
      elsewhere && sessions.length ? h("p", { class: "card-summary" }, "None in this project yet. These are from other projects.") : null,
      sessions.length
        ? h(
            "table",
            { class: "table compact clickable" },
            h("thead", {}, h("tr", {}, h("th", {}, "Session"), h("th", { class: "hide-narrow" }, elsewhere ? "Project" : "Model"), h("th", { class: "num" }, "Updated"))),
            h(
              "tbody",
              {},
              sessions.map((session) =>
                h(
                  "tr",
                  {},
                  h(
                    "td",
                    { class: "cell-main" },
                    h(
                      "button",
                      { class: "row-link", type: "button", title: session.title || session.label, onclick: () => resumeSession(session) },
                      session.title || session.label,
                    ),
                  ),
                  h(
                    "td",
                    { class: "hide-narrow cell-model" },
                    elsewhere ? h("span", { class: "muted", title: session.cwd }, baseName(session.cwd)) : modelLabel(session.model),
                  ),
                  h("td", { class: "num muted", title: fullTime(session.updated) }, relativeTime(session.updated)),
                ),
              ),
            ),
          )
        : emptyRow("No sessions in this project yet. Describe a task above to start one."),
    );
  });
}

function renderModelsCard(slot, card) {
  const models = resource("models");
  const settings = resource("settings");
  const key = JSON.stringify([models.data, models.loading, models.error, state.defaultModel, settings.data?.reloadable]);
  renderKeyed(`${slot}:models`, key, () => {
    const picker = modelPicker({
      value: state.defaultModel,
      title: "Default model for new sessions",
      align: "end",
      onChange: (spec) => saveDefaultModel(spec),
    });
    const providers = models.data?.providers ?? [];
    setChildren(
      card,
      cardHead("home-models", "Models and providers"),
      h("div", { class: "card-row" }, h("span", { class: "muted" }, "Default model"), picker.el),
      models.loading && !providers.length
        ? h("p", { class: "empty-note" }, h("span", { class: "spinner small" }), "Asking providers for their models")
        : models.error && !providers.length
          ? h("p", { class: "empty-note error" }, models.error)
          : h(
              "table",
              { class: "table compact" },
              h(
                "thead",
                {},
                h("tr", {}, h("th", {}, "Provider"), h("th", {}, "Status"), h("th", { class: "num" }, "Models")),
              ),
              h(
                "tbody",
                {},
                providers.map((p) =>
                  h(
                    "tr",
                    {},
                    h("td", { class: "cell-main" }, h("span", { class: "strong" }, p.provider), h("span", { class: "muted small" }, ` ${p.kind}`)),
                    h("td", { class: "cell-status" }, providerStatus(p)),
                    h("td", { class: "num" }, String(p.available?.length || p.configured?.length || 0)),
                  ),
                ),
              ),
            ),
      h(
        "footer",
        { class: "card-foot" },
        h("button", { class: "link", type: "button", onclick: () => navigate({ name: "settings", section: "providers" }) }, "Manage providers"),
      ),
    );
  });
}

export function providerStatus(p) {
  if (p.error === "no API key") return h("span", { class: "status warn", title: "Set a key in Settings, Providers" }, statusDot("waiting"), "No key");
  if (p.error) return h("span", { class: "status bad", title: p.error }, statusDot("failed"), h("span", { class: "ellipsis" }, p.error));
  return h("span", { class: "status ok" }, statusDot("ok"), "Responding");
}

async function saveDefaultModel(spec) {
  const settings = resource("settings").data ?? (await ensure("settings").promise) ?? resource("settings").data;
  if (!settings) {
    toast("Could not load the settings", "error");
    return;
  }
  const user = { ...(settings.user ?? {}), model: spec };
  try {
    const result = await api("/api/settings", { method: "PUT", body: { user } });
    if (result.applied) toast(`Default model is now ${spec}`, "success");
    else toast(result.message ?? "Saved; apply the settings to use it", "warning");
  } catch (error) {
    toast(error.message, "error");
  }
}

function renderServicesCard(slot, card) {
  const tools = resource("tools");
  const settings = resource("settings");
  const key = JSON.stringify([tools.data, tools.error, settings.data?.effective?.control, settings.data?.effective?.mcp]);
  renderKeyed(`${slot}:services`, key, () => {
    const servers = tools.data?.servers ?? state.servers;
    const list = tools.data?.tools ?? [];
    const kinds = { safe: 0, edit: 0, execute: 0 };
    for (const tool of list) kinds[tool.kind] = (kinds[tool.kind] ?? 0) + 1;
    const controlEnabled = settings.data?.effective?.control?.enabled;
    const control = servers.find((s) => s.name === "control");
    const mcp = servers.filter((s) => s.name !== "control");
    const rows = [
      h(
        "tr",
        {},
        h("td", { class: "cell-main" }, h("span", { class: "strong" }, "Control service"), h("span", { class: "muted small" }, " browser and computer")),
        h(
          "td",
          { class: "cell-status" },
          control
            ? control.error
              ? h("span", { class: "status bad", title: control.error }, statusDot("failed"), "Unavailable")
              : h("span", { class: "status ok" }, statusDot("ok"), "Connected")
            : h("span", { class: "status muted" }, statusDot("idle"), controlEnabled === false ? "Disabled" : "Not running"),
        ),
        h("td", { class: "num" }, control && !control.error ? String(control.tools) : "–"),
      ),
      ...mcp.map((server) =>
        h(
          "tr",
          {},
          h("td", { class: "cell-main" }, h("span", { class: "strong" }, server.name), h("span", { class: "muted small" }, " MCP")),
          h(
            "td",
            { class: "cell-status" },
            server.error
              ? h("span", { class: "status bad", title: server.error }, statusDot("failed"), "Unavailable")
              : h("span", { class: "status ok" }, statusDot("ok"), "Connected"),
          ),
          h("td", { class: "num" }, server.error ? "–" : String(server.tools)),
        ),
      ),
    ];
    setChildren(
      card,
      cardHead("home-services", "Tools and services"),
      h(
        "p",
        { class: "card-summary" },
        list.length
          ? `${plural(list.length, "tool")}: ${kinds.safe} read-only, ${kinds.edit} editing, ${kinds.execute} executing`
          : tools.loading
            ? "Loading tools"
            : "No tools loaded",
      ),
      h(
        "table",
        { class: "table compact" },
        h("thead", {}, h("tr", {}, h("th", {}, "Service"), h("th", {}, "Status"), h("th", { class: "num" }, "Tools"))),
        h("tbody", {}, rows),
      ),
      mcp.length ? null : h("p", { class: "empty-note" }, "No MCP servers configured."),
      h(
        "footer",
        { class: "card-foot" },
        h("button", { class: "link", type: "button", onclick: () => navigate({ name: "settings", section: "mcp" }) }, "MCP servers"),
        h("button", { class: "link", type: "button", onclick: () => navigate({ name: "settings", section: "control" }) }, "Control service"),
      ),
    );
  });
}

function renderMemoryCard(slot, card) {
  const memory = resource("memory");
  const settings = resource("settings");
  const key = JSON.stringify([memory.data, memory.error, settings.data?.effective?.memory]);
  renderKeyed(`${slot}:memory`, key, () => {
    const entries = memory.data?.entries ?? [];
    const global = entries.filter((e) => e.scope === "global").length;
    const project = entries.length - global;
    const config = settings.data?.effective?.memory;
    setChildren(
      card,
      cardHead("home-memory", "Memory"),
      h(
        "dl",
        { class: "stats" },
        h("div", {}, h("dt", {}, "Project"), h("dd", { class: "tabular" }, String(project))),
        h("div", {}, h("dt", {}, "Global"), h("dd", { class: "tabular" }, String(global))),
        h("div", {}, h("dt", {}, "Vector search"), h("dd", {}, memory.data?.vector ? "On" : "Off")),
        h("div", {}, h("dt", {}, "Auto recall"), h("dd", {}, config ? (config.enabled && config.auto_recall ? "On" : "Off") : "–")),
      ),
      entries.length
        ? h(
            "ul",
            { class: "memory-peek" },
            entries.slice(0, 4).map((entry) =>
              h("li", {}, h("span", { class: "strong ellipsis", title: entry.description }, entry.name), h("span", { class: "muted small ellipsis" }, entry.description)),
            ),
          )
        : h("p", { class: "empty-note" }, memory.error ? memory.error : "Nothing remembered yet. Agents save durable facts here as they work."),
      h(
        "footer",
        { class: "card-foot" },
        h("button", { class: "link", type: "button", onclick: () => navigate({ name: "settings", section: "memory" }) }, "Browse memory"),
      ),
    );
  });
}

// Agents overview: a dense tree table of every live agent, with a live
// transcript preview of the selected one on wide screens.

export function agentsView({ isWide }) {
  const slot = `agents${++instances}`;
  const table = h("div", { class: "agents-table-wrap" });
  const summary = h("p", { class: "page-summary" });
  const previewHost = h("aside", { class: "agent-preview", "aria-label": "Agent preview" });
  const root = h(
    "div",
    { class: "page agents-page" },
    h("header", { class: "page-head" }, h("div", {}, h("h1", { class: "page-title" }, "Agents"), summary)),
    h("div", { class: "agents-layout" }, table, previewHost),
  );
  let selected = null;
  let preview = null;

  const select = (id) => {
    if (!isWide()) {
      openAgent(id);
      return;
    }
    selected = id;
    view.render();
    showPreview();
  };

  const showPreview = () => {
    const agent = selected ? state.agents.get(selected) : null;
    if (preview?.agent === agent && agent) return;
    preview?.dispose();
    preview = null;
    if (!agent) {
      setChildren(previewHost, h("div", { class: "preview-empty" }, "Select an agent to follow its work here."));
      return;
    }
    const scroller = h("div", { class: "preview-scroll" });
    preview = new TranscriptView(agent, { scroller, embedded: true });
    scroller.append(preview.root);
    setChildren(
      previewHost,
      h(
        "header",
        { class: "preview-head" },
        h("span", { class: "preview-title ellipsis", title: displayName(agent) }, displayName(agent)),
        h("button", { class: "button small", type: "button", onclick: () => openAgent(agent.id) }, "Open transcript"),
      ),
      scroller,
    );
    preview.load();
  };

  const view = {
    root,
    render() {
      const agents = liveAgents();
      const waiting = waitingAgents();
      if (selected && !state.agents.get(selected)?.live) selected = null;
      if (!selected && agents.length) selected = [...agents].sort(byCreated).reverse().find((a) => !a.parent)?.id ?? agents[0].id;
      const running = agents.filter((a) => a.status === "running").length;
      const cost = agents.reduce((sum, a) => sum + (a.usage?.cost ?? 0), 0);
      const tokens = agents.reduce((sum, a) => sum + totalTokens(a.usage), 0);
      summary.textContent = agents.length
        ? [
            `${plural(agents.length, "agent")}`,
            `${running} running`,
            waiting.size ? `${waiting.size} waiting for approval` : null,
            `${formatTokens(tokens)} tokens`,
            cost ? formatCost(cost) : null,
          ]
            .filter(Boolean)
            .join(" · ")
        : "No live agents";
      const key = JSON.stringify([
        selected,
        [...waiting],
        agents.map((a) => [a.id, a.parent, displayName(a), a.status, a.activity, a.model, a.usage, a.contextTokens, todoProgress(a.todos)]),
      ]);
      renderKeyed(`${slot}:table`, key, () => setChildren(table, agentsTable(agents, waiting, selected, select)));
      if (isWide()) showPreview();
      else if (preview) {
        preview.dispose();
        preview = null;
        setChildren(previewHost);
      }
    },
    dispose() {
      preview?.dispose();
      preview = null;
    },
  };
  return view;
}

function agentsTable(agents, waiting, selected, select) {
  if (!agents.length) {
    return h(
      "div",
      { class: "empty-state" },
      h("p", { class: "empty-title" }, "No agents are running"),
      h("p", { class: "muted" }, "Start a session and every agent and subagent shows up here with its status, model and cost."),
      h("button", { class: "button primary", type: "button", onclick: () => navigate({ name: "home" }) }, "New session"),
    );
  }
  const ids = new Set(agents.map((a) => a.id));
  const roots = agents.filter((a) => !a.parent || !ids.has(a.parent)).sort(byCreated).reverse();
  const rows = [];
  const walk = (agent, depth) => {
    rows.push(agentRow(agent, depth, waiting.has(agent.id), agent.id === selected, select));
    agents
      .filter((other) => other.parent === agent.id)
      .sort(byCreated)
      .forEach((child) => walk(child, depth + 1));
  };
  roots.forEach((root) => walk(root, 0));
  return h(
    "table",
    { class: "table agents-table" },
    h(
      "thead",
      {},
      h(
        "tr",
        {},
        h("th", {}, "Agent"),
        h("th", {}, "Status"),
        h("th", { class: "col-model" }, "Model"),
        h("th", { class: "num col-context" }, "Context"),
        h("th", { class: "num" }, "Tokens"),
        h("th", { class: "num col-cost" }, "Cost"),
        h("th", { class: "num col-age" }, "Started"),
        h("th", { class: "col-actions" }, h("span", { class: "sr-only" }, "Actions")),
      ),
    ),
    h("tbody", {}, rows),
  );
}

function agentRow(agent, depth, waiting, isSelected, select) {
  const status = waiting ? "waiting" : agent.status;
  const progress = todoProgress(agent.todos);
  const window = contextWindow(agent.model);
  const context = agent.contextTokens ? `${Math.round((agent.contextTokens / window) * 100)}%` : "–";
  const running = agent.status === "running";
  return h(
    "tr",
    { class: `${isSelected ? "selected" : ""}`, "data-depth": depth },
    h(
      "td",
      { class: "cell-agent", style: { "--depth": depth } },
      h(
        "button",
        { class: "row-link agent-name", type: "button", "aria-pressed": String(isSelected), onclick: () => select(agent.id), title: `${displayName(agent)}\n${agent.id}` },
        depth ? h("span", { class: "tree-elbow", "aria-hidden": "true" }) : null,
        h("span", { class: "ellipsis" }, displayName(agent)),
      ),
      h(
        "div",
        { class: "agent-activity ellipsis", title: agent.activity },
        running ? agent.activity || "working" : agent.report ? reportLine(agent.report) : "",
      ),
    ),
    h(
      "td",
      { class: "cell-status" },
      h("span", { class: "status" }, statusDot(status), waiting ? "Approval" : STATUS_TEXT[agent.status] ?? agent.status),
      progress ? h("span", { class: "muted small tabular" }, ` ${progress.done}/${progress.total}`) : null,
    ),
    h("td", { class: "col-model cell-model" }, modelLabel(agent.model)),
    h("td", { class: "num col-context", title: agent.contextTokens ? `${formatTokens(agent.contextTokens)} of ${formatTokens(window)}` : "" }, context),
    h("td", { class: "num" }, formatTokens(totalTokens(agent.usage))),
    h("td", { class: "num col-cost" }, agent.usage?.cost ? formatCost(agent.usage.cost) : "–"),
    h(
      "td",
      { class: "num col-age muted", title: agent.created ? fullTime(agent.created) : "" },
      agent.created ? (running ? formatDuration(Date.now() - Date.parse(agent.created)) : relativeTime(agent.created)) : "–",
    ),
    h(
      "td",
      { class: "col-actions" },
      running
        ? h("button", { class: "icon-button small", type: "button", "aria-label": `Stop ${displayName(agent)}`, title: "Stop", onclick: () => stopAgent(agent) }, icon("stop", 14))
        : null,
      h(
        "button",
        { class: "icon-button small", type: "button", "aria-label": `Open ${displayName(agent)}`, title: "Open transcript", onclick: () => openAgent(agent.id) },
        icon("chevronRight", 14),
      ),
    ),
  );
}

// First line of a report, without markdown markers.
function reportLine(report) {
  const line = String(report).trim().split("\n").find((l) => l.trim()) ?? "";
  return line.replace(/^#+\s*|^[-*>]\s+/, "").replace(/[*_`]/g, "");
}

// Self-update: shared by the top bar popover and Settings, About.

const METHOD_NOTES = {
  homebrew: "malac was installed with Homebrew, which manages its files.",
  read_only: "The install directory is not writable by this user.",
  unsupported: "No release is built for this platform.",
};
const panels = new Set();

/** Re-renders every open update panel (after a check, install or event). */
export function refreshUpdatePanels() {
  for (const panel of panels) {
    if (panel.el.isConnected) panel.render();
    else panels.delete(panel);
  }
}

export function updatePanel() {
  const local = { busy: null, error: null };
  const el = h("div", { class: "update-panel" });
  const run = async (kind) => {
    local.busy = kind;
    local.error = null;
    panel.render();
    try {
      if (kind === "install") {
        const status = await installUpdate();
        if (status.installed) toast(`Installed ${status.installed}. Restart malac to use it.`, "success");
      } else {
        await checkForUpdate();
      }
    } catch (error) {
      local.error = error.message;
    }
    local.busy = null;
    panel.render();
  };
  const panel = {
    el,
    render() {
      const u = state.update;
      if (!u) {
        setChildren(
          el,
          h("p", { class: "muted" }, "This server does not report update status."),
          local.error ? h("p", { class: "field-error block" }, local.error) : null,
          h("div", { class: "form-actions" }, h("button", { class: "button", type: "button", disabled: Boolean(local.busy), onclick: () => run("check") }, local.busy ? "Checking…" : "Check for updates")),
        );
        return;
      }
      const target = u.installed ?? u.latest;
      const headline = u.installed
        ? h("p", { class: "update-headline" }, statusDot("ok"), `Version ${u.installed} is installed. Restart malac to use it.`)
        : u.available
          ? h("p", { class: "update-headline" }, statusDot("ok"), `Version ${u.latest} is available.`)
          : h("p", { class: "update-headline" }, statusDot("idle"), u.latest ? "malac is up to date." : "No release information yet.");
      const canInstall = u.available && !u.installed && u.method === "self_update";
      setChildren(
        el,
        headline,
        h(
          "dl",
          { class: "kv update-facts" },
          h("dt", {}, "Running"),
          h("dd", { class: "tabular" }, u.current),
          h("dt", {}, "Latest"),
          h(
            "dd",
            { class: "tabular" },
            u.latest ?? "Unknown",
            target && u.release_url
              ? [" · ", h("a", { href: u.release_url, target: "_blank", rel: "noopener noreferrer" }, "Release notes")]
              : null,
          ),
          h("dt", {}, "Last checked"),
          h("dd", { title: u.checked_at ? fullTime(u.checked_at) : "" }, u.checked_at ? relativeTime(u.checked_at) : "Never"),
        ),
        u.available && u.method !== "self_update"
          ? h(
              "div",
              { class: "update-hint" },
              h("p", { class: "muted small" }, METHOD_NOTES[u.method] ?? "", u.hint ? " Update it with:" : ""),
              u.hint
                ? h("div", { class: "hint-command" }, h("code", {}, u.hint), copyButton(u.hint, { label: "Copy command", icon: icon("copy", 14) }))
                : null,
            )
          : null,
        local.error ? h("p", { class: "field-error block" }, local.error) : null,
        h(
          "div",
          { class: "form-actions" },
          h(
            "button",
            { class: "button", type: "button", disabled: Boolean(local.busy), onclick: () => run("check") },
            local.busy === "check" ? [h("span", { class: "spinner small" }), "Checking…"] : "Check for updates",
          ),
          canInstall
            ? h(
                "button",
                { class: "button primary", type: "button", disabled: Boolean(local.busy), onclick: () => run("install") },
                local.busy === "install" ? [h("span", { class: "spinner small" }), "Installing…"] : `Install ${u.latest}`,
              )
            : null,
        ),
      );
    },
  };
  panels.add(panel);
  panel.render();
  return panel;
}

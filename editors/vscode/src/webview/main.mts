// The chat webview: renders the state the extension host sends and relays
// what the user does. It never talks to the daemon itself.

import type { AgentSummary, Banner, ChatState, FromWebview, MediaFile, ToWebview } from "../chat/messages.js";
import { Composer } from "./composer.mjs";
import { formatCost, formatTokens, h, icon, pig, setChildren, tildePath, truncate } from "./dom.mjs";
import { closeMenu, MenuItem, menuButton } from "./menu.mjs";
import { Transcript } from "./transcript.mjs";
import { setViewerHost } from "./viewer.mjs";

interface VsCodeApi {
  postMessage(message: unknown): void;
  getState(): unknown;
  setState(state: unknown): void;
}

declare function acquireVsCodeApi(): VsCodeApi;

const vscode = acquireVsCodeApi();
const STATUS_TEXT: Record<string, string> = {
  running: "Running",
  waiting: "Awaiting approval",
  completed: "Done",
  failed: "Failed",
  cancelled: "Stopped",
  idle: "Idle",
  saved: "Saved",
};

let state: ChatState | null = null;
let transcriptReady = false;

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const app = $("app");
const header = $("header");
const banners = $("banners");
const scroller = $("scroller");
const approvals = $("approvals");
const jump = $<HTMLButtonElement>("jump");
const toasts = $("toasts");

/** Keeps the shown tab and agent in the webview state, so an editor tab comes back to them after a reload. */
function remember(current: ChatState): void {
  const saved = (vscode.getState() as { chat?: { tab?: string | null; agent?: string | null } } | null) ?? {};
  const chat = { tab: current.tab?.id ?? null, agent: current.agent?.id ?? null };
  if (saved.chat?.tab !== chat.tab || saved.chat?.agent !== chat.agent) vscode.setState({ ...saved, chat });
}

function post(message: FromWebview): void {
  vscode.postMessage(message);
}

function toast(level: "info" | "warning" | "error", message: string): void {
  const node = h("div", { class: `toast ${level}`, role: level === "error" ? "alert" : "status" }, icon(level === "info" ? "info" : "alert", 14), h("span", {}, message));
  toasts.append(node);
  setTimeout(() => node.remove(), level === "error" ? 8000 : 4500);
}

// Files malac sent, fetched by the extension host (which holds the token) once per id; the newest are kept.
const SHARED_KEPT = 40;
const shared = new Map<string, { promise: Promise<{ media_type: string; data: string }>; resolve: (file: { media_type: string; data: string }) => void; reject: (error: Error) => void }>();

function sharedFile(id: string): Promise<{ media_type: string; data: string }> {
  let entry = shared.get(id);
  if (!entry) {
    let resolve!: (file: { media_type: string; data: string }) => void;
    let reject!: (error: Error) => void;
    const promise = new Promise<{ media_type: string; data: string }>((yes, no) => ((resolve = yes), (reject = no)));
    entry = { promise, resolve, reject };
    shared.set(id, entry);
    if (shared.size > SHARED_KEPT) shared.delete(shared.keys().next().value!);
    post({ type: "loadShared", id });
  }
  return entry.promise;
}

function media(action: "open" | "save", file: MediaFile): void {
  post({ type: "media", action, file });
}

setViewerHost({ media });

const transcript = new Transcript($("transcript"), scroller, {
  post,
  cwd: () => state?.tab?.cwd ?? "",
  agents: () => state?.agents ?? [],
  shared: sharedFile,
  media,
});
transcript.onPinnedChange = (pinned) => {
  jump.hidden = pinned;
};
setChildren(jump, icon("arrowDown", 14), h("span", {}, "Latest"));
jump.addEventListener("click", () => transcript.jumpToEnd());

const saved = (vscode.getState() as { draft?: { text: string; paths: { path: string; label: string }[] } } | null) ?? {};
const composer = new Composer(
  $<HTMLFormElement>("composer"),
  {
    post,
    state: () => state,
    toast,
    save: (draft) => vscode.setState({ ...(vscode.getState() as object | null), draft }),
    restore: () => saved.draft ?? null,
  },
  app,
);

// Header: which tab and agent the chat shows.

const agentButton = menuButton({ class: "agent-switch", label: "Agents and tabs", title: "Switch agent or tab", build: agentMenu, placement: "below" });
const newButton = h("button", { class: "icon-button", type: "button", title: "New agent", "aria-label": "New agent", onclick: () => post({ type: "selectAgent", agent: null }) }, icon("plus", 16));
const talkButton = h("button", { class: "icon-button", type: "button", title: "Talk to malac", "aria-label": "Talk to malac", hidden: true, onclick: talk }, icon("headphones", 16));
const meter = h("span", { class: "meter" });
setChildren(header, agentButton, meter, talkButton, newButton);

/** Talk mode runs in a terminal (`malac voice talk`), through the agent the chat shows. */
function talk(): void {
  if (!state) return;
  if (state.voice.blocked) return toast("warning", state.voice.blocked);
  if (!state.tab) return toast("warning", "Open a malac tab for this folder first.");
  post({ type: "command", command: "malac.talk", args: [state.tab.id, state.agent?.id ?? null] });
}

function agentMenu(): MenuItem[] {
  if (!state) return [];
  const items: MenuItem[] = [];
  const current = state.agent?.id ?? null;
  items.push({ label: "New agent", icon: "plus", heading: state.tab ? `Agents in ${state.tab.name}` : undefined, checked: current === null, onPick: () => post({ type: "selectAgent", agent: null }) });
  for (const agent of state.agents) {
    items.push({
      label: truncate(agent.name, 60),
      detail: STATUS_TEXT[agent.status],
      indent: agent.depth,
      checked: agent.id === current,
      onPick: () => post({ type: "selectAgent", agent: agent.id }),
    });
  }
  const others = state.tabs.filter((t) => t.id !== state!.tab?.id);
  others.forEach((tab, index) => {
    items.push({
      label: tab.name,
      detail: tildePath(tab.cwd, state!.home),
      icon: "folder",
      heading: index === 0 ? "Other tabs" : undefined,
      separator: index === 0,
      onPick: () => post({ type: "selectTab", tab: tab.id }),
    });
  });
  items.push({ label: "New tab for this folder…", icon: "plus", separator: true, onPick: () => post({ type: "command", command: "malac.newTabForFolder" }) });
  return items;
}

function renderHeader(): void {
  if (!state) return;
  const agent = state.agent;
  const status = agent?.status ?? "idle";
  setChildren(
    agentButton,
    h("span", { class: `dot ${status}`, "aria-hidden": "true" }),
    h("span", { class: "agent-name" }, agent ? agent.name : state.tab ? "New agent" : "malac"),
    state.tab ? h("span", { class: "tab-name" }, state.tab.name) : null,
    icon("chevronDown", 12),
  );
  agentButton.setAttribute("aria-label", `${agent ? `Agent ${agent.name}, ${STATUS_TEXT[status]}` : "New agent"}${state.tab ? ` in tab ${state.tab.name}` : ""}. Switch agent or tab`);
  newButton.disabled = !state.tab;
  talkButton.hidden = !state.voice.talk;
  talkButton.setAttribute("aria-disabled", String(Boolean(state.voice.blocked) || !state.tab));
  if (agent && (agent.usage || agent.contextTokens)) {
    const u = agent.usage;
    const tokens = u.input_tokens + u.output_tokens + u.cache_read_tokens + u.cache_write_tokens;
    meter.textContent = tokens ? `${formatTokens(tokens)} tok${u.cost ? ` · ${formatCost(u.cost)}` : ""}` : "";
    meter.title = `Input ${u.input_tokens}, output ${u.output_tokens}, cache read ${u.cache_read_tokens}, cache write ${u.cache_write_tokens}${agent.contextTokens ? `\nContext ${formatTokens(agent.contextTokens)}` : ""}`;
  } else {
    meter.textContent = "";
  }
}

function renderBanners(): void {
  if (!state) return;
  setChildren(banners, state.banners.map(bannerNode));
}

function bannerNode(banner: Banner): HTMLElement {
  return h(
    "div",
    { class: `banner ${banner.level}` },
    icon(banner.level === "info" ? "info" : "alert", 14),
    h("span", { class: "banner-text" }, banner.text),
    banner.actions.length
      ? h(
          "span",
          { class: "banner-actions" },
          banner.actions.map((action) =>
            h("button", { class: `button${action.primary ? " primary" : ""}`, type: "button", onclick: () => post({ type: "command", command: action.command, args: action.args }) }, action.label),
          ),
        )
      : null,
  );
}

function renderApprovals(): void {
  if (!state) return;
  transcript.claimPending(state.pending);
  const loose = state.pending.filter((p) => !transcript.showsRequest(p.id));
  setChildren(
    approvals,
    loose.map((request) => {
      const buttons = h("div", { class: "approval-actions" });
      const choose = (decision: "allow" | "allow_session" | "deny") => () => {
        post({ type: "approve", request: request.id, decision });
        for (const b of buttons.querySelectorAll("button")) b.disabled = true;
      };
      buttons.append(
        h("button", { class: "button primary", type: "button", onclick: choose("allow") }, "Allow"),
        h("button", { class: "button", type: "button", onclick: choose("allow_session"), title: "Allow this and similar calls for the rest of the session" }, "Always"),
        h("button", { class: "button", type: "button", onclick: choose("deny") }, "Deny"),
      );
      const agent = state!.agents.find((a) => a.id === request.agent);
      const who = h("strong", {}, agent?.name ?? request.agentLabel);
      const what = request.plugin
        ? h("span", {}, who, " asks to ", h("strong", {}, request.title.charAt(0).toLowerCase() + request.title.slice(1)), h("span", { class: "muted" }, ". It runs code on the daemon's machine."))
        : h("span", {}, who, ` wants to use ${request.tool}`, request.summary ? h("span", { class: "muted" }, `: ${truncate(request.summary, 160)}`) : null);
      return h(
        "div",
        { class: `approval${request.plugin ? " plugin" : ""}`, role: "group", "aria-label": `Approval for ${request.agentLabel}` },
        h("div", { class: "approval-text" }, icon("shield", 14), what),
        request.details ? detailsNode(request.id, request.details) : null,
        h("div", { class: "approval-row" }, buttons, agent && agent.id !== state!.agent?.id ? h("button", { class: "link", type: "button", onclick: () => post({ type: "openAgent", agent: agent.id }) }, "Show") : null),
      );
    }),
  );
  approvals.hidden = loose.length === 0;
}

/** Requests whose details the user opened, so re-rendering keeps them open. */
const openDetails = new Set<string>();

function detailsNode(id: string, details: string): HTMLElement {
  const summary = h("summary", {}, openDetails.has(id) ? "Hide details" : "Show details");
  const node = h("details", { class: "approval-details", open: openDetails.has(id) }, summary, h("pre", { class: "output" }, details));
  node.addEventListener("toggle", () => {
    if (node.open) openDetails.add(id);
    else openDetails.delete(id);
    summary.textContent = node.open ? "Hide details" : "Show details";
  });
  return node;
}

function emptyState(): HTMLElement {
  const tab = state?.tab;
  const tips = [
    ["at", "Type @ to add workspace files, / for commands"],
    ["paperclip", state?.dropHint ?? "Drop files or images here"],
    ["image", "Paste screenshots"],
    ["eye", "The open file goes along; click its chip to leave it out"],
  ];
  return h(
    "div",
    { class: "empty" },
    pig(40),
    h("p", { class: "empty-title" }, tab ? `What should malac do in ${tab.name}?` : "malac"),
    tab ? h("p", { class: "empty-sub" }, tildePath(tab.cwd, state?.home ?? null)) : h("p", { class: "empty-sub" }, state?.connection.state === "online" ? "Open a tab for this folder to start." : "Connecting to the daemon…"),
    h("ul", { class: "tips" }, tips.map(([name, text]) => h("li", {}, icon(name, 14), text))),
  );
}

function renderEmpty(): void {
  const existing = transcript.root.querySelector(":scope > .empty");
  const show = transcriptReady && transcript.isEmpty;
  if (show && !existing) transcript.root.append(emptyState());
  else if (show && existing) existing.replaceWith(emptyState());
  else if (!show) existing?.remove();
}

function refreshAgents(agents: AgentSummary[]): void {
  transcript.linkChildren(agents);
}

function render(): void {
  if (!state) return;
  app.removeAttribute("aria-busy");
  renderHeader();
  renderBanners();
  composer.update(state);
  refreshAgents(state.agents);
  renderApprovals();
  renderEmpty();
}

window.addEventListener("message", (event: MessageEvent<ToWebview>) => {
  const message = event.data;
  switch (message?.type) {
    case "state":
      state = message.state;
      remember(state);
      render();
      break;
    case "transcript":
      transcript.reset(message.agent, message.history, message.events, message.error);
      transcriptReady = true;
      if (state) {
        refreshAgents(state.agents);
        renderApprovals();
      }
      renderEmpty();
      break;
    case "events":
      if (message.agent !== transcript.agent) break;
      for (const env of message.events) transcript.apply(env);
      if (state) renderApprovals();
      renderEmpty();
      break;
    case "files":
      composer.files(message.id, message.files);
      break;
    case "attach":
      composer.addPaths(message.paths);
      if (message.images?.length) composer.addImages(message.images);
      composer.focus();
      break;
    case "focus":
      composer.focus(message.text);
      break;
    case "sent":
      composer.sentResult(message.ok, message.error, message.notes);
      if (message.ok) transcript.jumpToEnd();
      break;
    case "toast":
      toast(message.level, message.message);
      break;
    case "shared": {
      const entry = shared.get(message.id);
      if (!entry) break;
      if (message.data && message.media_type) entry.resolve({ media_type: message.media_type, data: message.data });
      else {
        // Asked again next time it renders (after a reconnect, say).
        shared.delete(message.id);
        entry.reject(new Error(message.error ?? "Cannot load the file"));
      }
      break;
    }
    case "dictation":
      composer.dictationUpdate(message);
      break;
  }
});

// Code block copy buttons, rendered by the markdown module.
document.addEventListener("click", (event) => {
  const button = (event.target as HTMLElement).closest(".code-copy");
  if (!button) return;
  const code = button.parentElement?.querySelector("code")?.textContent ?? "";
  void navigator.clipboard.writeText(code).then(() => {
    button.textContent = "Copied";
    setTimeout(() => (button.textContent = "Copy"), 1200);
  });
});

window.addEventListener("resize", () => closeMenu());
post({ type: "ready" });

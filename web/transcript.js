// The transcript of one agent: saved history plus live events, rendered as
// messages, tool cards, subagent cards and plans.

import {
  addUsage,
  baseName,
  clockTime,
  copyButton,
  dialog,
  emptyUsage,
  firstLine,
  formatCost,
  formatDuration,
  formatTokens,
  fullTime,
  h,
  plural,
  setChildren,
  truncate,
  usageParts,
} from "./dom.js";
import { icon, toolIcon } from "./icons.js";
import { highlight, languageFor, markdownNode, updateMarkdown } from "./markdown.js";
import {
  STATUS_TEXT,
  api,
  byCreated,
  childrenOf,
  decide,
  displayName,
  invalidate,
  loadHistory,
  state,
  todoProgress,
  viewAgent,
} from "./store.js";
import { statusDot } from "./widgets.js";

const IMAGE_TYPES = new Set(["image/png", "image/jpeg", "image/gif", "image/webp"]);
const SUMMARY_KEYS = ["command", "path", "pattern", "url", "query", "name", "text", "id", "message"];
const PIN_THRESHOLD_PX = 64;
const DRAFT_RENDER_MS = 90;
const DIFF_CONTEXT = 3;
const DIFF_MAX_CELLS = 400_000;
const PREVIEW_LINES = 24;
const IMAGE_PLACEHOLDER = /\[image\/[\w.+-]+ image\]/;

// Helpers

export function imageNode(block, alt = "Attached image") {
  if (!IMAGE_TYPES.has(block.media_type)) return null;
  const src = `data:${block.media_type};base64,${block.data}`;
  return h(
    "button",
    { class: "thumb", type: "button", "aria-label": `Open ${alt.toLowerCase()}`, onclick: () => openImage(src, alt) },
    h("img", { src, alt, loading: "lazy" }),
  );
}

function openImage(src, alt) {
  dialog({
    title: alt,
    className: "lightbox",
    body: h("img", { src, alt }),
    buttons: [{ label: "Close", value: null }],
  });
}

function resultText(content) {
  return (content ?? [])
    .map((block) => (block.type === "text" ? block.text : block.type === "image" ? `[${block.media_type} image]` : ""))
    .join("");
}

function resultImages(content) {
  return (content ?? []).filter((block) => block.type === "image");
}

/** A `malac_configure` request to enable a plugin that runs unapproved code. */
export function isPluginApproval(request) {
  return request?.tool === "malac_configure" && typeof request.input?.plugin === "string";
}

/** What enabling the plugin would run (its hooks, MCP servers, scripts), as the daemon lists it. */
export function pluginApprovalDetails(input) {
  return h(
    "div",
    { class: "plugin-approval" },
    h("p", { class: "muted small" }, "It runs code that was not approved before. Review what it adds:"),
    input?.contents ? h("pre", { class: "output plugin-contents", tabindex: "0", "aria-label": `What ${input.plugin} runs` }, input.contents) : null,
  );
}

export function summarizeInput(name, input) {
  if (!input || typeof input !== "object") return "";
  if (name === "agent_spawn") return `${input.label || input.agent || "agent"}: ${firstLine(input.task)}`;
  if (name === "todo") return plural(input.items?.length ?? 0, "item");
  if (name === "agent_wait") return input.ids?.length ? plural(input.ids.length, "agent") : "all subagents";
  const key = SUMMARY_KEYS.find((k) => typeof input[k] === "string");
  if (key) return truncate(firstLine(input[key]), 200);
  const json = JSON.stringify(input);
  return json === "{}" ? "" : truncate(json, 200);
}

function parseAgentId(output) {
  return /\(id ([0-9a-f-]{36})\)/.exec(output ?? "")?.[1] ?? null;
}

function reportFrom(output) {
  const header = /^Report from `[^`]*` \(id [0-9a-f-]+\):\s*/.exec(output ?? "");
  return header ? output.slice(header[0].length) : "";
}

function relativePath(path, cwd) {
  const value = String(path ?? "");
  const base = String(cwd ?? state.cwd ?? "");
  if (base && value.startsWith(`${base}/`)) return value.slice(base.length + 1);
  return value;
}

function humanize(name) {
  const text = name.replace(/^(browser|computer|memory|agent)_/, "").replace(/_/g, " ");
  return text.charAt(0).toUpperCase() + text.slice(1);
}

const TOOL_TITLES = { bash: "Bash", read: "Read", write: "Write", edit: "Edit", glob: "Glob", grep: "Grep", web_fetch: "Fetch", agent_wait: "Wait for subagents" };

function toolTitle(name) {
  if (TOOL_TITLES[name]) return { title: TOOL_TITLES[name], tag: null };
  if (name.includes("__")) {
    const [server, ...rest] = name.split("__");
    return { title: rest.join("__").replace(/_/g, " "), tag: server };
  }
  if (name.startsWith("browser_")) return { title: humanize(name), tag: "browser" };
  if (name.startsWith("computer_")) return { title: humanize(name), tag: "computer" };
  if (name.startsWith("memory_")) return { title: humanize(name), tag: "memory" };
  return { title: humanize(name), tag: null };
}

/** A titled block inside a tool card. */
function section(label, content, { copy, className = "" } = {}) {
  return h(
    "div",
    { class: `tool-section ${className}`.trim() },
    h(
      "div",
      { class: "tool-section-head" },
      h("span", {}, label),
      copy != null ? copyButton(copy, { label: `Copy ${label.toLowerCase()}`, icon: icon("copy", 14) }) : null,
    ),
    content,
  );
}

/** Monospace output with a "show all" toggle for long text. */
function outputBlock(text, { lang = "", isError = false, prompt = null } = {}) {
  const lines = text.split("\n");
  const long = lines.length > PREVIEW_LINES * 2;
  const code = h("code", {});
  const pre = h("pre", { class: `output${isError ? " error" : ""}${long ? " clipped" : ""}` }, code);
  const fill = (all) => {
    const shown = all || !long ? text : lines.slice(0, PREVIEW_LINES).join("\n");
    code.innerHTML = (prompt ? `<span class="tok-prompt">${prompt}</span>` : "") + highlight(shown, lang);
  };
  fill(false);
  if (!long) return pre;
  const more = h("button", { class: "link show-more", type: "button" }, `Show all ${lines.length} lines`);
  more.addEventListener("click", () => {
    const expanded = pre.classList.toggle("clipped") === false;
    fill(expanded);
    more.textContent = expanded ? "Show less" : `Show all ${lines.length} lines`;
  });
  return h("div", { class: "output-wrap" }, pre, more);
}

function inputTable(input) {
  const entries = Object.entries(input ?? {});
  if (!entries.length) return h("div", { class: "muted small" }, "No input");
  const flat = entries.every(([, v]) => v == null || typeof v !== "object");
  if (!flat) return outputBlock(JSON.stringify(input, null, 2), { lang: "json" });
  return h(
    "dl",
    { class: "kv" },
    entries.map(([key, value]) => [h("dt", {}, key), h("dd", {}, String(value))]),
  );
}

// Line diff of old_string against new_string, with common prefix and suffix
// trimmed before the quadratic part.
export function diffLines(before, after) {
  const a = before === "" ? [] : before.split("\n");
  const b = after === "" ? [] : after.split("\n");
  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) start += 1;
  let endA = a.length;
  let endB = b.length;
  while (endA > start && endB > start && a[endA - 1] === b[endB - 1]) {
    endA -= 1;
    endB -= 1;
  }
  const midA = a.slice(start, endA);
  const midB = b.slice(start, endB);
  const ops = a.slice(0, start).map((text) => ({ op: " ", text }));
  if (midA.length * midB.length > DIFF_MAX_CELLS) {
    ops.push(...midA.map((text) => ({ op: "-", text })), ...midB.map((text) => ({ op: "+", text })));
  } else {
    const n = midA.length;
    const m = midB.length;
    const table = Array.from({ length: n + 1 }, () => new Uint32Array(m + 1));
    for (let i = n - 1; i >= 0; i -= 1) {
      for (let j = m - 1; j >= 0; j -= 1) {
        table[i][j] = midA[i] === midB[j] ? table[i + 1][j + 1] + 1 : Math.max(table[i + 1][j], table[i][j + 1]);
      }
    }
    let i = 0;
    let j = 0;
    while (i < n || j < m) {
      if (i < n && j < m && midA[i] === midB[j]) {
        ops.push({ op: " ", text: midA[i] });
        i += 1;
        j += 1;
      } else if (i < n && (j === m || table[i + 1][j] >= table[i][j + 1])) {
        ops.push({ op: "-", text: midA[i] });
        i += 1;
      } else {
        ops.push({ op: "+", text: midB[j] });
        j += 1;
      }
    }
  }
  ops.push(...a.slice(endA).map((text) => ({ op: " ", text })));
  return ops;
}

function diffView(ops, lang) {
  const rows = [];
  const changed = ops.map((o) => o.op !== " ");
  const near = (index) => {
    for (let k = Math.max(0, index - DIFF_CONTEXT); k <= Math.min(ops.length - 1, index + DIFF_CONTEXT); k += 1) {
      if (changed[k]) return true;
    }
    return false;
  };
  let hidden = 0;
  const flushHidden = () => {
    if (hidden) rows.push(h("div", { class: "diff-gap" }, `${plural(hidden, "unchanged line")}`));
    hidden = 0;
  };
  ops.forEach((o, index) => {
    if (o.op === " " && !near(index)) {
      hidden += 1;
      return;
    }
    flushHidden();
    const line = h("span", { class: "diff-text" });
    line.innerHTML = highlight(o.text, lang) || " ";
    rows.push(
      h(
        "div",
        { class: `diff-line${o.op === "+" ? " add" : o.op === "-" ? " del" : ""}` },
        h("span", { class: "diff-sign", "aria-hidden": "true" }, o.op === " " ? "" : o.op === "+" ? "+" : "−"),
        line,
      ),
    );
  });
  flushHidden();
  return h("div", { class: "diff", role: "group", "aria-label": "Changes" }, rows);
}

function diffStats(ops) {
  return {
    added: ops.filter((o) => o.op === "+").length,
    removed: ops.filter((o) => o.op === "-").length,
  };
}

// Transcript view

export class TranscriptView {
  constructor(agent, { scroller = null, embedded = false } = {}) {
    this.agent = agent;
    this.scroller = scroller;
    this.embedded = embedded;
    this.root = h("div", { class: `transcript${embedded ? " embedded" : ""}` });
    this.cards = new Map();
    this.approvals = new Map();
    this.spawns = [];
    this.lastTodo = null;
    this.draft = null;
    this.turn = null;
    this.ready = false;
    this.pinned = true;
    this.followFrame = 0;
    this.awaitingImages = new Set();
    this.fetchImagesTimer = 0;
    this.onScroll = () => {
      const s = this.scroller;
      const pinned = s.scrollHeight - s.scrollTop - s.clientHeight < PIN_THRESHOLD_PX;
      if (pinned !== this.pinned) {
        this.pinned = pinned;
        invalidate("jump");
      }
    };
    scroller?.addEventListener("scroll", this.onScroll, { passive: true });
    state.views.add(this);
  }

  dispose() {
    state.views.delete(this);
    this.scroller?.removeEventListener("scroll", this.onScroll);
    clearTimeout(this.draft?.timer);
    clearTimeout(this.fetchImagesTimer);
    if (this.approvals.size) invalidate("permissions");
  }

  follow(force = false) {
    if (!this.scroller || (!this.pinned && !force) || this.followFrame) return;
    this.followFrame = requestAnimationFrame(() => {
      this.followFrame = 0;
      this.scroller.scrollTop = this.scroller.scrollHeight;
    });
  }

  jumpToEnd() {
    this.pinned = true;
    this.follow(true);
    invalidate("jump");
  }

  async load() {
    const agent = this.agent;
    let saved = agent.history;
    if (!saved) {
      const loading = h("div", { class: "transcript-loading" }, h("span", { class: "spinner" }), "Loading transcript");
      this.root.append(loading);
      try {
        saved = await loadHistory(agent);
      } catch (error) {
        saved = { messages: [], cutoff: 0, error: error.message };
      }
      loading.remove();
    }
    if (!state.views.has(this)) return;
    if (saved.error) this.notice("error", `Could not load the saved transcript: ${saved.error}`);
    for (const message of saved.messages) this.message(message);
    for (const { n, env } of agent.events) {
      if (n > saved.cutoff) this.apply(env);
    }
    this.linkKnownChildren();
    this.claimPending();
    this.ready = true;
    if (!this.root.childElementCount) this.root.append(emptyAgentState(agent));
    this.pinned = true;
    this.follow(true);
  }

  apply(env) {
    switch (env.type) {
      case "user_message":
        this.userMessage(env.text, env.images ?? [], env.at);
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
        this.assistantBlocks(env.message.content, env.at);
        this.scheduleImageFetch();
        break;
      case "tool_started":
        this.cards.get(env.call_id)?.start(env.summary, env.at);
        break;
      case "tool_finished":
        this.toolFinished(env);
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
        this.append(
          h(
            "details",
            { class: "divider-block" },
            h("summary", {}, icon("compress", 14), "Conversation compacted"),
            markdownNode(env.summary, "small"),
          ),
        );
        break;
      case "model_changed":
        this.append(h("div", { class: "event-line" }, icon("cpu", 14), "Model changed to ", h("strong", {}, env.model)));
        break;
      case "notice":
        this.notice(env.level, env.message);
        break;
      case "turn_finished":
        this.turnFinished(env);
        this.scheduleImageFetch();
        break;
      case "child_started":
        this.childStarted(env.child);
        break;
    }
  }

  append(node) {
    this.root.querySelector(":scope > .transcript-empty")?.remove();
    if (this.draft && node !== this.draft.root) this.root.insertBefore(node, this.draft.root);
    else this.root.append(node);
    this.follow();
  }

  message(message) {
    if (message.role === "user") this.userBlocks(message.content);
    else this.assistantBlocks(message.content);
  }

  userBlocks(blocks) {
    const text = [];
    const images = [];
    const attachments = [];
    let inAttachment = false;
    for (const block of blocks) {
      if (block.type === "tool_result") this.cards.get(block.tool_use_id)?.result(block);
      else if (block.type === "image" && inAttachment) attachments.at(-1).images.push(block);
      else if (block.type === "image") images.push(block);
      else if (block.type === "text" && block.text.startsWith("<recalled-memories>")) this.append(recalledNode(block.text));
      else if (block.type === "text" && ATTACHMENT_START.test(block.text)) {
        attachments.push(parseAttachment(block.text));
        inAttachment = true;
        continue;
      } else if (block.type === "text") text.push(block.text);
      inAttachment = false;
    }
    if (text.length || images.length || attachments.length) this.userMessage(text.join("\n"), images, null, attachments);
  }

  assistantBlocks(blocks, at) {
    for (const block of blocks) {
      if (block.type === "text" && block.text.trim()) this.append(assistantNode(block.text, at));
      else if (block.type === "thinking" && block.text.trim()) this.append(thinkingNode(block.text));
      else if (block.type === "tool_use") this.toolUse(block, at);
    }
  }

  userMessage(text, images, at, attachments = []) {
    this.clearDraft();
    const parsed = splitAttachedLine(text);
    const chips = attachments.length ? attachments : parsed.names.map((path) => ({ path, body: null, images: [] }));
    this.append(userNode(parsed.text, images, at, chips));
  }

  toolUse(block, at) {
    const card = createCard(block, this);
    if (at) card.at = at;
    this.cards.set(block.id, card);
    this.append(card.el);
    if (card instanceof SpawnCard) this.spawns.push(card);
    if (card instanceof TodoCard) {
      this.lastTodo?.supersede();
      this.lastTodo = card;
    }
  }

  toolFinished(env) {
    const card = this.cards.get(env.call_id);
    if (!card) return;
    card.finish(env.output, env.is_error, env.millis);
    if (IMAGE_PLACEHOLDER.test(env.output ?? "") && card.addImages) this.awaitingImages.add(env.call_id);
  }

  // Live tool results carry only a text preview; images are read back from
  // the session log once the results have been recorded.
  scheduleImageFetch() {
    if (!this.awaitingImages.size) return;
    clearTimeout(this.fetchImagesTimer);
    this.fetchImagesTimer = setTimeout(() => this.fetchImages(), 400);
  }

  async fetchImages() {
    const wanted = new Set(this.awaitingImages);
    try {
      const data = await api(`/api/sessions/${encodeURIComponent(this.agent.id)}`);
      for (const message of data.history ?? []) {
        for (const block of message.content) {
          if (block.type !== "tool_result" || !wanted.has(block.tool_use_id)) continue;
          this.cards.get(block.tool_use_id)?.addImages(resultImages(block.content));
          this.awaitingImages.delete(block.tool_use_id);
        }
      }
    } catch {
      // The images stay as placeholders; reopening the transcript loads them.
    }
  }

  ensureDraft() {
    if (!this.draft) {
      this.draft = { root: h("div", { class: "draft" }), text: "", textNode: null, thinking: null, preparing: null, timer: 0 };
      this.append(this.draft.root);
    }
    return this.draft;
  }

  // Streaming text is re-rendered as markdown at most every DRAFT_RENDER_MS.
  draftText(text) {
    const draft = this.ensureDraft();
    if (!draft.textNode) {
      draft.textNode = h("div", { class: "markdown assistant streaming" });
      draft.root.insertBefore(draft.textNode, draft.preparing);
    }
    draft.text += text;
    if (draft.timer) return;
    draft.timer = setTimeout(() => {
      draft.timer = 0;
      if (draft.textNode?.isConnected) updateMarkdown(draft.textNode, draft.text);
      this.follow();
    }, DRAFT_RENDER_MS);
  }

  draftThinking(text) {
    const draft = this.ensureDraft();
    if (!draft.thinking) {
      draft.thinking = document.createTextNode("");
      const node = h(
        "details",
        { class: "thinking live" },
        h("summary", {}, icon("bulb", 14), h("span", {}, "Thinking"), h("span", { class: "spinner small" })),
        h("div", { class: "thinking-body" }, draft.thinking),
      );
      draft.root.insertBefore(node, draft.root.firstChild);
    }
    draft.thinking.appendData(text);
  }

  draftTool(tool) {
    const draft = this.ensureDraft();
    draft.preparing ??= draft.root.appendChild(h("div", { class: "preparing" }));
    setChildren(draft.preparing, h("span", { class: "spinner small" }), `Preparing ${toolTitle(tool).title.toLowerCase()}`);
    this.follow();
  }

  clearDraft() {
    if (this.draft) clearTimeout(this.draft.timer);
    this.draft?.root.remove();
    this.draft = null;
  }

  turnFinished(env) {
    if (this.draft) {
      clearTimeout(this.draft.timer);
      if (this.draft.textNode) updateMarkdown(this.draft.textNode, this.draft.text);
      this.draft.preparing?.remove();
      this.draft.root.classList.add("interrupted");
      this.draft.textNode?.classList.remove("streaming");
      this.draft = null;
    }
    for (const card of this.cards.values()) card.abandon();
    const parts = [];
    if (env.status !== "completed") {
      parts.push(h("span", { class: `turn-status ${env.status}` }, STATUS_TEXT[env.status] ?? env.status));
    }
    if (this.turn) {
      parts.push(h("span", { title: "Turn duration" }, formatDuration(Date.parse(env.at) - this.turn.started)));
      const usage = this.turn.usage;
      parts.push(
        h(
          "span",
          { title: `Input ${usage.input_tokens}, output ${usage.output_tokens}, cache read ${usage.cache_read_tokens}, cache write ${usage.cache_write_tokens}` },
          usageParts(usage, 0).filter((p) => !p.startsWith("$")).join(" · "),
        ),
      );
      if (usage.cost) parts.push(h("span", {}, formatCost(usage.cost)));
      if (this.turn.context) parts.push(h("span", { title: "Context after this turn" }, `${formatTokens(this.turn.context)} context`));
    }
    if (parts.length) {
      this.append(h("div", { class: "turn-end", title: fullTime(env.at) }, parts));
    }
    this.turn = null;
  }

  notice(level, message) {
    const kind = level === "error" ? "alert" : level === "warning" ? "alert" : "info";
    this.append(h("div", { class: `notice ${level}`, role: level === "error" ? "alert" : null }, icon(kind, 14), h("span", {}, message)));
  }

  permissionRequested(env) {
    const card = this.cards.get(env.call_id) ?? this.matchCard(env.tool, env.input);
    if (!(card instanceof ToolCard)) return;
    this.approvals.set(env.request_id, card);
    card.awaitApproval({ id: env.request_id, tool: env.tool, summary: env.summary, input: env.input });
    invalidate("permissions");
  }

  permissionResolved(env) {
    const card = this.approvals.get(env.request_id);
    this.approvals.delete(env.request_id);
    if (!card) return;
    if (env.allowed) card.approved();
    else card.finish("Denied", true);
    invalidate("permissions");
  }

  // Requests pending before this page loaded carry no call id; match them
  // to an unfinished card with the same tool and input.
  matchCard(tool, input) {
    const json = JSON.stringify(input);
    let running = null;
    for (const card of this.cards.values()) {
      if (!(card instanceof ToolCard) || card.done || card.approval || card.name !== tool) continue;
      if (card.inputJson === json) return card;
      running = card;
    }
    // Enabling a plugin asks with its own input and no call id, from inside
    // the malac_configure call that is still running.
    return isPluginApproval({ tool, input }) ? running : null;
  }

  claimPending() {
    for (const request of state.pending.values()) {
      if (request.agent !== this.agent.id || this.approvals.has(request.id)) continue;
      this.permissionRequested({ request_id: request.id, call_id: request.call_id, tool: request.tool, input: request.input, summary: request.summary });
    }
  }

  /** Whether this view shows `requestId` inline, so the banner can skip it. */
  showsRequest(requestId) {
    return this.approvals.has(requestId) && this.root.isConnected;
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
    const standalone = new SpawnCard({ input: { label: child?.label } }, this);
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
      if (card.child === id || card.isAncestorOf(id)) card.refresh();
    }
  }
}

function emptyAgentState(agent) {
  return h(
    "div",
    { class: "transcript-empty" },
    h("p", { class: "transcript-empty-title" }, displayName(agent)),
    h("p", {}, agent.live ? "Waiting for the first message." : "This session has no messages yet."),
  );
}

function messageTools(getText, at) {
  return h(
    "div",
    { class: "msg-tools" },
    at ? h("time", { datetime: at, title: fullTime(at) }, clockTime(at)) : null,
    copyButton(getText, { label: "Copy message", icon: icon("copy", 14) }),
  );
}

function userNode(text, images, at, attachments = []) {
  return h(
    "div",
    { class: "msg user" },
    h("div", { class: "msg-head" }, h("span", { class: "msg-author" }, "You"), messageTools(() => text, at)),
    images.length ? h("div", { class: "thumbs" }, images.map((image) => imageNode(image))) : null,
    text ? h("div", { class: "user-text" }, text) : null,
    attachments.length ? h("div", { class: "attached" }, attachments.map(attachmentChip)) : null,
  );
}

// Prompt attachments: history has an `<attachment path="…">` block per
// file; the live user_message ends with a line `[attached: a.txt, src]`.
const ATTACHMENT_START = /^<attachment path="([^"]*)"[^>]*>/;
const ATTACHED_LINE = /\n?\[attached: ([^\]]*)\]\s*$/;

function parseAttachment(text) {
  const match = ATTACHMENT_START.exec(text);
  const body = text.slice(match[0].length).replace(/<\/attachment>\s*$/, "").trim();
  return { path: match[1], body, images: [] };
}

function splitAttachedLine(text) {
  const match = ATTACHED_LINE.exec(text ?? "");
  if (!match) return { text, names: [] };
  return { text: text.slice(0, match.index), names: match[1].split(",").map((name) => name.trim()).filter(Boolean) };
}

function attachmentChip(attachment) {
  const label = h("span", { class: "attached-name" }, icon("file", 14), baseName(attachment.path) || attachment.path);
  if (!attachment.body && !attachment.images.length) return h("span", { class: "attached-chip", title: attachment.path }, label);
  return h(
    "details",
    { class: "attached-chip" },
    h("summary", { title: attachment.path }, label),
    attachment.body ? h("pre", { class: "output" }, attachment.body) : null,
    attachment.images.length ? h("div", { class: "thumbs" }, attachment.images.map((image) => imageNode(image, "Attached image"))) : null,
  );
}

function assistantNode(text, at) {
  return h("div", { class: "msg assistant" }, markdownNode(text, "assistant"), messageTools(() => text, at));
}

function thinkingNode(text) {
  return h(
    "details",
    { class: "thinking" },
    h("summary", {}, icon("bulb", 14), h("span", {}, "Thinking"), h("span", { class: "thinking-preview" }, truncate(firstLine(text), 120))),
    h("div", { class: "thinking-body" }, text),
  );
}

function recalledNode(text) {
  const body = text.replace(/^<recalled-memories>\s*/, "").replace(/\s*<\/recalled-memories>\s*$/, "");
  return h(
    "details",
    { class: "thinking recalled" },
    h("summary", {}, icon("bookmark", 14), h("span", {}, "Recalled memories")),
    h("div", { class: "thinking-body" }, body),
  );
}

function createCard(use, view) {
  if (use.name === "agent_spawn") return new SpawnCard(use, view);
  if (use.name === "todo") return new TodoCard(use);
  return new ToolCard(use, view);
}

// Tool cards

class ToolCard {
  constructor(use, view) {
    this.name = use.name;
    this.input = use.input ?? {};
    this.inputJson = JSON.stringify(use.input);
    this.view = view;
    this.done = false;
    this.isError = false;
    this.output = null;
    this.images = [];
    this.millis = null;
    this.at = null;
    this.approval = null;
    this.body = null;
    const { title, tag } = toolTitle(this.name);
    const cwd = view?.agent.cwd;
    this.detailText = this.describe(cwd);
    this.state = h("span", { class: "tool-state", "aria-hidden": "true" });
    this.meta = h("span", { class: "tool-meta" });
    this.headButton = h(
      "button",
      { class: "tool-head", type: "button", "aria-expanded": "false" },
      h("span", { class: "tool-icon" }, icon(toolIcon(this.name), 14)),
      h("span", { class: "tool-title" }, title),
      tag ? h("span", { class: "tool-tag" }, tag) : null,
      h("span", { class: "tool-detail", title: this.detailText }, this.detailText),
      this.meta,
      this.state,
      icon("chevronRight", 14, "tool-chevron"),
    );
    this.headButton.addEventListener("click", () => this.toggle());
    this.errorLine = h("div", { class: "tool-error-line", hidden: true });
    this.approvalRow = h("div", { class: "tool-approval", hidden: true });
    this.bodyHost = h("div", { class: "tool-body", hidden: true });
    this.el = h("div", { class: "tool", "data-state": "pending", "data-tool": this.name }, this.headButton, this.errorLine, this.approvalRow, this.bodyHost);
    if (this.name === "edit") this.prepareEdit();
  }

  describe(cwd) {
    const input = this.input;
    switch (this.name) {
      case "read": {
        const range = input.offset ? `:${input.offset}${input.limit ? `+${input.limit}` : ""}` : "";
        return `${relativePath(input.path, cwd)}${range}`;
      }
      case "write":
      case "edit":
        return relativePath(input.path, cwd);
      case "glob":
      case "grep":
        return `${input.pattern ?? ""}${input.path ? ` in ${relativePath(input.path, cwd)}` : ""}`;
      case "bash":
        return firstLine(input.command);
      default:
        return summarizeInput(this.name, input);
    }
  }

  prepareEdit() {
    this.ops = diffLines(String(this.input.old_string ?? ""), String(this.input.new_string ?? ""));
    const { added, removed } = diffStats(this.ops);
    setChildren(this.meta, h("span", { class: "diff-added" }, `+${added}`), h("span", { class: "diff-removed" }, `−${removed}`));
    if (added + removed <= 16) this.toggle(true);
  }

  toggle(open = this.bodyHost.hidden) {
    if (open && !this.body) this.renderBody();
    this.bodyHost.hidden = !open;
    this.headButton.setAttribute("aria-expanded", String(open));
    this.el.classList.toggle("open", open);
  }

  renderBody() {
    this.body = h("div", {});
    this.paintBody();
    this.bodyHost.append(this.body);
  }

  paintBody() {
    if (!this.body) return;
    const input = this.input;
    const parts = [];
    const output = this.output;
    switch (this.name) {
      case "bash":
        if (input.description) parts.push(h("p", { class: "tool-note" }, input.description));
        parts.push(
          section("Command", outputBlock(String(input.command ?? ""), { lang: "sh", prompt: "$ " }), { copy: String(input.command ?? "") }),
        );
        if (input.background || input.timeout) {
          parts.push(
            h(
              "div",
              { class: "tool-flags" },
              input.background ? h("span", { class: "pill" }, "background") : null,
              input.timeout ? h("span", { class: "pill" }, `timeout ${input.timeout}s`) : null,
            ),
          );
        }
        break;
      case "edit":
        parts.push(
          h(
            "div",
            { class: "file-head" },
            icon("file", 14),
            h("span", { class: "file-path", title: input.path }, relativePath(input.path, this.view?.agent.cwd)),
            input.replace_all ? h("span", { class: "pill" }, "replace all") : null,
            copyButton(String(input.path ?? ""), { label: "Copy path", icon: icon("copy", 14) }),
          ),
          diffView(this.ops, languageFor(input.path)),
        );
        break;
      case "write": {
        const content = String(input.content ?? "");
        parts.push(
          h(
            "div",
            { class: "file-head" },
            icon("filePlus", 14),
            h("span", { class: "file-path", title: input.path }, relativePath(input.path, this.view?.agent.cwd)),
            h("span", { class: "muted small tabular" }, plural(content.split("\n").length, "line")),
            copyButton(content, { label: "Copy content", icon: icon("copy", 14) }),
          ),
          outputBlock(content, { lang: languageFor(input.path) }),
        );
        break;
      }
      case "read":
      case "glob":
      case "grep":
      case "web_fetch":
        break;
      default:
        parts.push(section("Input", inputTable(input), { copy: JSON.stringify(input, null, 2) }));
    }
    const onlyImages = output != null && this.images.length > 0 && !output.replace(new RegExp(IMAGE_PLACEHOLDER.source, "g"), "").trim();
    if (output != null && !onlyImages && !(this.name === "edit" && !this.isError) && !(this.name === "write" && !this.isError)) {
      const lang = this.name === "read" ? languageFor(input.path) : "";
      const content =
        this.name === "agent_wait" && !this.isError
          ? markdownNode(output, "small")
          : outputBlock(output || "(no output)", { lang: this.isError ? "" : lang, isError: this.isError });
      parts.push(section(this.isError ? "Error" : "Output", content, { copy: output }));
    } else if (output == null && !this.done) {
      parts.push(h("div", { class: "tool-waiting" }, h("span", { class: "spinner small" }), this.approval ? "Waiting for approval" : "Running"));
    }
    if (this.images.length) {
      const label = /^(browser|computer)_/.test(this.name) ? "Screenshot" : "Image";
      parts.push(
        section(this.images.length > 1 ? `${label}s` : label, h("div", { class: "thumbs" }, this.images.map((image) => imageNode(image, label)))),
      );
    }
    setChildren(this.body, parts);
  }

  start(summary, at) {
    if (at) this.at = at;
    if (this.el.dataset.state === "pending") this.el.dataset.state = "running";
    this.headButton.title = this.at ? `Started ${fullTime(this.at)}` : "";
  }

  awaitApproval(request) {
    this.approval = request;
    this.el.dataset.state = "approval";
    const buttons = h("div", { class: "approval-actions" });
    const choose = (decision) => () => {
      if (!decide(request.id, decision)) return;
      for (const button of buttons.querySelectorAll("button")) button.disabled = true;
    };
    if (isPluginApproval(request)) {
      buttons.append(
        h("button", { class: "button primary small", type: "button", onclick: choose("allow") }, "Enable plugin"),
        h("button", { class: "button small", type: "button", onclick: choose("deny") }, "Keep it off"),
      );
      setChildren(
        this.approvalRow,
        h("span", { class: "approval-text" }, icon("shield", 14), h("span", {}, "Enable the plugin ", h("strong", {}, request.input.plugin), "?")),
        buttons,
        pluginApprovalDetails(request.input),
      );
      this.approvalRow.hidden = false;
      this.toggle(true);
      this.paintBody();
      return;
    }
    buttons.append(
      h("button", { class: "button primary small", type: "button", onclick: choose("allow") }, "Allow"),
      h(
        "button",
        { class: "button small", type: "button", title: "Allow this and similar calls for the rest of the session", onclick: choose("allow_session") },
        "Always allow",
      ),
      h("button", { class: "button small danger", type: "button", onclick: choose("deny") }, "Deny"),
    );
    setChildren(
      this.approvalRow,
      h("span", { class: "approval-text" }, icon("shield", 14), "Approval needed", request.summary ? h("span", { class: "muted" }, ` · ${truncate(request.summary, 120)}`) : null),
      buttons,
    );
    this.approvalRow.hidden = false;
    this.toggle(true);
    this.paintBody();
  }

  approved() {
    this.approval = null;
    this.approvalRow.hidden = true;
    if (this.el.dataset.state === "approval") this.el.dataset.state = "running";
    this.paintBody();
  }

  finish(output, isError, millis) {
    this.done = true;
    this.isError = Boolean(isError);
    this.output = output ?? "";
    this.approval = null;
    this.approvalRow.hidden = true;
    this.el.dataset.state = isError ? "error" : "done";
    if (millis != null) this.millis = millis;
    if (this.name !== "edit") {
      setChildren(this.meta, this.metaText());
    }
    if (this.millis != null) this.meta.append(h("span", { class: "tool-time" }, formatDuration(this.millis)));
    if (isError) {
      this.errorLine.textContent = firstLine(output) || "Failed";
      this.errorLine.hidden = false;
    }
    this.paintBody();
  }

  metaText() {
    if (this.isError || !this.output || IMAGE_PLACEHOLDER.test(this.output)) return null;
    if (this.name === "glob" || this.name === "grep") {
      const count = this.output.trim() ? this.output.trim().split("\n").length : 0;
      return h("span", { class: "tabular" }, plural(count, "result"));
    }
    if (this.name === "read") return h("span", { class: "tabular" }, plural(this.output.split("\n").length, "line"));
    return null;
  }

  result(block) {
    this.finish(resultText(block.content), block.is_error);
    this.addImages(resultImages(block.content));
  }

  addImages(images) {
    if (!images.length) return;
    this.images = images;
    this.meta.querySelector(".image-count")?.remove();
    this.meta.prepend(h("span", { class: "image-count" }, plural(images.length, "image")));
    this.paintBody();
  }

  abandon() {
    if (this.done) return;
    this.el.dataset.state = "skipped";
    this.approvalRow.hidden = true;
    this.paintBody();
  }
}

// Subagent card: follows the live child agent while it works.

class SpawnCard {
  constructor(use, view) {
    this.input = use.input ?? {};
    this.view = view;
    this.label = this.input.label || this.input.agent || "agent";
    this.child = null;
    this.started = false;
    this.done = false;
    this.isError = false;
    this.output = "";
    this.millis = null;
    this.shownReport = null;
    this.dot = statusDot("idle");
    this.status = h("span", { class: "subagent-status" }, "Queued");
    this.meta = h("span", { class: "subagent-meta" });
    this.activity = h("div", { class: "subagent-activity", hidden: true });
    this.progress = h("div", { class: "subagent-progress", hidden: true });
    this.report = h("div", { class: "subagent-report", hidden: true });
    this.openButton = h(
      "button",
      { class: "button small", type: "button", disabled: true, onclick: () => this.open() },
      "Open transcript",
      icon("chevronRight", 14),
    );
    const task = String(this.input.task ?? "");
    this.el = h(
      "div",
      { class: "subagent", "data-status": "idle" },
      h(
        "div",
        { class: "subagent-head" },
        h("span", { class: "tool-icon" }, icon("agents", 14)),
        h("span", { class: "subagent-label" }, this.label),
        this.input.agent && this.input.agent !== this.label ? h("span", { class: "tool-tag" }, this.input.agent) : null,
        this.input.read_only ? h("span", { class: "tool-tag" }, "read-only") : null,
        h("span", { class: "subagent-state" }, this.dot, this.status),
        this.meta,
        this.openButton,
      ),
      task
        ? h(
            "details",
            { class: "subagent-task" },
            h("summary", {}, h("span", { class: "subagent-task-line" }, firstLine(task))),
            h("div", { class: "subagent-task-body" }, task),
          )
        : null,
      this.activity,
      this.progress,
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
    if (!this.done && !this.child) this.status.textContent = "Not started";
  }

  agent() {
    return this.child ? state.agents.get(this.child) : null;
  }

  isAncestorOf(id) {
    if (!this.child) return false;
    for (let agent = state.agents.get(id); agent?.parent; agent = state.agents.get(agent.parent)) {
      if (agent.parent === this.child) return true;
    }
    return false;
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
    this.el.dataset.status = status;
    this.dot.className = `dot ${status}`;
    this.status.textContent = STATUS_TEXT[status] ?? status;
    const meta = [];
    if (agent?.model) meta.push(agent.model);
    if (this.millis != null && status !== "running") meta.push(formatDuration(this.millis));
    else if (status === "running" && agent?.created) meta.push(formatDuration(Date.now() - Date.parse(agent.created)));
    const tokens = agent?.usage ? agent.usage.input_tokens + agent.usage.output_tokens : 0;
    if (tokens) meta.push(`${formatTokens(tokens)} tok`);
    if (agent?.usage?.cost) meta.push(formatCost(agent.usage.cost));
    const nested = agent ? childrenOf(agent.id).length : 0;
    if (nested) meta.push(plural(nested, "subagent"));
    this.meta.textContent = meta.join(" · ");
    const activity = status === "running" ? agent?.activity ?? "" : "";
    if (this.activity.textContent !== activity) this.activity.textContent = activity;
    this.activity.hidden = !activity;
    const progress = todoProgress(agent?.todos);
    this.progress.hidden = !progress || status !== "running";
    if (progress) {
      setChildren(
        this.progress,
        h("span", { class: "bar" }, h("span", { class: "bar-fill", style: { width: `${(progress.done / progress.total) * 100}%` } })),
        h("span", { class: "tabular" }, `${progress.done}/${progress.total}`),
      );
    }
    this.showReport(agent?.report || (this.done ? reportFrom(this.output) || this.output : ""));
  }

  showReport(report) {
    if (report === this.shownReport) return;
    this.shownReport = report;
    setChildren(this.report, report ? markdownNode(report, "small") : "");
    this.report.hidden = !report;
    this.view?.follow();
  }

  open() {
    if (this.child) viewAgent(this.child, { label: this.label, parent: this.view?.agent.id ?? null });
  }
}

// Plan card: the latest todo list stays open, older ones collapse.

class TodoCard {
  constructor(use) {
    const items = Array.isArray(use.input?.items) ? use.input.items : [];
    const progress = todoProgress(items) ?? { done: 0, total: 0 };
    this.errorLine = h("div", { class: "tool-error-line", hidden: true });
    this.el = h(
      "details",
      { class: "plan", open: true },
      h(
        "summary",
        {},
        h("span", { class: "tool-icon" }, icon("todo", 14)),
        h("span", { class: "plan-title" }, "Plan"),
        h("span", { class: "bar" }, h("span", { class: "bar-fill", style: { width: `${progress.total ? (progress.done / progress.total) * 100 : 0}%` } })),
        h("span", { class: "plan-count tabular" }, `${progress.done}/${progress.total}`),
        icon("chevronRight", 14, "tool-chevron"),
      ),
      h(
        "ul",
        { class: "plan-items" },
        items.map((item) =>
          h(
            "li",
            { class: `plan-item ${item.status}` },
            h("span", { class: "plan-mark", "aria-label": item.status.replace("_", " ") }, item.status === "completed" ? icon("check", 12) : null),
            h("span", {}, item.content),
          ),
        ),
      ),
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
    this.el.classList.add("superseded");
  }
}

// One agent's conversation: saved history plus live events, rendered as
// messages, tool cards (with approvals inline), subagent cards and plans.

import type { AgentSummary, FromWebview, PendingView } from "../chat/messages.js";
import type { Block, Envelope, ImageInput, Message, Usage } from "../protocol.js";
import { baseName, clockTime, firstLine, formatCost, formatDuration, formatTokens, h, icon, plural, relativeTo, setChildren, toolIcon, truncate } from "./dom.mjs";
import { renderMarkdown } from "./markdown.mjs";

const PIN_THRESHOLD_PX = 48;
const DRAFT_RENDER_MS = 80;
const PREVIEW_LINES = 20;
const DIFF_CONTEXT = 3;
const DIFF_MAX_CELLS = 250_000;
const IMAGE_TYPES = new Set(["image/png", "image/jpeg", "image/gif", "image/webp"]);
const ATTACHMENT_START = /^<attachment path="([^"]*)"[^>]*>/;
const ATTACHED_LINE = /\n?\[attached: ([^\]]*)\]\s*$/;
const STATUS_TEXT: Record<string, string> = {
  running: "Running",
  waiting: "Awaiting approval",
  completed: "Done",
  failed: "Failed",
  cancelled: "Stopped",
  idle: "Idle",
  saved: "Saved",
};
const TOOL_TITLES: Record<string, string> = {
  bash: "Bash",
  read: "Read",
  write: "Write",
  edit: "Edit",
  glob: "Glob",
  grep: "Grep",
  web_fetch: "Fetch",
  agent_wait: "Wait for subagents",
  agent_spawn: "Subagent",
  todo: "Plan",
};

export interface TranscriptContext {
  post(message: FromWebview): void;
  cwd(): string;
  agents(): AgentSummary[];
}

function toolTitle(name: string): { title: string; tag: string | null } {
  if (TOOL_TITLES[name]) return { title: TOOL_TITLES[name], tag: null };
  if (name.includes("__")) {
    const [server, ...rest] = name.split("__");
    return { title: rest.join("__").replace(/_/g, " "), tag: server };
  }
  const prefix = /^(browser|computer|memory)_/.exec(name)?.[1] ?? null;
  const text = name.replace(/^(browser|computer|memory|agent)_/, "").replace(/_/g, " ");
  return { title: text.charAt(0).toUpperCase() + text.slice(1), tag: prefix };
}

function resultText(content: Block[] | undefined): string {
  return (content ?? []).map((b) => (b.type === "text" ? b.text : b.type === "image" ? `[${b.media_type} image]` : "")).join("");
}

function imageNode(image: { media_type: string; data: string }, alt: string): HTMLElement | null {
  if (!IMAGE_TYPES.has(image.media_type)) return null;
  const src = `data:${image.media_type};base64,${image.data}`;
  const img = h("img", { src, alt, loading: "lazy" });
  return h("a", { class: "thumb", href: src, title: alt, "aria-label": `${alt} (opens full size)`, onclick: (e: Event) => toggleZoom(e, img) }, img);
}

function toggleZoom(event: Event, img: HTMLImageElement): void {
  event.preventDefault();
  img.closest(".thumb")?.classList.toggle("zoomed");
}

function copyButton(text: () => string, label = "Copy"): HTMLButtonElement {
  const button = h("button", { class: "icon-button small", type: "button", title: label, "aria-label": label }, icon("copy", 14));
  button.addEventListener("click", async (event) => {
    event.stopPropagation();
    try {
      await navigator.clipboard.writeText(text());
      setChildren(button, icon("check", 14));
      setTimeout(() => setChildren(button, icon("copy", 14)), 1200);
    } catch {
      // The clipboard can be unavailable; nothing to do.
    }
  });
  return button;
}

/** Monospace output with a "show all" toggle for long text. */
function outputBlock(text: string, { error = false, prompt = "" } = {}): HTMLElement {
  const lines = text.split("\n");
  const long = lines.length > PREVIEW_LINES * 2;
  const code = h("code", {});
  const pre = h("pre", { class: `output${error ? " error" : ""}` }, code);
  const fill = (all: boolean) => {
    code.textContent = `${prompt}${all || !long ? text : lines.slice(0, PREVIEW_LINES).join("\n")}`;
  };
  fill(false);
  if (!long) return pre;
  const more = h("button", { class: "link", type: "button" }, `Show all ${lines.length} lines`);
  let expanded = false;
  more.addEventListener("click", () => {
    expanded = !expanded;
    fill(expanded);
    more.textContent = expanded ? "Show less" : `Show all ${lines.length} lines`;
  });
  return h("div", { class: "output-wrap" }, pre, more);
}

// Line diff of old_string against new_string, common prefix and suffix trimmed first.
export function diffLines(before: string, after: string): { op: " " | "+" | "-"; text: string }[] {
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
  const ops: { op: " " | "+" | "-"; text: string }[] = a.slice(0, start).map((text) => ({ op: " " as const, text }));
  if (midA.length * midB.length > DIFF_MAX_CELLS) {
    ops.push(...midA.map((text) => ({ op: "-" as const, text })), ...midB.map((text) => ({ op: "+" as const, text })));
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
        ops.push({ op: "-", text: midA[i++] });
      } else {
        ops.push({ op: "+", text: midB[j++] });
      }
    }
  }
  ops.push(...a.slice(endA).map((text) => ({ op: " " as const, text })));
  return ops;
}

function diffView(ops: { op: string; text: string }[]): HTMLElement {
  const rows: HTMLElement[] = [];
  const changed = ops.map((o) => o.op !== " ");
  const near = (index: number) => {
    for (let k = Math.max(0, index - DIFF_CONTEXT); k <= Math.min(ops.length - 1, index + DIFF_CONTEXT); k += 1) if (changed[k]) return true;
    return false;
  };
  let hidden = 0;
  const flush = () => {
    if (hidden) rows.push(h("div", { class: "diff-gap" }, plural(hidden, "unchanged line")));
    hidden = 0;
  };
  ops.forEach((o, index) => {
    if (o.op === " " && !near(index)) {
      hidden += 1;
      return;
    }
    flush();
    rows.push(
      h(
        "div",
        { class: `diff-line${o.op === "+" ? " add" : o.op === "-" ? " del" : ""}` },
        h("span", { class: "diff-sign", "aria-hidden": "true" }, o.op === " " ? " " : o.op === "+" ? "+" : "−"),
        h("span", { class: "diff-text" }, o.text || " "),
      ),
    );
  });
  flush();
  return h("div", { class: "diff", role: "group", "aria-label": "Changes" }, rows);
}

// Editor context the extension appends to a prompt: "path, lines 7-13:" (or
// "path (unsaved changes):") followed by a fenced block.
const CONTEXT_BLOCK = /\n*^(.+?)(?:, (lines? (\d+)(?:-(\d+))?)| \(unsaved changes\)):\n\n(`{3,})[^\n]*\n([\s\S]*?)\n\5(?:\n\(cut at \d+ KB\))?\s*(?![\s\S])/m;

export function splitContext(text: string): { typed: string; context: { heading: string; label: string; code: string }[] } {
  const context: { heading: string; label: string; code: string }[] = [];
  let rest = text;
  for (let guard = 0; guard < 4; guard += 1) {
    const match = CONTEXT_BLOCK.exec(rest);
    if (!match) break;
    const [, file, , first, last, , code] = match;
    const label = `${baseName(file)}${first ? `:${first}${last ? `-${last}` : ""}` : ""}`;
    context.unshift({ heading: match[0].trim().split("\n")[0], label, code });
    rest = rest.slice(0, match.index).trimEnd();
  }
  return { typed: rest, context };
}

interface Card {
  el: HTMLElement;
  start(at?: string): void;
  finish(output: string, isError: boolean, millis?: number): void;
  result(block: Extract<Block, { type: "tool_result" }>): void;
  abandon(): void;
}

class ToolCard implements Card {
  readonly name: string;
  readonly input: Record<string, unknown>;
  readonly inputJson: string;
  el: HTMLElement;
  done = false;
  approval: string | null = null;
  private output: string | null = null;
  private isError = false;
  private images: { media_type: string; data: string }[] = [];
  private millis: number | null = null;
  private head: HTMLButtonElement;
  private meta: HTMLElement;
  private approvalRow: HTMLElement;
  private body: HTMLElement;
  private errorLine: HTMLElement;
  private painted = false;
  private ops: { op: " " | "+" | "-"; text: string }[] | null = null;

  constructor(
    use: Extract<Block, { type: "tool_use" }>,
    private readonly view: Transcript,
  ) {
    this.name = use.name;
    this.input = (use.input as Record<string, unknown>) ?? {};
    this.inputJson = JSON.stringify(use.input);
    const { title, tag } = toolTitle(this.name);
    const detail = this.describe();
    this.meta = h("span", { class: "tool-meta" });
    const bodyId = `tool-${Math.random().toString(36).slice(2)}`;
    this.head = h(
      "button",
      { class: "tool-head", type: "button", "aria-expanded": "false", "aria-controls": bodyId },
      h("span", { class: "tool-state", "aria-hidden": "true" }),
      icon(toolIcon(this.name), 14, "tool-icon"),
      h("span", { class: "tool-title" }, title),
      tag ? h("span", { class: "tool-tag" }, tag) : null,
      h("span", { class: "tool-detail", title: detail }, detail),
      this.meta,
      icon("chevronRight", 14, "tool-chevron"),
    );
    this.head.addEventListener("click", () => this.toggle());
    this.errorLine = h("div", { class: "tool-error-line", hidden: true });
    this.approvalRow = h("div", { class: "tool-approval", hidden: true });
    this.body = h("div", { class: "tool-body", id: bodyId, hidden: true });
    this.el = h("div", { class: "tool", dataset: { state: "pending", tool: this.name } }, this.head, this.errorLine, this.approvalRow, this.body);
    if (this.name === "edit") {
      this.ops = diffLines(String(this.input.old_string ?? ""), String(this.input.new_string ?? ""));
      const added = this.ops.filter((o) => o.op === "+").length;
      const removed = this.ops.filter((o) => o.op === "-").length;
      setChildren(this.meta, h("span", { class: "diff-added" }, `+${added}`), h("span", { class: "diff-removed" }, `−${removed}`));
      if (added + removed <= 12) this.toggle(true);
    }
  }

  private path(): string {
    return relativeTo(String(this.input.path ?? ""), this.view.context.cwd());
  }

  describe(): string {
    const input = this.input;
    switch (this.name) {
      case "read": {
        const range = input.offset ? `:${input.offset}${input.limit ? `+${input.limit}` : ""}` : "";
        return `${this.path()}${range}`;
      }
      case "write":
      case "edit":
        return this.path();
      case "glob":
      case "grep":
        return `${input.pattern ?? ""}${input.path ? ` in ${this.path()}` : ""}`;
      case "bash":
        return firstLine(input.command);
      case "agent_wait": {
        const ids = Array.isArray(input.ids) ? input.ids.length : 0;
        return ids ? plural(ids, "agent") : "all subagents";
      }
      default: {
        for (const key of ["command", "path", "pattern", "url", "query", "name", "text", "id", "message"]) {
          if (typeof input[key] === "string") return truncate(firstLine(input[key]), 160);
        }
        const json = JSON.stringify(input);
        return json === "{}" ? "" : truncate(json, 160);
      }
    }
  }

  toggle(open = this.body.hidden): void {
    if (open && !this.painted) this.paint();
    this.body.hidden = !open;
    this.head.setAttribute("aria-expanded", String(open));
    this.el.classList.toggle("open", open);
  }

  private fileHead(path: string, iconName: string, extra: (Node | null)[] = []): HTMLElement {
    return h(
      "div",
      { class: "file-head" },
      icon(iconName, 14),
      h("button", { class: "link file-link", type: "button", title: `Open ${path}`, onclick: () => this.view.context.post({ type: "openFile", path }) }, this.path()),
      ...extra,
    );
  }

  private paint(): void {
    this.painted = true;
    const input = this.input;
    const parts: (Node | null)[] = [];
    switch (this.name) {
      case "bash":
        if (input.description) parts.push(h("p", { class: "tool-note" }, String(input.description)));
        parts.push(outputBlock(String(input.command ?? ""), { prompt: "$ " }));
        if (input.background) parts.push(h("span", { class: "pill" }, "background"));
        break;
      case "edit":
        parts.push(this.fileHead(String(input.path ?? ""), "file", [input.replace_all ? h("span", { class: "pill" }, "replace all") : null]), diffView(this.ops ?? []));
        break;
      case "write": {
        const content = String(input.content ?? "");
        parts.push(this.fileHead(String(input.path ?? ""), "filePlus", [h("span", { class: "muted" }, plural(content.split("\n").length, "line"))]), outputBlock(content));
        break;
      }
      case "read":
        parts.push(this.fileHead(String(input.path ?? ""), "file"));
        break;
      case "glob":
      case "grep":
      case "web_fetch":
        break;
      default:
        parts.push(h("div", { class: "section-label" }, "Input"), outputBlock(JSON.stringify(input, null, 2)));
    }
    const quiet = (this.name === "edit" || this.name === "write") && !this.isError;
    if (this.output !== null && !quiet && !(this.images.length && !this.output.replace(/\[image\/[\w.+-]+ image\]/g, "").trim())) {
      parts.push(
        h("div", { class: "section-label" }, this.isError ? "Error" : "Output", copyButton(() => this.output ?? "", "Copy output")),
        outputBlock(this.output || "(no output)", { error: this.isError }),
      );
    } else if (this.output === null && !this.done) {
      parts.push(h("div", { class: "tool-waiting" }, h("span", { class: "spinner" }), this.approval ? "Waiting for approval" : "Running"));
    }
    if (this.images.length) parts.push(h("div", { class: "thumbs" }, this.images.map((image) => imageNode(image, "Tool image"))));
    setChildren(this.body, parts);
  }

  private repaint(): void {
    if (this.painted) this.paint();
  }

  start(): void {
    if (this.el.dataset.state === "pending") this.el.dataset.state = "running";
  }

  awaitApproval(request: { id: string; summary: string }): void {
    this.approval = request.id;
    this.el.dataset.state = "approval";
    const buttons = h("div", { class: "approval-actions" });
    const choose = (decision: "allow" | "allow_session" | "deny") => () => {
      this.view.context.post({ type: "approve", request: request.id, decision });
      for (const button of buttons.querySelectorAll("button")) button.disabled = true;
    };
    buttons.append(
      h("button", { class: "button primary", type: "button", onclick: choose("allow") }, "Allow"),
      h("button", { class: "button", type: "button", title: "Allow this and similar calls for the rest of the session", onclick: choose("allow_session") }, "Always"),
      h("button", { class: "button", type: "button", onclick: choose("deny") }, "Deny"),
    );
    setChildren(
      this.approvalRow,
      h("span", { class: "approval-text" }, icon("shield", 14), h("span", {}, "Approval needed"), request.summary ? h("span", { class: "muted" }, truncate(request.summary, 140)) : null),
      buttons,
    );
    this.approvalRow.hidden = false;
    this.repaint();
  }

  approved(): void {
    this.approval = null;
    this.approvalRow.hidden = true;
    if (this.el.dataset.state === "approval") this.el.dataset.state = "running";
    this.repaint();
  }

  finish(output: string, isError: boolean, millis?: number): void {
    this.done = true;
    this.isError = Boolean(isError);
    this.output = output ?? "";
    this.approval = null;
    this.approvalRow.hidden = true;
    this.el.dataset.state = isError ? "error" : "done";
    if (millis !== undefined && millis !== null) this.millis = millis;
    if (this.name !== "edit") {
      const meta = this.metaText();
      setChildren(this.meta, meta ? h("span", {}, meta) : null);
    }
    if (this.millis !== null) {
      this.meta.querySelector(".tool-time")?.remove();
      this.meta.append(h("span", { class: "tool-time" }, formatDuration(this.millis)));
    }
    if (isError) {
      this.errorLine.textContent = firstLine(output) || "Failed";
      this.errorLine.hidden = false;
    }
    this.repaint();
  }

  private metaText(): string | null {
    const output = this.output;
    if (this.isError || !output || /\[image\//.test(output)) return null;
    if (this.name === "glob" || this.name === "grep") return plural(output.trim() ? output.trim().split("\n").length : 0, "result");
    if (this.name === "read") return plural(output.split("\n").length, "line");
    return null;
  }

  result(block: Extract<Block, { type: "tool_result" }>): void {
    this.finish(resultText(block.content), Boolean(block.is_error));
    const images = (block.content ?? []).filter((b): b is Extract<Block, { type: "image" }> => b.type === "image");
    if (images.length) {
      this.images = images;
      this.repaint();
    }
  }

  abandon(): void {
    if (this.done) return;
    this.el.dataset.state = "skipped";
    this.approvalRow.hidden = true;
    this.repaint();
  }
}

/** A subagent: its task and live status, with a button to open it. */
class SpawnCard implements Card {
  el: HTMLElement;
  child: string | null = null;
  readonly label: string;
  private statusEl: HTMLElement;
  private open: HTMLButtonElement;
  private finished = false;
  private failed = false;

  constructor(
    use: Extract<Block, { type: "tool_use" }>,
    private readonly view: Transcript,
  ) {
    const input = (use.input as Record<string, unknown>) ?? {};
    this.label = String(input.label ?? input.agent ?? "agent");
    this.statusEl = h("span", { class: "spawn-status" }, "Starting");
    this.open = h("button", { class: "button", type: "button", disabled: true, onclick: () => this.child && this.view.context.post({ type: "openAgent", agent: this.child }) }, "Open");
    this.el = h(
      "div",
      { class: "spawn", dataset: { state: "running" } },
      h("div", { class: "spawn-head" }, h("span", { class: "dot", "aria-hidden": "true" }), icon("agents", 14), h("strong", {}, this.label), this.statusEl, this.open),
      input.task ? h("div", { class: "spawn-task" }, truncate(String(input.task), 400)) : null,
    );
  }

  link(child: string): void {
    this.child = child;
    this.open.disabled = false;
    this.open.setAttribute("aria-label", `Open subagent ${this.label}`);
  }

  refresh(agents: AgentSummary[]): void {
    const child = this.child ? agents.find((a) => a.id === this.child) : undefined;
    if (child) {
      const text = child.status === "running" && child.activity ? child.activity : STATUS_TEXT[child.status] ?? child.status;
      this.statusEl.textContent = truncate(text, 60);
      this.el.dataset.state = child.status;
    } else if (this.finished) {
      this.el.dataset.state = this.failed ? "failed" : "completed";
      this.statusEl.textContent = this.failed ? "Failed" : "Done";
    }
  }

  start(): void {}

  finish(output: string, isError: boolean): void {
    this.finished = true;
    this.failed = isError;
    const id = /\(id ([0-9a-f-]{36})\)/.exec(output ?? "")?.[1];
    if (id && !this.child) this.link(id);
    this.refresh(this.view.context.agents());
  }

  result(block: Extract<Block, { type: "tool_result" }>): void {
    this.finish(resultText(block.content), Boolean(block.is_error));
  }

  abandon(): void {
    if (!this.finished && !this.child) this.el.dataset.state = "cancelled";
  }
}

class TodoCard implements Card {
  el: HTMLElement;

  constructor(use: Extract<Block, { type: "tool_use" }>) {
    const items = (((use.input as { items?: { content: string; status: string }[] }) ?? {}).items ?? []).slice(0, 50);
    const done = items.filter((i) => i.status === "completed").length;
    this.el = h(
      "div",
      { class: "todo" },
      h("div", { class: "todo-head" }, icon("todo", 14), h("strong", {}, "Plan"), h("span", { class: "muted" }, `${done} of ${items.length} done`)),
      h(
        "ul",
        { class: "todo-list" },
        items.map((item) =>
          h(
            "li",
            { class: `todo-item ${item.status}` },
            h("span", { class: "todo-box", "aria-hidden": "true" }, item.status === "completed" ? icon("check", 12) : null),
            h("span", { class: "visually-hidden" }, item.status === "completed" ? "Done: " : item.status === "in_progress" ? "In progress: " : "To do: "),
            item.content,
          ),
        ),
      ),
    );
  }

  supersede(): void {
    this.el.classList.add("superseded");
  }

  start(): void {}
  finish(): void {}
  result(): void {}
  abandon(): void {}
}

interface Draft {
  root: HTMLElement;
  text: string;
  textNode: HTMLElement | null;
  thinking: Text | null;
  preparing: HTMLElement | null;
  timer: ReturnType<typeof setTimeout> | undefined;
}

export class Transcript {
  agent: string | null = null;
  private cards = new Map<string, Card>();
  private approvals = new Map<string, ToolCard>();
  private spawns: SpawnCard[] = [];
  private lastTodo: TodoCard | null = null;
  private draft: Draft | null = null;
  private turn: { started: number; usage: Usage; context: number } | null = null;
  pinned = true;
  private followFrame = 0;
  onPinnedChange: (pinned: boolean) => void = () => {};

  constructor(
    readonly root: HTMLElement,
    private readonly scroller: HTMLElement,
    readonly context: TranscriptContext,
  ) {
    scroller.addEventListener(
      "scroll",
      () => {
        const pinned = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < PIN_THRESHOLD_PX;
        if (pinned !== this.pinned) {
          this.pinned = pinned;
          this.onPinnedChange(pinned);
        }
      },
      { passive: true },
    );
  }

  reset(agent: string | null, history: Message[], events: Envelope[], error?: string): void {
    this.agent = agent;
    this.cards.clear();
    this.approvals.clear();
    this.spawns = [];
    this.lastTodo = null;
    if (this.draft) clearTimeout(this.draft.timer);
    this.draft = null;
    this.turn = null;
    setChildren(this.root);
    if (error) this.notice("error", `Could not load the saved transcript: ${error}`);
    for (const message of history) this.message(message);
    for (const env of events) this.apply(env);
    this.linkChildren(this.context.agents());
    this.pinned = true;
    this.follow(true);
  }

  get isEmpty(): boolean {
    return !this.root.querySelector(".msg, .tool, .spawn, .todo, .notice");
  }

  follow(force = false): void {
    if ((!this.pinned && !force) || this.followFrame) return;
    this.followFrame = requestAnimationFrame(() => {
      this.followFrame = 0;
      this.scroller.scrollTop = this.scroller.scrollHeight;
    });
  }

  jumpToEnd(): void {
    this.pinned = true;
    this.follow(true);
    this.onPinnedChange(true);
  }

  apply(env: Envelope): void {
    switch (env.type) {
      case "user_message":
        this.userMessage(String(env.text ?? ""), (env.images as ImageInput[] | undefined) ?? [], env.at);
        this.turn = { started: Date.parse(env.at), usage: { input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_write_tokens: 0, cost: 0 }, context: 0 };
        break;
      case "turn_resumed":
        if (!this.turn) this.turn = { started: Date.parse(env.at), usage: { input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_write_tokens: 0, cost: 0 }, context: 0 };
        break;
      case "text_delta":
        this.draftText(String(env.text ?? ""));
        break;
      case "thinking_delta":
        this.draftThinking(String(env.text ?? ""));
        break;
      case "tool_call_streaming":
        this.draftTool(String(env.tool ?? ""));
        break;
      case "assistant_message":
        this.clearDraft();
        this.assistantBlocks((env.message as Message).content, env.at);
        break;
      case "tool_started":
        this.cards.get(String(env.call_id))?.start(env.at);
        break;
      case "tool_finished":
        this.cards.get(String(env.call_id))?.finish(String(env.output ?? ""), Boolean(env.is_error), Number(env.millis));
        break;
      case "permission_requested":
        this.permissionRequested({ id: String(env.request_id), callId: String(env.call_id ?? ""), tool: String(env.tool ?? ""), input: env.input, summary: String(env.summary ?? "") });
        break;
      case "permission_resolved":
        this.permissionResolved(String(env.request_id), Boolean(env.allowed));
        break;
      case "usage":
        if (this.turn) {
          const turn = env.turn as Usage;
          for (const key of Object.keys(this.turn.usage) as (keyof Usage)[]) this.turn.usage[key] += Number(turn?.[key] ?? 0);
          this.turn.context = Number(env.context_tokens) || 0;
        }
        break;
      case "compacted":
        this.append(h("details", { class: "divider-block" }, h("summary", {}, icon("compress", 14), "Conversation compacted"), this.markdown(String(env.summary ?? ""), "small")));
        break;
      case "model_changed":
        this.append(h("div", { class: "event-line" }, icon("cpu", 14), "Model changed to ", h("strong", {}, String(env.model ?? ""))));
        break;
      case "notice":
        this.notice(String(env.level ?? "info"), String(env.message ?? ""));
        break;
      case "turn_finished":
        this.turnFinished(env);
        break;
      case "agent_started":
        if (env.parent === this.agent) this.linkChildren(this.context.agents());
        break;
    }
  }

  private append(node: HTMLElement): void {
    if (this.draft && node !== this.draft.root) this.root.insertBefore(node, this.draft.root);
    else this.root.append(node);
    this.follow();
  }

  private markdown(text: string, className = ""): HTMLElement {
    const node = h("div", { class: `markdown ${className}`.trim() });
    node.innerHTML = renderMarkdown(text);
    return node;
  }

  private message(message: Message): void {
    if (message.role === "user") this.userBlocks(message.content);
    else this.assistantBlocks(message.content);
  }

  private userBlocks(blocks: Block[]): void {
    const text: string[] = [];
    const images: { media_type: string; data: string }[] = [];
    const attachments: { path: string; images: { media_type: string; data: string }[] }[] = [];
    let inAttachment = false;
    for (const block of blocks) {
      if (block.type === "tool_result") {
        this.cards.get(block.tool_use_id)?.result(block);
        inAttachment = false;
      } else if (block.type === "image" && inAttachment) {
        attachments.at(-1)!.images.push(block);
      } else if (block.type === "image") {
        images.push(block);
      } else if (block.type === "text" && block.text.startsWith("<recalled-memories>")) {
        this.append(
          h(
            "details",
            { class: "thinking" },
            h("summary", {}, icon("bookmark", 14), "Recalled memories"),
            h("div", { class: "thinking-body" }, block.text.replace(/^<recalled-memories>\s*|\s*<\/recalled-memories>\s*$/g, "")),
          ),
        );
      } else if (block.type === "text" && ATTACHMENT_START.test(block.text)) {
        attachments.push({ path: ATTACHMENT_START.exec(block.text)![1], images: [] });
        inAttachment = true;
        continue;
      } else if (block.type === "text") {
        text.push(block.text);
        inAttachment = false;
      }
    }
    if (text.length || images.length || attachments.length) {
      this.userNode(text.join("\n"), images, null, attachments.map((a) => a.path), attachments.flatMap((a) => a.images));
    }
  }

  private userMessage(text: string, images: ImageInput[], at: string | null): void {
    this.clearDraft();
    const match = ATTACHED_LINE.exec(text);
    const names = match ? match[1].split(",").map((n) => n.trim()).filter(Boolean) : [];
    this.userNode(match ? text.slice(0, match.index) : text, images, at, names, []);
  }

  private userNode(text: string, images: { media_type: string; data: string }[], at: string | null, attached: string[], attachedImages: { media_type: string; data: string }[]): void {
    const all = [...images, ...attachedImages];
    const { typed, context } = splitContext(text.trim());
    this.append(
      h(
        "div",
        { class: "msg user" },
        typed ? h("div", { class: "user-text" }, typed) : null,
        context.map((block) =>
          h(
            "details",
            { class: "context-block" },
            h("summary", { class: "chip static", title: block.heading }, icon("file", 12), block.label),
            h("pre", { class: "output" }, block.code),
          ),
        ),
        all.length ? h("div", { class: "thumbs" }, all.map((image) => imageNode(image, "Attached image"))) : null,
        attached.length
          ? h(
              "div",
              { class: "attached" },
              attached.map((path) => h("span", { class: "chip static", title: path }, icon(/\.\w+$/.test(path) ? "file" : "folder", 12), baseName(path))),
            )
          : null,
        at ? h("time", { class: "msg-time", datetime: at }, clockTime(at)) : null,
      ),
    );
  }

  private assistantBlocks(blocks: Block[], at?: string): void {
    for (const block of blocks ?? []) {
      if (block.type === "text" && block.text.trim()) {
        const node = this.markdown(block.text, "assistant");
        this.append(h("div", { class: "msg assistant" }, node, h("div", { class: "msg-tools" }, at ? h("time", { datetime: at }, clockTime(at)) : null, copyButton(() => block.text, "Copy message"))));
      } else if (block.type === "thinking" && block.text.trim()) {
        this.append(
          h(
            "details",
            { class: "thinking" },
            h("summary", {}, icon("bulb", 14), "Thinking", h("span", { class: "thinking-preview" }, truncate(firstLine(block.text), 100))),
            h("div", { class: "thinking-body" }, block.text),
          ),
        );
      } else if (block.type === "tool_use") {
        this.toolUse(block);
      }
    }
  }

  private toolUse(block: Extract<Block, { type: "tool_use" }>): void {
    let card: Card;
    if (block.name === "agent_spawn") {
      const spawn = new SpawnCard(block, this);
      this.spawns.push(spawn);
      card = spawn;
    } else if (block.name === "todo") {
      const todo = new TodoCard(block);
      this.lastTodo?.supersede();
      this.lastTodo = todo;
      card = todo;
    } else {
      card = new ToolCard(block, this);
    }
    this.cards.set(block.id, card);
    this.append(card.el);
  }

  private ensureDraft(): Draft {
    if (!this.draft) {
      this.draft = { root: h("div", { class: "draft", "aria-busy": "true" }), text: "", textNode: null, thinking: null, preparing: null, timer: undefined };
      this.append(this.draft.root);
    }
    return this.draft;
  }

  private draftText(text: string): void {
    const draft = this.ensureDraft();
    if (!draft.textNode) {
      draft.textNode = h("div", { class: "markdown assistant streaming" });
      draft.root.insertBefore(draft.textNode, draft.preparing);
    }
    draft.text += text;
    if (draft.timer) return;
    draft.timer = setTimeout(() => {
      draft.timer = undefined;
      if (draft.textNode?.isConnected) draft.textNode.innerHTML = renderMarkdown(draft.text);
      this.follow();
    }, DRAFT_RENDER_MS);
  }

  private draftThinking(text: string): void {
    const draft = this.ensureDraft();
    if (!draft.thinking) {
      draft.thinking = document.createTextNode("");
      draft.root.insertBefore(
        h("details", { class: "thinking live" }, h("summary", {}, icon("bulb", 14), "Thinking", h("span", { class: "spinner" })), h("div", { class: "thinking-body" }, draft.thinking)),
        draft.root.firstChild,
      );
    }
    draft.thinking.appendData(text);
    this.follow();
  }

  private draftTool(tool: string): void {
    const draft = this.ensureDraft();
    draft.preparing ??= draft.root.appendChild(h("div", { class: "preparing" }));
    setChildren(draft.preparing, h("span", { class: "spinner" }), `Preparing ${toolTitle(tool).title.toLowerCase()}`);
    this.follow();
  }

  private clearDraft(): void {
    if (this.draft) clearTimeout(this.draft.timer);
    this.draft?.root.remove();
    this.draft = null;
  }

  private turnFinished(env: Envelope): void {
    if (this.draft) {
      clearTimeout(this.draft.timer);
      if (this.draft.textNode) {
        this.draft.textNode.innerHTML = renderMarkdown(this.draft.text);
        this.draft.textNode.classList.remove("streaming");
      }
      this.draft.preparing?.remove();
      this.draft.root.classList.add("interrupted");
      this.draft = null;
    }
    for (const card of this.cards.values()) card.abandon();
    const parts: (Node | string)[] = [];
    const status = String(env.status ?? "completed");
    if (status !== "completed") parts.push(h("span", { class: `turn-status ${status}` }, STATUS_TEXT[status] ?? status));
    if (this.turn) {
      parts.push(h("span", {}, formatDuration(Date.parse(env.at) - this.turn.started)));
      const u = this.turn.usage;
      const tokens = u.input_tokens + u.output_tokens + u.cache_read_tokens + u.cache_write_tokens;
      if (tokens) parts.push(h("span", { title: `Input ${u.input_tokens}, output ${u.output_tokens}, cache read ${u.cache_read_tokens}, cache write ${u.cache_write_tokens}` }, `${formatTokens(tokens)} tokens`));
      if (u.cost) parts.push(h("span", {}, formatCost(u.cost)));
      if (this.turn.context) parts.push(h("span", { title: "Context after this turn" }, `${formatTokens(this.turn.context)} context`));
    }
    if (parts.length) this.append(h("div", { class: "turn-end" }, parts));
    this.turn = null;
  }

  private notice(level: string, message: string): void {
    this.append(h("div", { class: `notice ${level}`, role: level === "error" ? "alert" : null }, icon(level === "info" ? "info" : "alert", 14), h("span", {}, message)));
  }

  private permissionRequested(request: { id: string; callId: string; tool: string; input: unknown; summary: string }): void {
    if (this.approvals.has(request.id)) return;
    let card = this.cards.get(request.callId);
    if (!(card instanceof ToolCard)) card = this.matchCard(request.tool, request.input);
    if (!(card instanceof ToolCard)) return;
    this.approvals.set(request.id, card);
    card.awaitApproval(request);
    if (!card.el.classList.contains("open")) card.toggle(true);
    this.follow();
  }

  private permissionResolved(id: string, allowed: boolean): void {
    const card = this.approvals.get(id);
    this.approvals.delete(id);
    if (!card) return;
    if (allowed) card.approved();
    else card.finish("Denied", true);
  }

  // Requests pending before the view loaded may lack a call id; match by tool and input.
  private matchCard(tool: string, input: unknown): ToolCard | undefined {
    const json = JSON.stringify(input);
    for (const card of this.cards.values()) {
      if (card instanceof ToolCard && !card.done && !card.approval && card.name === tool && card.inputJson === json) return card;
    }
    return undefined;
  }

  /** Shows requests from the snapshot inline when their card is here. */
  claimPending(pending: PendingView[]): void {
    const open = new Set(pending.map((p) => p.id));
    for (const [id, card] of this.approvals) {
      if (!open.has(id)) {
        this.approvals.delete(id);
        card.approved();
      }
    }
    for (const request of pending) {
      if (request.agent === this.agent && !this.approvals.has(request.id)) {
        this.permissionRequested({ id: request.id, callId: request.callId, tool: request.tool, input: request.input, summary: request.summary });
      }
    }
  }

  showsRequest(id: string): boolean {
    return this.approvals.has(id);
  }

  /** Links subagent cards to their agents and refreshes their status. */
  linkChildren(agents: AgentSummary[]): void {
    const children = agents.filter((a) => a.parent === this.agent);
    const linked = new Set(this.spawns.map((s) => s.child));
    for (const card of this.spawns) {
      if (card.child) continue;
      const child = children.find((a) => !linked.has(a.id) && a.label === card.label);
      if (child) {
        card.link(child.id);
        linked.add(child.id);
      }
    }
    for (const card of this.spawns) card.refresh(agents);
  }
}

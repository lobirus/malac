// The composer: the context chip, attachments, the prompt with "@" file
// picking, the model picker, the mode switch, stop and send. Accepts drops
// and pastes of files and images.

import type { ChatState, FromWebview } from "../chat/messages.js";
import type { ImageInput, Mode } from "../protocol.js";
import { baseName, h, icon, relativeTo, setChildren, truncate } from "./dom.mjs";
import { closeMenu, MenuItem, menuButton } from "./menu.mjs";
import { matchCommands, slashQuery, sourceLabel, type Completion } from "./slash.mjs";
import { acceptsDrag, codeDragToUriList, convertTransfer, IMAGE_TYPES, MAX_IMAGE_BYTES, pastedImageName, toBase64 } from "./transfer.mjs";

export const MODES: { mode: Mode; label: string; icon: string; help: string }[] = [
  { mode: "ask", label: "Ask", icon: "hand", help: "Confirm edits and commands" },
  { mode: "edits", label: "Edits", icon: "pencil", help: "Edit files freely, confirm commands" },
  { mode: "autopilot", label: "Autopilot", icon: "zap", help: "Never ask; deny rules still apply" },
];

const HISTORY_LIMIT = 50;

interface PathChip {
  path: string;
  label: string;
}

interface Draft {
  text: string;
  paths: PathChip[];
}

export interface ComposerHost {
  post(message: FromWebview): void;
  state(): ChatState | null;
  toast(level: "info" | "warning" | "error", message: string): void;
  save(draft: Draft): void;
  restore(): Draft | null;
}

export class Composer {
  readonly form: HTMLFormElement;
  private input: HTMLTextAreaElement;
  private chips: HTMLElement;
  private modelButton: HTMLButtonElement;
  private modeButton: HTMLButtonElement;
  private sendButton: HTMLButtonElement;
  private stopButton: HTMLButtonElement;
  private picker: HTMLElement;
  private fileInput: HTMLInputElement;
  private paths: PathChip[] = [];
  private images: (ImageInput & { name: string })[] = [];
  private sending = false;
  private sent: string[] = [];
  private recall = -1;
  private mention: { start: number; query: string; id: number; results: { path: string; relative: string }[]; active: number } | null = null;
  private slash: { query: string; results: Completion[]; active: number } | null = null;
  private searchId = 0;

  constructor(
    form: HTMLFormElement,
    private readonly host: ComposerHost,
    dropZone: HTMLElement,
  ) {
    this.form = form;
    this.chips = h("div", { class: "chips", role: "list", "aria-label": "Context" });
    this.input = h("textarea", {
      class: "prompt",
      rows: "1",
      "aria-label": "Message",
      placeholder: "Ask malac to do something",
      "aria-autocomplete": "list",
      "aria-controls": "mention-list",
    });
    this.picker = h("div", { class: "mention", id: "mention-list", role: "listbox", "aria-label": "Files", hidden: true });
    this.fileInput = h("input", { type: "file", accept: "image/png,image/jpeg,image/gif,image/webp", multiple: true, hidden: true, "aria-hidden": "true", tabindex: "-1" });
    const mentionButton = h(
      "button",
      { class: "icon-button", type: "button", title: "Add a file (@)", "aria-label": "Add a file", onclick: () => this.startMention() },
      icon("at", 16),
    );
    const imageButton = h(
      "button",
      { class: "icon-button", type: "button", title: "Attach an image", "aria-label": "Attach an image", onclick: () => this.fileInput.click() },
      icon("image", 16),
    );
    this.modelButton = menuButton({ class: "pill-button model-button", label: "Model", title: "Model", build: () => this.modelItems() });
    this.modeButton = menuButton({ class: "pill-button mode-button", label: "Permission mode", title: "Permission mode (Shift+Tab to cycle)", build: () => this.modeItems() });
    this.stopButton = h(
      "button",
      { class: "icon-button stop", type: "button", title: "Stop (Esc)", "aria-label": "Stop the agent", hidden: true, onclick: () => this.host.post({ type: "stop" }) },
      icon("stop", 14),
    );
    this.sendButton = h("button", { class: "send", type: "submit", title: "Send (Enter)", "aria-label": "Send" }, icon("send", 16));
    setChildren(
      form,
      this.picker,
      this.chips,
      this.input,
      h("div", { class: "toolbar" }, mentionButton, imageButton, this.modelButton, this.modeButton, h("span", { class: "spacer" }), this.stopButton, this.sendButton),
      this.fileInput,
    );
    this.bind(dropZone);
    const draft = host.restore();
    if (draft) {
      this.input.value = draft.text ?? "";
      this.paths = draft.paths ?? [];
    }
    this.renderChips();
    this.autosize();
  }

  focus(text?: string): void {
    if (text) {
      this.input.value = this.input.value ? `${this.input.value}\n${text}` : text;
      this.autosize();
      this.persist();
    }
    this.input.focus();
    this.input.setSelectionRange(this.input.value.length, this.input.value.length);
  }

  addPaths(paths: PathChip[]): void {
    for (const chip of paths) if (!this.paths.some((p) => p.path === chip.path)) this.paths.push(chip);
    this.renderChips();
    this.persist();
  }

  addImages(images: ImageInput[]): void {
    for (const image of images) this.images.push({ ...image, name: image.name ?? "image" });
    this.renderChips();
  }

  sentResult(ok: boolean, error?: string, notes?: string[]): void {
    this.sending = false;
    this.sendButton.disabled = false;
    this.form.removeAttribute("aria-busy");
    if (!ok) {
      this.host.toast("error", error ?? "Could not send");
      return;
    }
    const text = this.input.value.trim();
    if (text) this.sent = [text, ...this.sent.filter((t) => t !== text)].slice(0, HISTORY_LIMIT);
    this.recall = -1;
    this.input.value = "";
    this.paths = [];
    this.images = [];
    this.renderChips();
    this.autosize();
    this.persist();
    if (notes?.length) this.host.toast("warning", notes.join("; "));
  }

  /** Refreshes the parts that depend on the chat state. */
  update(state: ChatState): void {
    const agent = state.agent;
    const running = Boolean(agent?.running);
    this.stopButton.hidden = !running;
    const tabName = state.tab?.name;
    this.input.placeholder = !state.tab
      ? "Open a tab to start"
      : !agent
        ? `Ask malac to do something in ${tabName}; @ adds files, / runs commands`
        : agent.parent
          ? `Message ${agent.label}`
          : agent.live
            ? running
              ? "Send a follow-up (queued until this turn ends)"
              : "Send a follow-up"
            : "Send a message to continue this session";
    const model = agent ? agent.model : (state.newModel ?? state.defaultModel ?? "");
    const [provider, name] = model.includes("/") ? [model.slice(0, model.indexOf("/")), model.slice(model.indexOf("/") + 1)] : ["", model];
    setChildren(this.modelButton, h("span", { class: "model-name" }, name || "Model"), provider ? h("span", { class: "model-provider" }, provider) : null, icon("chevronDown", 12));
    this.modelButton.title = model ? `Model: ${model}` : "Model";
    this.modelButton.setAttribute("aria-label", this.modelButton.title);
    const mode = MODES.find((m) => m.mode === state.tab?.mode) ?? MODES[0];
    setChildren(this.modeButton, icon(mode.icon, 14), h("span", {}, mode.label));
    this.modeButton.dataset.mode = mode.mode;
    this.modeButton.disabled = !state.tab;
    this.modeButton.setAttribute("aria-label", `Permission mode: ${mode.label}. ${mode.help}`);
    this.renderChips();
  }

  private modelItems(): MenuItem[] {
    const state = this.host.state();
    if (!state) return [];
    const current = state.agent ? state.agent.model : (state.newModel ?? state.defaultModel);
    const models = [...new Set([...(state.models ?? []), ...(current ? [current] : [])])];
    return models.map((model) => ({
      label: model,
      detail: model === state.defaultModel ? "default" : undefined,
      checked: model === current,
      onPick: () => this.host.post({ type: "setModel", model }),
    }));
  }

  private modeItems(): MenuItem[] {
    const state = this.host.state();
    return MODES.map((m) => ({
      label: m.label,
      detail: m.help,
      icon: m.icon,
      checked: state?.tab?.mode === m.mode,
      onPick: () => this.host.post({ type: "setMode", mode: m.mode }),
    }));
  }

  private cycleMode(): void {
    const state = this.host.state();
    if (!state?.tab) return;
    const index = MODES.findIndex((m) => m.mode === state.tab!.mode);
    const next = MODES[(index + 1) % MODES.length];
    this.host.post({ type: "setMode", mode: next.mode });
    this.host.toast("info", `Mode: ${next.label}. ${next.help}`);
  }

  private renderChips(): void {
    const state = this.host.state();
    const items: Node[] = [];
    const chip = state?.context;
    if (chip) {
      const toggle = h(
        "button",
        {
          class: `chip context${chip.enabled ? "" : " off"}`,
          type: "button",
          role: "switch",
          "aria-checked": String(chip.enabled),
          title: chip.title,
          "aria-label": `Current file ${chip.label}${chip.enabled ? ", included" : ", left out"}`,
          onclick: () => this.host.post({ type: "toggleContext", enabled: !chip.enabled }),
        },
        icon(chip.enabled ? "eye" : "eyeOff", 12),
        h("span", { class: "chip-label" }, chip.label),
      );
      items.push(h("span", { role: "listitem" }, toggle));
    }
    const cwd = state?.tab?.cwd ?? "";
    this.paths.forEach((p, index) => {
      items.push(
        h(
          "span",
          { class: "chip", role: "listitem", title: p.path },
          icon(/\.[\w-]+$/.test(p.path) ? "file" : "folder", 12),
          h("span", { class: "chip-label" }, p.label || relativeTo(p.path, cwd) || baseName(p.path)),
          h(
            "button",
            {
              class: "chip-remove",
              type: "button",
              "aria-label": `Remove ${p.label || p.path}`,
              onclick: () => {
                this.paths.splice(index, 1);
                this.renderChips();
                this.persist();
                this.input.focus();
              },
            },
            icon("x", 12),
          ),
        ),
      );
    });
    this.images.forEach((image, index) => {
      items.push(
        h(
          "span",
          { class: "chip image", role: "listitem", title: image.name },
          h("img", { src: `data:${image.media_type};base64,${image.data}`, alt: "" }),
          h("span", { class: "chip-label" }, truncate(image.name, 28)),
          h(
            "button",
            {
              class: "chip-remove",
              type: "button",
              "aria-label": `Remove ${image.name}`,
              onclick: () => {
                this.images.splice(index, 1);
                this.renderChips();
                this.input.focus();
              },
            },
            icon("x", 12),
          ),
        ),
      );
    });
    setChildren(this.chips, items);
    this.chips.hidden = items.length === 0;
  }

  private persist(): void {
    this.host.save({ text: this.input.value, paths: this.paths });
  }

  private autosize(): void {
    this.input.style.height = "auto";
    this.input.style.height = `${Math.min(this.input.scrollHeight, Math.max(120, window.innerHeight * 0.4))}px`;
  }

  private submit(): void {
    if (this.sending) return;
    const text = this.input.value.trim();
    if (!text && !this.paths.length && !this.images.length) return;
    const state = this.host.state();
    if (!state?.tab) {
      this.host.toast("warning", "Open a malac tab for this folder first.");
      return;
    }
    this.sending = true;
    this.sendButton.disabled = true;
    this.form.setAttribute("aria-busy", "true");
    this.closeMention();
    this.host.post({
      type: "submit",
      text,
      attachments: this.paths.map((p) => ({ path: p.path })),
      images: this.images.map(({ media_type, data, name }) => ({ media_type, data, name })),
    });
  }

  // "@" file picking

  private startMention(): void {
    const { selectionStart, value } = this.input;
    const before = value.slice(0, selectionStart);
    const needsSpace = before && !/\s$/.test(before);
    this.input.setRangeText(`${needsSpace ? " " : ""}@`, selectionStart, this.input.selectionEnd, "end");
    this.input.focus();
    this.updateMention();
  }

  private updateMention(): void {
    const caret = this.input.selectionStart;
    const before = this.input.value.slice(0, caret);
    const query = slashQuery(before);
    if (query !== null && this.updateSlash(query)) return;
    this.slash = null;
    const match = /(^|\s)@([^\s@]*)$/.exec(before);
    if (!match) {
      this.closeMention();
      return;
    }
    const start = caret - match[2].length - 1;
    const fileQuery = match[2];
    if (this.mention && this.mention.start === start && this.mention.query === fileQuery) return;
    const id = ++this.searchId;
    this.mention = { start, query: fileQuery, id, results: this.mention?.results ?? [], active: 0 };
    this.host.post({ type: "searchFiles", id, query: fileQuery });
    this.renderMention();
  }

  /** Shows the commands matching `/query`; false when none match. */
  private updateSlash(query: string): boolean {
    const results = matchCommands(this.host.state()?.commands ?? [], query);
    if (!results.length) {
      this.closeMention();
      return true;
    }
    const keep = this.slash?.results[this.slash.active]?.name;
    const active = Math.max(0, results.findIndex((c) => c.name === keep));
    this.mention = null;
    this.slash = { query, results, active };
    this.renderSlash();
    return true;
  }

  private renderSlash(): void {
    const slash = this.slash;
    if (!slash) return;
    this.picker.hidden = false;
    this.picker.setAttribute("aria-label", "Commands");
    this.input.setAttribute("aria-expanded", "true");
    setChildren(
      this.picker,
      slash.results.map((command, index) =>
        h(
          "div",
          {
            id: `mention-${index}`,
            class: `mention-item command${index === slash.active ? " active" : ""}`,
            role: "option",
            "aria-selected": String(index === slash.active),
            title: command.description,
            onmousedown: (event: MouseEvent) => {
              event.preventDefault();
              this.pickCommand(index);
            },
          },
          h("span", { class: "mention-name" }, `/${command.name}`),
          command.hint ? h("span", { class: "command-hint" }, command.hint) : null,
          h("span", { class: "mention-dir" }, command.description),
          sourceLabel(command.source) ? h("span", { class: "command-source" }, sourceLabel(command.source)) : null,
        ),
      ),
    );
    this.input.setAttribute("aria-activedescendant", `mention-${slash.active}`);
    this.picker.querySelector(".active")?.scrollIntoView({ block: "nearest" });
  }

  /** Puts `/name ` in the prompt, ready for its arguments. */
  private pickCommand(index: number): void {
    const command = this.slash?.results[index];
    if (!command) return;
    const caret = this.input.selectionStart;
    const start = this.input.value.indexOf("/");
    this.input.setRangeText(`/${command.name} `, start, caret, "end");
    this.closeMention();
    this.autosize();
    this.persist();
    this.input.focus();
  }

  files(id: number, files: { path: string; relative: string }[]): void {
    if (!this.mention || this.mention.id !== id) return;
    this.mention.results = files;
    this.mention.active = 0;
    this.renderMention();
  }

  private renderMention(): void {
    const mention = this.mention;
    if (!mention) return;
    this.picker.hidden = false;
    this.input.setAttribute("aria-expanded", "true");
    if (!mention.results.length) {
      setChildren(this.picker, h("div", { class: "mention-empty" }, mention.query ? `No files match “${mention.query}”` : "Type to search files"));
      this.input.removeAttribute("aria-activedescendant");
      return;
    }
    setChildren(
      this.picker,
      mention.results.map((file, index) =>
        h(
          "div",
          {
            id: `mention-${index}`,
            class: `mention-item${index === mention.active ? " active" : ""}`,
            role: "option",
            "aria-selected": String(index === mention.active),
            onmousedown: (event: MouseEvent) => {
              event.preventDefault();
              this.pick(index);
            },
          },
          icon("file", 14),
          h("span", { class: "mention-name" }, baseName(file.relative)),
          h("span", { class: "mention-dir" }, file.relative.includes("/") ? file.relative.slice(0, file.relative.lastIndexOf("/")) : ""),
        ),
      ),
    );
    this.input.setAttribute("aria-activedescendant", `mention-${mention.active}`);
    this.picker.querySelector(".active")?.scrollIntoView({ block: "nearest" });
  }

  private pick(index: number): void {
    const mention = this.mention;
    const file = mention?.results[index];
    if (!mention || !file) return;
    const end = mention.start + 1 + mention.query.length;
    this.input.setRangeText(`@${file.relative} `, mention.start, end, "end");
    this.addPaths([{ path: file.path, label: file.relative }]);
    this.closeMention();
    this.autosize();
    this.persist();
    this.input.focus();
  }

  private closeMention(): void {
    this.mention = null;
    this.slash = null;
    this.picker.hidden = true;
    this.picker.setAttribute("aria-label", "Files");
    this.input.setAttribute("aria-expanded", "false");
    this.input.removeAttribute("aria-activedescendant");
  }

  // Events

  private bind(dropZone: HTMLElement): void {
    this.form.addEventListener("submit", (event) => {
      event.preventDefault();
      this.submit();
    });
    this.input.addEventListener("keydown", (event) => this.onKey(event));
    this.input.addEventListener("input", () => {
      this.autosize();
      this.updateMention();
      this.persist();
    });
    this.input.addEventListener("click", () => this.updateMention());
    this.input.addEventListener("blur", () => setTimeout(() => this.closeMention(), 150));
    this.input.addEventListener("paste", (event) => {
      const files = [...(event.clipboardData?.files ?? [])];
      const uriList = event.clipboardData?.getData("text/uri-list") ?? "";
      if (!files.length && !uriList) return;
      event.preventDefault();
      void this.takeTransfer([uriList], files, true);
    });
    this.fileInput.addEventListener("change", () => {
      void this.takeTransfer([], [...(this.fileInput.files ?? [])], false);
      this.fileInput.value = "";
    });
    let depth = 0;
    const leave = () => {
      depth = 0;
      dropZone.classList.remove("dragging");
    };
    dropZone.addEventListener("dragenter", (event) => {
      if (!acceptsDrag([...(event.dataTransfer?.types ?? [])])) return;
      event.preventDefault();
      depth += 1;
      dropZone.classList.add("dragging");
    });
    dropZone.addEventListener("dragover", (event) => {
      if (!acceptsDrag([...(event.dataTransfer?.types ?? [])])) return;
      event.preventDefault();
      if (event.dataTransfer) event.dataTransfer.dropEffect = "copy";
    });
    dropZone.addEventListener("dragleave", () => {
      depth -= 1;
      if (depth <= 0) leave();
    });
    dropZone.addEventListener("drop", (event) => {
      const transfer = event.dataTransfer;
      leave();
      if (!transfer || !acceptsDrag([...transfer.types])) return;
      event.preventDefault();
      const lists = ["application/vnd.code.uri-list", "text/uri-list", "resourceurls", "codefiles"]
        .filter((type) => transfer.types.includes(type))
        .map((type) => codeDragToUriList(type, transfer.getData(type)));
      void this.takeTransfer(lists, [...transfer.files], false);
    });
  }

  /** Attaches what a drop or paste carried. */
  async takeTransfer(uriLists: string[], files: File[], pasted: boolean): Promise<void> {
    const converted = convertTransfer({ uriLists, files });
    const cwd = this.host.state()?.tab?.cwd ?? "";
    if (converted.paths.length) this.addPaths(converted.paths.map((path) => ({ path, label: relativeTo(path, cwd) })));
    const now = new Date();
    for (const file of converted.images) {
      if (!IMAGE_TYPES.has(file.type) || file.size > MAX_IMAGE_BYTES) continue;
      const data = toBase64(await file.arrayBuffer());
      const name = pasted ? pastedImageName(file.type, now, file.name) : file.name || pastedImageName(file.type, now);
      this.images.push({ media_type: file.type, data, name });
    }
    if (converted.images.length) this.renderChips();
    if (converted.uploads.length) {
      const uploads = await Promise.all(converted.uploads.map(async (file) => ({ name: file.name, type: file.type, data: toBase64(await file.arrayBuffer()) })));
      this.host.post({ type: "upload", files: uploads });
    }
    for (const rejected of converted.rejected) this.host.toast("warning", `${rejected.name}: ${rejected.reason}`);
    this.input.focus();
  }

  private onKey(event: KeyboardEvent): void {
    if (event.isComposing) return;
    const slash = this.slash;
    if (slash && !this.picker.hidden) {
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        const delta = event.key === "ArrowDown" ? 1 : -1;
        slash.active = (slash.active + delta + slash.results.length) % slash.results.length;
        this.renderSlash();
        return;
      }
      if ((event.key === "Enter" && !event.shiftKey) || event.key === "Tab") {
        event.preventDefault();
        const command = slash.results[slash.active];
        // A fully typed command without arguments runs right away.
        if (event.key === "Enter" && command?.name === slash.query && !command.hint) {
          this.closeMention();
          this.submit();
        } else {
          this.pickCommand(slash.active);
        }
        return;
      }
      if (event.key === "Escape") {
        event.preventDefault();
        this.closeMention();
        return;
      }
    }
    const mention = this.mention;
    if (mention && !this.picker.hidden && mention.results.length) {
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        const delta = event.key === "ArrowDown" ? 1 : -1;
        mention.active = (mention.active + delta + mention.results.length) % mention.results.length;
        this.renderMention();
        return;
      }
      if (event.key === "Enter" || event.key === "Tab") {
        event.preventDefault();
        this.pick(mention.active);
        return;
      }
    }
    if (mention && event.key === "Escape") {
      event.preventDefault();
      this.closeMention();
      return;
    }
    if (event.key === "Enter" && !event.shiftKey && !event.altKey) {
      event.preventDefault();
      this.submit();
    } else if (event.key === "Tab" && event.shiftKey) {
      event.preventDefault();
      this.cycleMode();
    } else if (event.key === "ArrowUp" && !this.input.value && this.sent.length) {
      event.preventDefault();
      this.recall = Math.min(this.recall + 1, this.sent.length - 1);
      this.input.value = this.sent[this.recall];
      this.autosize();
    } else if (event.key === "ArrowDown" && this.recall >= 0 && this.input.selectionStart === this.input.value.length) {
      event.preventDefault();
      this.recall -= 1;
      this.input.value = this.recall >= 0 ? this.sent[this.recall] : "";
      this.autosize();
    } else if (event.key === "Escape") {
      closeMenu();
      const state = this.host.state();
      if (!this.input.value && state?.agent?.running) {
        event.preventDefault();
        this.host.post({ type: "stop" });
      }
    }
  }
}

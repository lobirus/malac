// Files malac sent with `send_to_user`. Its result names each file in a
// `<shared id="…" name="…" type="…" size="…"/>` tag after an optional
// `<caption>…</caption>`; the card shows the caption and the files
// themselves: images inline (fetched by the extension host, which holds the
// token), other files with Open and Save as….

import type { MediaFile } from "../chat/messages.js";
import type { Block } from "../protocol.js";
import { firstLine, formatBytes, h, icon, setChildren } from "./dom.mjs";
import { openViewer } from "./viewer.mjs";

export interface SharedFile {
  id: string;
  name: string;
  type: string;
  size: number;
}

/** Image types the webview shows inline. */
const INLINE = new Set(["image/png", "image/jpeg", "image/gif", "image/webp", "image/svg+xml", "image/bmp"]);
const TAG = /<shared\s([^>]*?)\/>/g;
const ATTRIBUTE = /([\w-]+)="([^"]*)"/g;
const CAPTION = /<caption>([\s\S]*?)<\/caption>/;

/** The caption and files in a `send_to_user` result; `text` is what is left (the "Sent to the user: …" line). */
export function parseShared(output: string): { caption: string | null; files: SharedFile[]; text: string } {
  const files: SharedFile[] = [];
  for (const match of output.matchAll(TAG)) {
    const attrs = Object.fromEntries([...match[1].matchAll(ATTRIBUTE)].map(([, key, value]) => [key, value]));
    if (!attrs.id || !attrs.name) continue;
    files.push({ id: attrs.id, name: attrs.name, type: attrs.type || "application/octet-stream", size: Number(attrs.size) || 0 });
  }
  const caption = CAPTION.exec(output)?.[1].trim() || null;
  const text = output.replace(TAG, "").replace(CAPTION, "").trim();
  return { caption, files, text };
}

/** "PDF", "Markdown", "PNG image" … for the file card. */
export function kindLabel(file: Pick<SharedFile, "name" | "type">): string {
  const known: Record<string, string> = {
    "application/pdf": "PDF",
    "text/markdown": "Markdown",
    "text/html": "HTML",
    "text/csv": "CSV",
    "application/json": "JSON",
    "application/zip": "ZIP archive",
    "text/plain": "Text",
  };
  if (known[file.type]) return known[file.type];
  const extension = /\.([\w]{1,8})$/.exec(file.name)?.[1];
  if (file.type.startsWith("image/")) return `${(extension ?? file.type.slice(6)).toUpperCase()} image`;
  return extension ? extension.toUpperCase() : "File";
}

export function isInline(file: Pick<SharedFile, "type">): boolean {
  return INLINE.has(file.type);
}

export interface SharedHost {
  /** The file's contents through the extension host. */
  shared(id: string): Promise<{ media_type: string; data: string }>;
  media(action: "open" | "save", file: MediaFile): void;
}

/** The `send_to_user` tool card: the files, not the call. */
export class SharedCard {
  readonly el: HTMLElement;
  private done = false;

  constructor(private readonly host: SharedHost) {
    this.el = h("div", { class: "shared", dataset: { state: "pending" } }, h("div", { class: "event-line" }, h("span", { class: "spinner" }), "Sending files"));
  }

  start(): void {
    if (this.el.dataset.state === "pending") this.el.dataset.state = "running";
  }

  finish(output: string, isError: boolean): void {
    this.done = true;
    this.el.dataset.state = isError ? "error" : "done";
    if (isError) {
      setChildren(this.el, h("div", { class: "shared-error" }, icon("alert", 14), h("span", {}, `Could not send the files: ${firstLine(output) || "failed"}`)));
      return;
    }
    const { caption, files, text } = parseShared(output);
    const images = files.filter(isInline);
    const others = files.filter((f) => !isInline(f));
    setChildren(
      this.el,
      caption ? h("p", { class: "shared-caption" }, caption) : null,
      images.length ? h("div", { class: "shared-images" }, images.map((file) => this.imageNode(file))) : null,
      others.map((file) => this.fileNode(file)),
      // An older daemon's result, or one without tags: show what it said.
      files.length ? null : h("p", { class: "shared-caption muted" }, text || "Nothing was sent."),
    );
  }

  result(block: Extract<Block, { type: "tool_result" }>): void {
    this.finish((block.content ?? []).map((b) => (b.type === "text" ? b.text : "")).join(""), Boolean(block.is_error));
  }

  abandon(): void {
    if (this.done) return;
    this.el.dataset.state = "skipped";
    setChildren(this.el, h("div", { class: "event-line muted" }, icon("x", 14), "Not sent"));
  }

  private imageNode(file: SharedFile): HTMLElement {
    const button = h("button", { class: "thumb shared-thumb loading", type: "button", title: file.name, "aria-label": `${file.name} (loading)`, disabled: true }, h("span", { class: "spinner" }));
    this.host.shared(file.id).then(
      ({ media_type, data }) => {
        const type = INLINE.has(media_type) ? media_type : file.type;
        const src = `data:${type};base64,${data}`;
        const img = h("img", { src, alt: file.name });
        button.classList.remove("loading");
        button.disabled = false;
        button.setAttribute("aria-label", `${file.name}, opens the image viewer`);
        button.addEventListener("click", () => openViewer({ src, id: file.id, name: file.name, media_type: type }, button));
        setChildren(button, img);
      },
      (error: Error) => button.replaceWith(this.fileNode(file, error.message)),
    );
    return button;
  }

  private fileNode(file: SharedFile, problem?: string): HTMLElement {
    const media: MediaFile = { id: file.id, name: file.name, media_type: file.type };
    const open = () => this.host.media("open", media);
    return h(
      "div",
      { class: "shared-file", role: "group", "aria-label": file.name },
      icon(file.type.startsWith("image/") ? "image" : "file", 16, "shared-file-icon"),
      h(
        "div",
        { class: "shared-file-text" },
        h("button", { class: "link shared-file-name", type: "button", title: `Open ${file.name}`, onclick: open }, file.name),
        h("span", { class: "muted" }, problem ?? [kindLabel(file), file.size ? formatBytes(file.size) : ""].filter(Boolean).join(" · ")),
      ),
      h(
        "div",
        { class: "shared-file-actions" },
        h("button", { class: "button", type: "button", onclick: open, "aria-label": `Open ${file.name}` }, "Open"),
        h("button", { class: "button", type: "button", onclick: () => this.host.media("save", media), "aria-label": `Save ${file.name} as` }, "Save as…"),
      ),
    );
  }
}

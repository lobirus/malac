// The full-screen image viewer: every image in the chat (tool results,
// attachments, files malac sent) opens here. It fits the image, zooms
// around the pointer with the wheel or a pinch, pans by dragging,
// double-click toggles 100%, + / - / 0 / 1 work from the keyboard and Esc
// closes it, returning focus. "Open in editor" and "Save as…" go through the
// extension host, which writes the file.

import type { MediaFile } from "../chat/messages.js";
import { h, icon } from "./dom.mjs";

export interface ViewerImage extends MediaFile {
  /** A data URL. */
  src: string;
}

export interface ViewerHost {
  media(action: "open" | "save", file: MediaFile): void;
}

const MAX_SCALE = 16;
const STEP = 1.25;
const PAN_STEP = 48;

let host: ViewerHost = { media: () => {} };
let current: Viewer | null = null;

export function setViewerHost(next: ViewerHost): void {
  host = next;
}

/** Opens the viewer; focus goes back to `opener` when it closes. */
export function openViewer(image: ViewerImage, opener: HTMLElement | null = null): void {
  current?.close();
  current = new Viewer(image, opener ?? (document.activeElement as HTMLElement | null));
}

export function closeViewer(): void {
  current?.close();
}

/** The scale that shows all of a `w`×`h` image in the stage, never above 100%. */
export function fitScale(w: number, h: number, stageW: number, stageH: number): number {
  if (!w || !h || !stageW || !stageH) return 1;
  return Math.min(stageW / w, stageH / h, 1);
}

/** Keeps an image edge from moving inside the stage; a smaller image is centred. */
export function clampOffset(offset: number, size: number, stage: number): number {
  if (size <= stage) return (stage - size) / 2;
  return Math.min(0, Math.max(stage - size, offset));
}

/** The offset that keeps the point under (`at`) still while the scale changes. */
export function zoomOffset(offset: number, at: number, from: number, to: number): number {
  return at - (at - offset) * (to / from);
}

class Viewer {
  private readonly root: HTMLElement;
  private readonly stage: HTMLElement;
  private readonly img: HTMLImageElement;
  private readonly zoomLabel: HTMLButtonElement;
  private width = 0;
  private height = 0;
  private scale = 1;
  private x = 0;
  private y = 0;
  private fitted = true;
  private pointers = new Map<number, { x: number; y: number }>();
  private pinch: { distance: number; scale: number; midX: number; midY: number } | null = null;
  private readonly onResize = () => (this.fitted ? this.fit() : this.apply());

  constructor(
    private readonly image: ViewerImage,
    private readonly opener: HTMLElement | null,
  ) {
    const tool = (name: string, label: string, onclick: () => void) => h("button", { class: "icon-button", type: "button", title: label, "aria-label": label, onclick }, icon(name, 16));
    const center = () => [this.stage.clientWidth / 2, this.stage.clientHeight / 2] as const;
    this.zoomLabel = h("button", { class: "viewer-zoom", type: "button", title: "Actual size (1)", onclick: () => this.zoomTo(1, ...center()) }, "100%");
    this.img = h("img", { class: "viewer-img", alt: image.name, draggable: "false" });
    this.stage = h("div", { class: "viewer-stage" }, this.img);
    this.root = h(
      "div",
      { class: "viewer", role: "dialog", "aria-modal": "true", "aria-label": `Image: ${image.name}`, tabindex: "-1" },
      h(
        "div",
        { class: "viewer-bar" },
        h("span", { class: "viewer-name", title: image.name }, image.name),
        h(
          "span",
          { class: "viewer-tools" },
          tool("zoomOut", "Zoom out (−)", () => this.zoomTo(this.scale / STEP, ...center())),
          this.zoomLabel,
          tool("zoomIn", "Zoom in (+)", () => this.zoomTo(this.scale * STEP, ...center())),
          tool("fit", "Fit to window (0)", () => this.fit()),
          h("span", { class: "viewer-separator", "aria-hidden": "true" }),
          tool("external", "Open in editor", () => host.media("open", this.file())),
          tool("download", "Save as…", () => host.media("save", this.file())),
          tool("x", "Close (Esc)", () => this.close()),
        ),
      ),
      this.stage,
    );
    this.bind();
    document.body.append(this.root);
    this.img.addEventListener("load", () => {
      this.width = this.img.naturalWidth;
      this.height = this.img.naturalHeight;
      this.img.style.width = `${this.width}px`;
      this.img.style.height = `${this.height}px`;
      this.fit();
    });
    this.img.addEventListener("error", () => this.stage.append(h("p", { class: "viewer-error" }, "This image cannot be shown.")));
    this.img.src = image.src;
    this.root.focus();
  }

  private file(): MediaFile {
    const { id, name, media_type, data } = this.image;
    return id ? { id, name, media_type } : { name, media_type, data };
  }

  close(): void {
    if (current === this) current = null;
    window.removeEventListener("resize", this.onResize);
    this.root.remove();
    if (this.opener?.isConnected) this.opener.focus();
  }

  /** The fitted scale, below which zooming out stops. */
  private get minScale(): number {
    return fitScale(this.width, this.height, this.stage.clientWidth, this.stage.clientHeight);
  }

  fit(): void {
    this.scale = this.minScale;
    this.fitted = true;
    this.apply();
  }

  /** Zooms to `next`, keeping the stage point (`atX`, `atY`) where it is. */
  zoomTo(next: number, atX: number, atY: number): void {
    const scale = Math.min(MAX_SCALE, Math.max(this.minScale, next));
    if (!this.width || scale === this.scale) return;
    this.x = zoomOffset(this.x, atX, this.scale, scale);
    this.y = zoomOffset(this.y, atY, this.scale, scale);
    this.scale = scale;
    this.fitted = Math.abs(scale - this.minScale) < 1e-6;
    this.apply();
  }

  private apply(): void {
    const stageW = this.stage.clientWidth;
    const stageH = this.stage.clientHeight;
    const w = this.width * this.scale;
    const hgt = this.height * this.scale;
    this.x = clampOffset(this.x, w, stageW);
    this.y = clampOffset(this.y, hgt, stageH);
    this.img.style.transform = `translate(${this.x}px, ${this.y}px) scale(${this.scale})`;
    // Zoomed screenshots read better as pixels than as blur.
    this.img.classList.toggle("pixelated", this.scale >= 2);
    this.stage.classList.toggle("pannable", w > stageW + 0.5 || hgt > stageH + 0.5);
    const percent = `${Math.round(this.scale * 100)}%`;
    this.zoomLabel.textContent = percent;
    this.zoomLabel.setAttribute("aria-label", `Zoom ${percent}. Actual size`);
  }

  private pan(dx: number, dy: number): void {
    this.x += dx;
    this.y += dy;
    this.apply();
  }

  private point(event: { clientX: number; clientY: number }): [number, number] {
    const rect = this.stage.getBoundingClientRect();
    return [event.clientX - rect.left, event.clientY - rect.top];
  }

  private bind(): void {
    window.addEventListener("resize", this.onResize);
    this.root.addEventListener("keydown", (event) => this.onKey(event));
    this.stage.addEventListener(
      "wheel",
      (event) => {
        event.preventDefault();
        // Trackpad pinches arrive as wheel events with ctrlKey and small deltas.
        const delta = event.deltaY * (event.deltaMode === 1 ? 16 : 1);
        this.zoomTo(this.scale * Math.exp(-delta * (event.ctrlKey ? 0.01 : 0.002)), ...this.point(event));
      },
      { passive: false },
    );
    this.stage.addEventListener("dblclick", (event) => {
      const [atX, atY] = this.point(event);
      if (this.fitted) this.zoomTo(this.minScale >= 1 ? 2 : 1, atX, atY);
      else this.fit();
    });
    this.stage.addEventListener("pointerdown", (event) => {
      if (event.button !== 0) return;
      this.stage.setPointerCapture(event.pointerId);
      this.pointers.set(event.pointerId, { x: event.clientX, y: event.clientY });
      if (this.pointers.size === 2) this.pinch = this.pinchState();
      this.stage.classList.add("dragging");
    });
    this.stage.addEventListener("pointermove", (event) => {
      const last = this.pointers.get(event.pointerId);
      if (!last) return;
      const dx = event.clientX - last.x;
      const dy = event.clientY - last.y;
      this.pointers.set(event.pointerId, { x: event.clientX, y: event.clientY });
      if (this.pinch && this.pointers.size >= 2) {
        const now = this.pinchState();
        const [atX, atY] = this.point({ clientX: now.midX, clientY: now.midY });
        this.x += now.midX - this.pinch.midX;
        this.y += now.midY - this.pinch.midY;
        this.pinch.midX = now.midX;
        this.pinch.midY = now.midY;
        this.zoomTo((this.pinch.scale * now.distance) / this.pinch.distance, atX, atY);
        this.apply();
      } else if (this.pointers.size === 1) {
        this.pan(dx, dy);
      }
    });
    const release = (event: PointerEvent) => {
      this.pointers.delete(event.pointerId);
      if (this.pointers.size < 2) this.pinch = null;
      if (!this.pointers.size) this.stage.classList.remove("dragging");
    };
    this.stage.addEventListener("pointerup", release);
    this.stage.addEventListener("pointercancel", release);
  }

  private pinchState(): { distance: number; scale: number; midX: number; midY: number } {
    const [a, b] = [...this.pointers.values()];
    return { distance: Math.max(1, Math.hypot(a.x - b.x, a.y - b.y)), scale: this.scale, midX: (a.x + b.x) / 2, midY: (a.y + b.y) / 2 };
  }

  private onKey(event: KeyboardEvent): void {
    const [cx, cy] = [this.stage.clientWidth / 2, this.stage.clientHeight / 2];
    const keys: Record<string, () => void> = {
      Escape: () => this.close(),
      "+": () => this.zoomTo(this.scale * STEP, cx, cy),
      "=": () => this.zoomTo(this.scale * STEP, cx, cy),
      "-": () => this.zoomTo(this.scale / STEP, cx, cy),
      _: () => this.zoomTo(this.scale / STEP, cx, cy),
      "0": () => this.fit(),
      "1": () => this.zoomTo(1, cx, cy),
      ArrowLeft: () => this.pan(PAN_STEP, 0),
      ArrowRight: () => this.pan(-PAN_STEP, 0),
      ArrowUp: () => this.pan(0, PAN_STEP),
      ArrowDown: () => this.pan(0, -PAN_STEP),
    };
    if (event.key === "Tab") {
      this.trapFocus(event);
      return;
    }
    const action = keys[event.key];
    if (!action || event.metaKey || event.ctrlKey || event.altKey) return;
    // Enter and Space stay with the focused button.
    event.preventDefault();
    event.stopPropagation();
    action();
  }

  private trapFocus(event: KeyboardEvent): void {
    const buttons = [...this.root.querySelectorAll<HTMLButtonElement>("button:not([disabled])")];
    if (!buttons.length) return;
    const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
    const next = event.shiftKey ? (index <= 0 ? buttons.length - 1 : index - 1) : index === buttons.length - 1 ? 0 : index + 1;
    event.preventDefault();
    buttons[next].focus();
  }
}

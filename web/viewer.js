// The full-screen image viewer, for every image the page shows: tool
// results, prompt attachments, pasted images and files agents send. It
// fits the image to the screen; the wheel, a pinch, a double click or tap
// and + and - zoom around the pointer, dragging pans, and the arrow keys
// (or a swipe) move between the images of one message. Escape, the back
// button or Close leave it, and focus goes back to the thumbnail.

import { h, setChildren, toast } from "./dom.js";
import { icon } from "./icons.js";

const MAX_SCALE = 8;
const STEP = 1.5;
const MARGIN = 16;
const DOUBLE_TAP_MS = 300;
const TAP_SLOP_PX = 8;
const SWIPE_PX = 60;
const CLOSE_SWIPE_PX = 110;
const EXTENSIONS = { "image/png": "png", "image/jpeg": "jpg", "image/gif": "gif", "image/webp": "webp", "image/svg+xml": "svg" };
const FOCUSABLE = "button:not([disabled]):not([hidden])";

// Images are `{name, type, src, blob}`: `src()` gives what an <img> shows
// (a string, or a promise of one) and `blob()` the original file.

/** An image given as base64 data, as tool results and attachments carry them. */
export function dataImage(mediaType, data, name) {
  const src = `data:${mediaType};base64,${data}`;
  let blob = null;
  return {
    name,
    type: mediaType,
    src: () => src,
    blob: () => (blob ??= base64Blob(data, mediaType)),
  };
}

/** An image fetched when first shown: `load()` resolves to its Blob. */
export function blobImage({ name, type, load }) {
  let promise = null;
  let url = null;
  const blob = () =>
    (promise ??= load().catch((error) => {
      promise = null;
      throw error;
    }));
  return {
    name,
    type,
    src: async () => (url ??= URL.createObjectURL(await blob())),
    blob,
  };
}

/** Resolves when `img` has loaded its source (decode() waits while the tab is hidden). */
function loaded(img) {
  if (img.complete && img.naturalWidth) return Promise.resolve();
  return new Promise((resolve, reject) => {
    img.addEventListener("load", () => resolve(), { once: true });
    img.addEventListener("error", () => reject(new Error("the file is not an image this browser can show")), { once: true });
  });
}

function base64Blob(data, type) {
  const bytes = Uint8Array.from(atob(data), (c) => c.charCodeAt(0));
  return new Blob([bytes], { type });
}

/** The image's name with an extension, for saving it. */
function fileName(image) {
  const name = image.name || "image";
  return /\.\w{2,5}$/.test(name) ? name : `${name}.${EXTENSIONS[image.type] ?? "png"}`;
}

/** Saves `blob` as a download called `name`. */
export function saveBlob(blob, name) {
  const url = URL.createObjectURL(blob);
  const link = h("a", { href: url, download: name, hidden: true });
  document.body.append(link);
  link.click();
  link.remove();
  setTimeout(() => URL.revokeObjectURL(url), 60_000);
}

/**
 * Shows a blob (or the blob a promise resolves to) in a new tab. Markup
 * opens as text, so nothing runs on this page's origin. For a promise the
 * tab opens right away, which popup blockers allow, and loads when it is
 * ready.
 */
export async function openBlob(source) {
  const tab = source instanceof Blob ? null : window.open("", "_blank");
  try {
    const blob = await source;
    const scriptable = /^(text\/html|image\/svg\+xml|application\/xhtml\+xml|text\/xml|application\/xml)/i.test(blob.type);
    const url = URL.createObjectURL(scriptable ? new Blob([blob], { type: "text/plain" }) : blob);
    if (tab) {
      tab.opener = null;
      tab.location.href = url;
    } else {
      window.open(url, "_blank", "noopener");
    }
    setTimeout(() => URL.revokeObjectURL(url), 60_000);
  } catch (error) {
    tab?.close();
    throw error;
  }
}

// Thumbnails

const thumbs = new WeakMap();

/**
 * A thumbnail button for `image`. It opens the viewer with every thumbnail
 * of the same message (or tool card, shared files or attachments).
 */
export function thumbnail(image, { label = image.name || "Image" } = {}) {
  const img = h("img", { alt: label, draggable: "false" });
  const button = h("button", { class: "thumb", type: "button", "aria-label": `Open ${label}`, title: image.name || null }, img);
  button.addEventListener("click", () => openFrom(button));
  thumbs.set(button, image);
  const src = image.src();
  if (typeof src === "string") {
    img.loading = "lazy";
    img.src = src;
    return button;
  }
  button.classList.add("loading");
  button.append(h("span", { class: "spinner small", "aria-hidden": "true" }));
  src
    .then((url) => {
      img.src = url;
      return loaded(img);
    })
    .catch((error) => {
      button.disabled = true;
      button.classList.add("broken");
      button.title = error?.message ?? "";
      const name = image.name || "The image";
      setChildren(button, icon("image", 16), h("span", {}, error?.status === 404 ? `${name} is no longer available` : `${name} could not be loaded`));
    })
    .finally(() => {
      button.classList.remove("loading");
      button.querySelector(".spinner")?.remove();
    });
  return button;
}

function openFrom(button) {
  const group = button.closest(".msg, .tool, .share-card, .attachments") ?? button.parentElement;
  const buttons = [...group.querySelectorAll(".thumb")].filter((b) => thumbs.has(b) && !b.disabled);
  openViewer(
    buttons.map((b) => thumbs.get(b)),
    Math.max(0, buttons.indexOf(button)),
  );
}

// The viewer

let current = null;

/** Opens the viewer on `images[index]`. */
function openViewer(images, index = 0) {
  current?.close();
  if (images.length) current = new Viewer(images, index);
}

const canShare = () => typeof navigator.share === "function" && typeof navigator.canShare === "function";
const clamp = (value, min, max) => Math.min(max, Math.max(min, value));

class Viewer {
  constructor(images, index) {
    this.images = images;
    this.index = 0;
    this.previous = document.activeElement;
    this.natural = null;
    this.scale = 1;
    this.fit = 1;
    this.x = 0;
    this.y = 0;
    this.pointers = new Map();
    this.gesture = null;
    this.lastTap = null;
    this.lastPointer = "mouse";
    this.ticket = 0;
    this.closed = false;

    const tool = (name, label, onclick, className = "") =>
      h("button", { class: `icon-button viewer-button ${className}`.trim(), type: "button", "aria-label": label, title: label, onclick }, icon(name, 18));
    const labelled = (name, label, title, onclick) =>
      h("button", { class: "button small viewer-action", type: "button", "aria-label": title, title, onclick }, icon(name, 14), h("span", { class: "viewer-action-label" }, label));
    this.name = h("span", { class: "viewer-name" });
    this.count = h("span", { class: "viewer-count tabular" });
    this.zoomOut = tool("zoomOut", "Zoom out (-)", () => this.zoomBy(1 / STEP), "viewer-step");
    this.zoomIn = tool("zoomIn", "Zoom in (+)", () => this.zoomBy(STEP), "viewer-step");
    this.zoomLabel = h(
      "button",
      { class: "button ghost small viewer-zoom tabular", type: "button", title: "Fit to the screen or show actual size (0, double-click)", onclick: () => this.toggleZoom() },
      "100%",
    );
    const bar = h(
      "div",
      { class: "viewer-bar" },
      h("div", { class: "viewer-title" }, this.name, this.count),
      h(
        "div",
        { class: "viewer-tools" },
        this.zoomOut,
        this.zoomLabel,
        this.zoomIn,
        h("span", { class: "viewer-sep", "aria-hidden": "true" }),
        canShare() ? tool("share", "Share", () => this.share()) : null,
        labelled("download", "Download", "Download", () => this.download()),
        labelled("external", "Open original", "Open the original in a new tab", () => this.openOriginal()),
        tool("x", "Close (Esc)", () => this.close()),
      ),
    );
    this.image = h("img", { class: "viewer-image", alt: "", draggable: "false", hidden: true });
    this.status = h("div", { class: "viewer-status", role: "status" });
    this.stage = h("div", { class: "viewer-stage" }, this.image, this.status);
    this.prev = h("button", { class: "icon-button viewer-nav prev", type: "button", "aria-label": "Previous image (←)", title: "Previous image (←)", onclick: () => this.go(-1) }, icon("chevronLeft", 20));
    this.next = h("button", { class: "icon-button viewer-nav next", type: "button", "aria-label": "Next image (→)", title: "Next image (→)", onclick: () => this.go(1) }, icon("chevronRight", 20));
    this.root = h("div", { class: "viewer", role: "dialog", "aria-modal": "true", "aria-label": "Image viewer", tabindex: "-1" }, bar, this.stage, this.prev, this.next);

    this.onKey = (event) => this.key(event);
    this.onResize = () => this.resized();
    this.onPop = () => this.close({ fromHistory: true });
    this.stage.addEventListener("pointerdown", (event) => this.down(event));
    this.stage.addEventListener("pointermove", (event) => this.move(event));
    this.stage.addEventListener("pointerup", (event) => this.up(event));
    this.stage.addEventListener("pointercancel", (event) => this.up(event, true));
    this.stage.addEventListener("wheel", (event) => this.wheel(event), { passive: false });
    // With the pointer captured, the stage is the target; hit-test the image instead.
    this.stage.addEventListener("dblclick", (event) => {
      const p = this.point(event);
      if (this.lastPointer !== "touch" && this.onImage(p)) this.toggleZoom(p.x, p.y);
    });
    this.image.addEventListener("contextmenu", (event) => this.lastPointer === "touch" && event.preventDefault());

    document.body.append(this.root);
    document.addEventListener("keydown", this.onKey, true);
    window.addEventListener("resize", this.onResize);
    // The back button (or a phone's back gesture) closes the viewer. An
    // entry left by a viewer open during a reload is used again.
    this.pushed = !history.state?.malacViewer;
    if (this.pushed) history.pushState({ malacViewer: true }, "");
    window.addEventListener("popstate", this.onPop);
    this.root.focus();
    this.show(index);
  }

  close({ fromHistory = false } = {}) {
    if (this.closed) return;
    this.closed = true;
    this.ticket += 1;
    this.root.remove();
    document.removeEventListener("keydown", this.onKey, true);
    window.removeEventListener("resize", this.onResize);
    window.removeEventListener("popstate", this.onPop);
    if (current === this) current = null;
    if (!fromHistory && history.state?.malacViewer) {
      if (this.pushed) history.back();
      else history.replaceState(null, "");
    }
    if (this.previous?.isConnected) this.previous.focus({ preventScroll: true });
    else document.getElementById("content")?.focus({ preventScroll: true });
  }

  go(step) {
    if (this.images.length > 1) this.show(this.index + step);
  }

  async show(index) {
    const count = this.images.length;
    this.index = ((index % count) + count) % count;
    const image = this.images[this.index];
    const ticket = ++this.ticket;
    this.natural = null;
    this.apply(false);
    this.name.textContent = image.name || "Image";
    this.name.title = image.name || "";
    this.count.textContent = count > 1 ? `${this.index + 1} of ${count}` : "";
    this.root.setAttribute("aria-label", count > 1 ? `${image.name || "Image"}, ${this.index + 1} of ${count}` : image.name || "Image");
    this.prev.hidden = count < 2;
    this.next.hidden = count < 2;
    this.image.hidden = true;
    this.setStatus(h("span", { class: "spinner" }), h("span", {}, "Loading image"));
    try {
      const src = await image.src();
      if (ticket !== this.ticket) return;
      this.image.alt = image.name || "Image";
      this.image.src = src;
      await loaded(this.image);
      if (ticket !== this.ticket) return;
      this.natural = { width: this.image.naturalWidth || 1, height: this.image.naturalHeight || 1 };
      this.image.style.width = `${this.natural.width}px`;
      this.image.style.height = `${this.natural.height}px`;
      this.image.hidden = false;
      this.setStatus();
      this.reset(false);
    } catch (error) {
      if (ticket !== this.ticket) return;
      this.setStatus(icon("image", 24), h("strong", {}, "This image could not be loaded"), error?.message ? h("span", { class: "muted small" }, error.message) : null);
    }
  }

  setStatus(...children) {
    setChildren(this.status, children);
    this.status.hidden = children.length === 0;
  }

  // Geometry: the image sits at (x, y) in the stage, scaled by `scale`
  // around its top left corner.

  size() {
    return { width: this.stage.clientWidth, height: this.stage.clientHeight };
  }

  point(event) {
    const rect = this.stage.getBoundingClientRect();
    return { x: event.clientX - rect.left, y: event.clientY - rect.top };
  }

  onImage(p) {
    if (!this.natural) return false;
    return p.x >= this.x && p.y >= this.y && p.x <= this.x + this.natural.width * this.scale && p.y <= this.y + this.natural.height * this.scale;
  }

  zoomed() {
    return this.natural != null && this.scale > this.fit * 1.001;
  }

  /** Back to the whole image on the screen, no larger than its own size. */
  reset(animate = true) {
    if (!this.natural) return;
    const { width, height } = this.size();
    this.fit = Math.min((width - MARGIN * 2) / this.natural.width, (height - MARGIN * 2) / this.natural.height, 1);
    this.fit = Math.max(this.fit, 0.01);
    this.scale = this.fit;
    this.place();
    this.apply(animate);
  }

  resized() {
    if (!this.natural) return;
    if (!this.zoomed()) return this.reset(false);
    const { width, height } = this.size();
    this.fit = Math.max(0.01, Math.min((width - MARGIN * 2) / this.natural.width, (height - MARGIN * 2) / this.natural.height, 1));
    this.scale = Math.max(this.scale, this.fit);
    this.place();
    this.apply(false);
  }

  /** Centres what is smaller than the stage and keeps the rest from leaving an edge. */
  place() {
    const { width, height } = this.size();
    const w = this.natural.width * this.scale;
    const h2 = this.natural.height * this.scale;
    this.x = w <= width ? (width - w) / 2 : clamp(this.x, width - w, 0);
    this.y = h2 <= height ? (height - h2) / 2 : clamp(this.y, height - h2, 0);
  }

  zoomAt(target, cx, cy, animate = false) {
    if (!this.natural) return;
    const scale = clamp(target, this.fit, Math.max(MAX_SCALE, this.fit));
    const px = (cx - this.x) / this.scale;
    const py = (cy - this.y) / this.scale;
    this.scale = scale;
    this.x = cx - px * scale;
    this.y = cy - py * scale;
    this.place();
    this.apply(animate);
  }

  zoomBy(factor) {
    const { width, height } = this.size();
    this.zoomAt(this.scale * factor, width / 2, height / 2, true);
  }

  /** Between fitting the screen and actual size (twice the size for small images). */
  toggleZoom(cx, cy) {
    if (!this.natural) return;
    if (this.zoomed()) return this.reset();
    const { width, height } = this.size();
    this.zoomAt(this.fit < 1 ? 1 : 2, cx ?? width / 2, cy ?? height / 2, true);
  }

  apply(animate) {
    this.stage.classList.toggle("animate", Boolean(animate));
    this.image.style.transform = `translate(${this.x}px, ${this.y}px) scale(${this.scale})`;
    this.stage.classList.toggle("zoomed", this.zoomed());
    const ready = this.natural != null;
    this.zoomLabel.textContent = ready ? `${Math.round(this.scale * 100)}%` : "–";
    this.zoomLabel.disabled = !ready;
    this.zoomOut.disabled = !ready || !this.zoomed();
    this.zoomIn.disabled = !ready || this.scale >= Math.max(MAX_SCALE, this.fit) * 0.999;
  }

  // Pointers: one drags (or swipes at fit, on touch), two pinch.

  down(event) {
    if (!this.natural || (event.pointerType === "mouse" && event.button !== 0)) return;
    this.lastPointer = event.pointerType;
    this.stage.setPointerCapture?.(event.pointerId);
    this.pointers.set(event.pointerId, this.point(event));
    this.stage.classList.remove("animate");
    if (this.pointers.size === 1) this.startPan(this.point(event), event);
    else if (this.pointers.size === 2) this.startPinch();
  }

  startPan(start, event) {
    this.gesture = { kind: "pan", start, x: this.x, y: this.y, type: event?.pointerType ?? "touch", moved: !event };
    this.stage.classList.toggle("dragging", this.zoomed());
  }

  startPinch() {
    const [a, b] = [...this.pointers.values()];
    const mid = { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 };
    this.gesture = {
      kind: "pinch",
      distance: Math.max(1, Math.hypot(a.x - b.x, a.y - b.y)),
      scale: this.scale,
      px: (mid.x - this.x) / this.scale,
      py: (mid.y - this.y) / this.scale,
    };
    this.lastTap = null;
  }

  move(event) {
    if (!this.pointers.has(event.pointerId)) return;
    const p = this.point(event);
    this.pointers.set(event.pointerId, p);
    const g = this.gesture;
    if (g?.kind === "pinch" && this.pointers.size >= 2) {
      const [a, b] = [...this.pointers.values()];
      const mid = { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 };
      const scale = clamp((g.scale * Math.hypot(a.x - b.x, a.y - b.y)) / g.distance, this.fit, Math.max(MAX_SCALE, this.fit));
      this.scale = scale;
      this.x = mid.x - g.px * scale;
      this.y = mid.y - g.py * scale;
      this.place();
      this.apply(false);
    } else if (g?.kind === "pan") {
      const dx = p.x - g.start.x;
      const dy = p.y - g.start.y;
      if (Math.hypot(dx, dy) > TAP_SLOP_PX) g.moved = true;
      if (this.zoomed()) {
        this.x = g.x + dx;
        this.y = g.y + dy;
        this.place();
      } else if (g.type === "touch") {
        // At fit a finger swipes: sideways to the next image, down to close.
        this.x = g.x + (this.images.length > 1 ? dx : 0);
        this.y = g.y + Math.max(0, dy);
      }
      this.apply(false);
    }
  }

  up(event, cancelled = false) {
    if (!this.pointers.has(event.pointerId)) return;
    const p = this.point(event);
    this.pointers.delete(event.pointerId);
    const g = this.gesture;
    this.stage.classList.remove("dragging");
    if (g?.kind === "pinch") {
      // Lifting one finger of a pinch keeps panning with the other.
      if (this.pointers.size === 1) this.startPan([...this.pointers.values()][0], null);
      else this.gesture = null;
      return;
    }
    this.gesture = null;
    if (g?.kind !== "pan" || cancelled) return;
    const dx = p.x - g.start.x;
    const dy = p.y - g.start.y;
    if (g.moved) {
      if (!this.zoomed() && g.type === "touch") {
        if (this.images.length > 1 && Math.abs(dx) > SWIPE_PX && Math.abs(dx) > Math.abs(dy)) return this.go(dx < 0 ? 1 : -1);
        if (dy > CLOSE_SWIPE_PX && dy > Math.abs(dx)) return this.close();
        this.place();
        this.apply(true);
      }
      return;
    }
    if (g.type === "mouse") {
      // A click beside the image closes, as with any lightbox.
      if (!this.onImage(g.start)) this.close();
      return;
    }
    const tap = { at: event.timeStamp, x: p.x, y: p.y };
    const last = this.lastTap;
    if (last && tap.at - last.at < DOUBLE_TAP_MS && Math.hypot(tap.x - last.x, tap.y - last.y) < 30) {
      this.lastTap = null;
      this.toggleZoom(p.x, p.y);
    } else {
      this.lastTap = tap;
    }
  }

  wheel(event) {
    event.preventDefault();
    if (!this.natural) return;
    const unit = event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? this.size().height : 1;
    // Trackpad pinches arrive as wheel events with ctrlKey and small deltas.
    const factor = Math.exp(-event.deltaY * unit * (event.ctrlKey ? 0.01 : 0.002));
    const p = this.point(event);
    this.zoomAt(this.scale * factor, p.x, p.y);
  }

  key(event) {
    if (event.isComposing) return;
    const mod = event.ctrlKey || event.metaKey || event.altKey;
    if (event.key === "Escape") {
      this.close();
    } else if (event.key === "Tab") {
      const focusable = [...this.root.querySelectorAll(FOCUSABLE)].filter((node) => node.offsetParent !== null);
      if (!focusable.length) return;
      const first = focusable[0];
      const last = focusable.at(-1);
      const inside = this.root.contains(document.activeElement) && document.activeElement !== this.root;
      if (event.shiftKey && (!inside || document.activeElement === first)) last.focus();
      else if (!event.shiftKey && (!inside || document.activeElement === last)) first.focus();
      else return;
    } else if (!mod && (event.key === "+" || event.key === "=")) {
      this.zoomBy(STEP);
    } else if (!mod && (event.key === "-" || event.key === "_")) {
      this.zoomBy(1 / STEP);
    } else if (!mod && event.key === "0") {
      this.reset();
    } else if (!mod && (event.key === "ArrowRight" || event.key === "ArrowLeft") && this.images.length > 1) {
      this.go(event.key === "ArrowRight" ? 1 : -1);
    } else {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
  }

  // The original file

  async original() {
    try {
      return await this.images[this.index].blob();
    } catch (error) {
      toast(`The image could not be loaded: ${error.message}`, "error");
      return null;
    }
  }

  async download() {
    const blob = await this.original();
    if (blob) saveBlob(blob, fileName(this.images[this.index]));
  }

  async openOriginal() {
    const blob = await this.original();
    if (blob) openBlob(blob);
  }

  async share() {
    const image = this.images[this.index];
    const blob = await this.original();
    if (!blob) return;
    const file = new File([blob], fileName(image), { type: blob.type || image.type });
    if (!navigator.canShare({ files: [file] })) {
      toast("This browser cannot share images. Use Download instead.", "warning");
      return;
    }
    try {
      await navigator.share({ files: [file], title: image.name || undefined });
    } catch (error) {
      if (error.name !== "AbortError") toast(`Could not share: ${error.message}`, "error");
    }
  }
}

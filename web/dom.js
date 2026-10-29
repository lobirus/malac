// DOM construction, formatting and small UI primitives shared by every view.

export function h(tag, props, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(props ?? {})) {
    if (value == null || value === false) continue;
    if (key.startsWith("on") && typeof value === "function") node.addEventListener(key.slice(2), value);
    else if (key === "class") node.className = value;
    else if (key === "value") node.value = value;
    else if (key === "checked") node.checked = Boolean(value);
    else if (key === "dataset") Object.assign(node.dataset, value);
    else if (key === "style" && typeof value === "object") Object.assign(node.style, value);
    else node.setAttribute(key, value === true ? "" : String(value));
  }
  return appendChildren(node, children);
}

export function appendChildren(node, children) {
  for (const child of children.flat(Infinity)) {
    if (child != null && child !== false) node.append(child);
  }
  return node;
}

export function setChildren(node, ...children) {
  node.replaceChildren();
  return appendChildren(node, children);
}

// Text

const HTML_ESCAPES = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" };

export function escapeHtml(text) {
  return String(text).replace(/[&<>"']/g, (c) => HTML_ESCAPES[c]);
}

export function truncate(text, max) {
  const value = String(text ?? "");
  return value.length > max ? `${value.slice(0, max - 1)}…` : value;
}

export function firstLine(text) {
  return String(text ?? "").trim().split("\n", 1)[0];
}

export function baseName(path) {
  const parts = String(path ?? "").split("/").filter(Boolean);
  return parts.at(-1) ?? String(path ?? "");
}

let userHome = null;

/** Set from the daemon snapshot so paths under the user's home read as `~/…`. */
export function setUserHome(home) {
  userHome = home ? String(home).replace(/\/+$/, "") : null;
}

/** A path with the user's home replaced by `~`. */
export function displayPath(path) {
  const value = String(path ?? "");
  if (userHome && (value === userHome || value.startsWith(`${userHome}/`))) {
    return `~${value.slice(userHome.length)}`;
  }
  return value;
}

/** `~/x` as an absolute path, when the home directory is known. */
export function expandHome(path) {
  const value = String(path ?? "");
  if (userHome && (value === "~" || value.startsWith("~/"))) return `${userHome}${value.slice(1)}`;
  return value;
}

export function shortPath(path, keep = 2) {
  const value = displayPath(path);
  const parts = value.split("/").filter(Boolean);
  return parts.length > keep ? `…/${parts.slice(-keep).join("/")}` : value;
}

export function plural(count, word, many = `${word}s`) {
  return `${count} ${count === 1 ? word : many}`;
}

/** `512 B`, `2.1 KB`, `3.0 MB`, as the daemon writes sizes. */
export function byteSize(bytes) {
  const units = ["B", "KB", "MB", "GB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return unit === 0 ? `${bytes} B` : `${value.toFixed(1)} ${units[unit]}`;
}

// Numbers and time

function trimZero(number) {
  return number.replace(/\.0$/, "");
}

export function formatTokens(n) {
  const value = Number(n) || 0;
  if (value < 1000) return String(value);
  if (value < 1_000_000) return `${trimZero((value / 1000).toFixed(1))}k`;
  return `${trimZero((value / 1_000_000).toFixed(1))}M`;
}

export function formatCost(cost) {
  if (!cost) return "";
  return `$${cost < 0.01 ? cost.toFixed(4) : cost.toFixed(2)}`;
}

export function formatDuration(ms) {
  if (ms < 1000) return `${Math.max(0, Math.round(ms))}ms`;
  if (ms < 60_000) return `${trimZero((ms / 1000).toFixed(1))}s`;
  const seconds = Math.round(ms / 1000);
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ${String(seconds % 60).padStart(2, "0")}s`;
  return `${Math.floor(seconds / 3600)}h ${String(Math.floor((seconds % 3600) / 60)).padStart(2, "0")}m`;
}

export function relativeTime(iso) {
  const seconds = (Date.now() - Date.parse(iso)) / 1000;
  if (!Number.isFinite(seconds)) return "";
  if (seconds < 45) return "just now";
  if (seconds < 3600) return `${Math.max(1, Math.round(seconds / 60))}m ago`;
  if (seconds < 86_400) return `${Math.floor(seconds / 3600)}h ago`;
  if (seconds < 604_800) return `${Math.floor(seconds / 86_400)}d ago`;
  return new Date(iso).toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

export function clockTime(iso) {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "";
  return date.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
}

export function fullTime(iso) {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "";
  return date.toLocaleString(undefined, { dateStyle: "medium", timeStyle: "medium" });
}

/** Sidebar group for a timestamp: Today, Yesterday, This week, … */
export function dateGroup(iso) {
  const date = new Date(iso);
  const today = new Date();
  today.setHours(0, 0, 0, 0);
  const days = Math.floor((today - new Date(date).setHours(0, 0, 0, 0)) / 86_400_000);
  if (days <= 0) return "Today";
  if (days === 1) return "Yesterday";
  if (days < 7) return "Previous 7 days";
  if (days < 30) return "Previous 30 days";
  return date.toLocaleDateString(undefined, { month: "long", year: "numeric" });
}

export function emptyUsage() {
  return { input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_write_tokens: 0, cost: 0 };
}

export function addUsage(total, usage) {
  for (const key of Object.keys(total)) total[key] += usage?.[key] ?? 0;
  return total;
}

export function usageParts(usage, contextTokens) {
  if (!usage) return [];
  const parts = [`${formatTokens(usage.input_tokens)} in`, `${formatTokens(usage.output_tokens)} out`];
  if (usage.cache_read_tokens) parts.push(`${formatTokens(usage.cache_read_tokens)} cached`);
  if (usage.cost) parts.push(formatCost(usage.cost));
  if (contextTokens) parts.push(`${formatTokens(contextTokens)} context`);
  return parts;
}

export function totalTokens(usage) {
  if (!usage) return 0;
  return usage.input_tokens + usage.output_tokens + usage.cache_read_tokens + usage.cache_write_tokens;
}

// Data

export function clone(value) {
  return value == null ? value : JSON.parse(JSON.stringify(value));
}

// Storage that never throws: sandboxed frames and private windows may refuse it.

function safeStorage(kind) {
  return {
    get(key) {
      try {
        return window[kind].getItem(key);
      } catch {
        return null;
      }
    },
    set(key, value) {
      try {
        if (value == null) window[kind].removeItem(key);
        else window[kind].setItem(key, value);
      } catch {
        // Not persisted; the value only lives for this page.
      }
    },
  };
}

export const local = safeStorage("localStorage");
export const sessionStore = safeStorage("sessionStorage");

// Feedback

const TOAST_MS = 6000;

export function toast(message, level = "info", options = {}) {
  const host = document.getElementById("toasts");
  if (!host) return;
  const close = () => node.remove();
  const node = h(
    "div",
    { class: `toast ${level}`, role: level === "error" ? "alert" : "status" },
    h("span", { class: "toast-text" }, message),
    options.action
      ? h("button", { class: "toast-action", type: "button", onclick: () => (options.action.run(), close()) }, options.action.label)
      : null,
    h("button", { class: "toast-close", type: "button", "aria-label": "Dismiss", onclick: close }, "×"),
  );
  host.append(node);
  setTimeout(close, options.duration ?? (level === "error" ? TOAST_MS * 2 : TOAST_MS));
}

export async function copyText(text) {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    // Clipboard API is unavailable in insecure or embedded contexts; fall back to a selection copy.
    const area = h("textarea", { class: "copy-buffer", readonly: true }, text);
    document.body.append(area);
    area.select();
    const ok = document.execCommand("copy");
    area.remove();
    return ok;
  }
}

/** A small button that copies `getText()` and confirms with a check mark. */
export function copyButton(getText, { label = "Copy", icon: iconNode, className = "" } = {}) {
  const button = h(
    "button",
    { class: `copy-button ${className}`.trim(), type: "button", title: label, "aria-label": label },
    iconNode ?? null,
  );
  button.addEventListener("click", async (event) => {
    event.stopPropagation();
    event.preventDefault();
    const ok = await copyText(typeof getText === "function" ? getText() : getText);
    button.classList.toggle("copied", ok);
    button.title = ok ? "Copied" : "Copy failed";
    setTimeout(() => {
      button.classList.remove("copied");
      button.title = label;
    }, 1400);
  });
  return button;
}

// Dialogs and popovers

const FOCUSABLE = "a[href], button, input, select, textarea, [tabindex]:not([tabindex='-1'])";

let openPopover = null;

export function closePopover() {
  openPopover?.close();
}

/**
 * Shows `content` anchored below `anchor`, or above it when it fits only
 * there. It stays inside the window: placed again whenever its size changes
 * (content that loads later) and shortened to the room it has, so a long one
 * scrolls. Clicking outside, Escape or a second call closes it. Returns a
 * handle with `close()`.
 */
export function popover(anchor, content, { align = "start", className = "", onClose } = {}) {
  closePopover();
  const panel = h("div", { class: `popover ${className}`.trim(), role: "dialog" }, content);
  document.body.append(panel);
  const margin = 8;
  const gap = 6;
  const place = () => {
    const rect = anchor.getBoundingClientRect();
    panel.style.maxHeight = "";
    const width = panel.offsetWidth;
    const height = panel.offsetHeight;
    let left = align === "end" ? rect.right - width : rect.left;
    left = Math.max(margin, Math.min(left, window.innerWidth - width - margin));
    const below = window.innerHeight - rect.bottom - gap - margin;
    const above = rect.top - gap - margin;
    // Below if it fits there, else above if it fits there, else the roomier side, shortened.
    const up = height > below && (height <= above || above > below);
    let room = up ? above : below;
    let top = up ? rect.top - gap - Math.min(height, room) : rect.bottom + gap;
    if (room < 160) {
      // Too little room either side (a very short window): use the whole height.
      room = window.innerHeight - 2 * margin;
      top = margin;
    }
    if (height > room) panel.style.maxHeight = `${room}px`;
    panel.style.left = `${left}px`;
    panel.style.top = `${Math.max(margin, top)}px`;
  };
  place();
  let frame = 0;
  const resized = new ResizeObserver(() => {
    cancelAnimationFrame(frame);
    frame = requestAnimationFrame(place);
  });
  resized.observe(panel);
  const onPointer = (event) => {
    if (!panel.contains(event.target) && !anchor.contains(event.target)) handle.close();
  };
  const onKey = (event) => {
    if (event.key === "Escape") {
      event.stopPropagation();
      handle.close();
      anchor.focus?.();
    }
  };
  // Tabbing out of the panel closes it, so focus order stays with the page.
  const onFocusOut = (event) => {
    const next = event.relatedTarget;
    if (next && !panel.contains(next) && !anchor.contains(next)) handle.close();
  };
  panel.addEventListener("focusout", onFocusOut);
  const handle = {
    panel,
    place,
    close() {
      if (openPopover !== handle) return;
      openPopover = null;
      resized.disconnect();
      cancelAnimationFrame(frame);
      panel.remove();
      document.removeEventListener("pointerdown", onPointer, true);
      document.removeEventListener("keydown", onKey, true);
      window.removeEventListener("resize", place);
      anchor.setAttribute?.("aria-expanded", "false");
      onClose?.();
    },
  };
  openPopover = handle;
  anchor.setAttribute?.("aria-expanded", "true");
  setTimeout(() => {
    document.addEventListener("pointerdown", onPointer, true);
    document.addEventListener("keydown", onKey, true);
    window.addEventListener("resize", place);
  });
  return handle;
}

/**
 * A modal dialog: traps Tab, closes on Escape or a backdrop click, and gives
 * focus back to what had it. `initialFocus` picks the element to focus.
 */
export function modal({ title, body, footer, className = "", onClose, initialFocus }) {
  const previous = document.activeElement;
  let closed = false;
  const close = (value) => {
    if (closed) return;
    closed = true;
    overlay.remove();
    document.removeEventListener("keydown", onKey, true);
    previous?.focus?.();
    onClose?.(value);
  };
  const onKey = (event) => {
    if (event.key === "Escape") {
      event.stopPropagation();
      close(null);
    } else if (event.key === "Tab") {
      const focusable = [...overlay.querySelectorAll(FOCUSABLE)].filter((node) => !node.disabled && node.offsetParent !== null);
      if (!focusable.length) return;
      const first = focusable[0];
      const last = focusable.at(-1);
      if (event.shiftKey && (document.activeElement === first || !overlay.contains(document.activeElement))) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && (document.activeElement === last || !overlay.contains(document.activeElement))) {
        event.preventDefault();
        first.focus();
      }
    }
  };
  const box = h(
    "div",
    { class: `modal ${className}`.trim(), role: "dialog", "aria-modal": "true", "aria-label": title },
    h("h2", { class: "modal-title" }, title),
    h("div", { class: "modal-body" }, body),
    footer ? h("div", { class: "modal-actions" }, footer) : null,
  );
  const overlay = h("div", { class: "modal-backdrop", onclick: (event) => event.target === overlay && close(null) }, box);
  document.body.append(overlay);
  document.addEventListener("keydown", onKey, true);
  const target = initialFocus?.(box) ?? box.querySelector(".modal-body input, .modal-body textarea, .modal-body select") ?? box.querySelector("button");
  target?.focus();
  return { close, box };
}

/** A modal with a title, body and buttons. Resolves to the clicked button's value. */
export function dialog({ title, body, buttons, className = "" }) {
  return new Promise((resolve) => {
    const actions = buttons.map((button) =>
      h("button", { class: `button ${button.kind ?? ""}`.trim(), type: "button", onclick: () => handle.close(button.value) }, button.label),
    );
    // The first field gets focus; otherwise Cancel for destructive dialogs, else the main action.
    const danger = actions.some((b) => b.classList.contains("danger"));
    const handle = modal({
      title,
      body,
      footer: actions,
      className,
      onClose: (value) => resolve(value ?? null),
      initialFocus: (box) =>
        box.querySelector(".modal-body input, .modal-body textarea, .modal-body select") ??
        (danger ? actions[0] : actions.find((b) => b.classList.contains("primary")) ?? actions[0]),
    });
  });
}

/** Asks for one line of text. Resolves to the trimmed text, or null. */
export function promptDialog({ title, label, value = "", confirm = "Save", placeholder = "" }) {
  return new Promise((resolve) => {
    const input = h("input", { class: "input", type: "text", value, placeholder, "aria-label": label, autocomplete: "off", spellcheck: "false" });
    const ok = h("button", { class: "button primary", type: "submit" }, confirm);
    const form = h(
      "form",
      {
        class: "prompt-form",
        onsubmit: (event) => {
          event.preventDefault();
          if (input.value.trim()) handle.close(input.value.trim());
        },
      },
      h("label", { class: "label" }, label),
      input,
    );
    const handle = modal({
      title,
      body: form,
      footer: [h("button", { class: "button", type: "button", onclick: () => handle.close(null) }, "Cancel"), ok],
      onClose: (result) => resolve(result ?? null),
    });
    ok.addEventListener("click", (event) => {
      event.preventDefault();
      form.requestSubmit();
    });
    input.select();
  });
}

export async function confirmDialog({ title, message, confirm = "Confirm", danger = false }) {
  const value = await dialog({
    title,
    body: h("p", {}, message),
    buttons: [
      { label: "Cancel", value: false },
      { label: confirm, value: true, kind: danger ? "danger" : "primary" },
    ],
  });
  return value === true;
}

// Settings: every key of the user config, the control service's
// control.toml, provider keys, memory and raw TOML editors.
//
// Structured edits start from the user file as read (not the merged
// configuration), so defaults are never written out; inherited values are
// shown as placeholders.

import { clone, confirmDialog, formatDuration, fullTime, h, local, plural, setChildren, shortPath, toast } from "./dom.js";
import { icon, pigLogo } from "./icons.js";
import { markdownNode } from "./markdown.js";
import { api, checkControl, markAllStale, refresh, resource, state } from "./store.js";
import { TAGLINE, controlSummary, updatePanel } from "./views.js";
import { modelPicker, toggle } from "./widgets.js";
import { remoteSection } from "./remote.js";

export const SECTIONS = [
  { id: "general", title: "General", icon: "settings", blurb: "Models, permission mode and limits for new sessions." },
  { id: "providers", title: "Providers", icon: "server", blurb: "Model providers, their endpoints and API keys." },
  { id: "models", title: "Models", icon: "cpu", blurb: "Per-model overrides for context size, output length and sampling." },
  { id: "permissions", title: "Permissions", icon: "shield", blurb: "Rules that allow or deny tool calls without asking." },
  { id: "mcp", title: "MCP servers", icon: "plug", blurb: "External tool servers over stdio, HTTP or WebSocket." },
  { id: "control", title: "Control service", icon: "monitor", blurb: "Browser and computer control through malac-control." },
  { id: "memory", title: "Memory", icon: "bookmark", blurb: "Durable facts agents remember across sessions." },
  { id: "agents", title: "Agents", icon: "agents", blurb: "Subagent limits, agent types and skills." },
  { id: "profiles", title: "Profiles", icon: "layers", blurb: "Named overlays selected with malac --profile <name>." },
  { id: "interface", title: "Interface", icon: "eye", blurb: "Terminal and web interface options." },
  { id: "remote", title: "Remote access", icon: "globe", blurb: "Other devices, pairing, tunnels and keeping the machine awake." },
  { id: "advanced", title: "Advanced", icon: "code", blurb: "Edit the configuration files as TOML." },
  { id: "about", title: "About", icon: "info", blurb: "Version, updates and where malac keeps its files." },
];

const MODE_OPTIONS = [
  { value: "ask", label: "Ask" },
  { value: "edits", label: "Edits" },
  { value: "autopilot", label: "Autopilot" },
];
const KIND_OPTIONS = [
  { value: "openai", label: "OpenAI compatible" },
  { value: "anthropic", label: "Anthropic" },
];
const PRESETS = [
  { label: "OpenAI", kind: "openai", base_url: "https://api.openai.com/v1", api_key_env: "OPENAI_API_KEY" },
  { label: "Anthropic", kind: "anthropic", base_url: "https://api.anthropic.com", api_key_env: "ANTHROPIC_API_KEY" },
  { label: "OpenRouter", kind: "openai", base_url: "https://openrouter.ai/api/v1", api_key_env: "OPENROUTER_API_KEY" },
  { label: "Ollama (local)", kind: "openai", base_url: "http://localhost:11434/v1" },
  { label: "LM Studio (local)", kind: "openai", base_url: "http://localhost:1234/v1" },
  { label: "Custom", kind: "openai", base_url: "" },
];
const BUILT_IN_PROVIDERS = new Set(["anthropic", "openai"]);
const CONTROL_DEFAULTS = {
  computer: { enabled: true, max_screenshot_width: 1366 },
  browser: { enabled: true, headless: false, profiles: {} },
};
const DEFAULTS = {
  max_steps: 200,
  agents: { max_depth: 2, max_concurrent: 4 },
  memory: { enabled: true, auto_recall: true, recall_limit: 5 },
  vector: { url: "http://localhost:19530", collection: "malac_memory" },
  model: { context_window: 200000, max_output_tokens: 16384 },
};
const NAME = /^[A-Za-z0-9_-]+$/;
const RULE = /^[\w*?.\-]+(\(.*\))?$/;
const TOP_SECTIONS = ["permissions", "control", "memory", "agents", "ui", "web", "update", "daemon", "remote"];

// Paths and values

const isObject = (value) => value != null && typeof value === "object" && !Array.isArray(value);

function getPath(obj, path) {
  let current = obj;
  for (const key of path) {
    if (!isObject(current)) return undefined;
    current = current[key];
  }
  return current;
}

function setPath(obj, path, value) {
  let current = obj;
  for (const key of path.slice(0, -1)) {
    if (!isObject(current[key])) current[key] = {};
    current = current[key];
  }
  current[path.at(-1)] = value;
}

function deletePath(obj, path) {
  const parent = getPath(obj, path.slice(0, -1));
  if (isObject(parent)) delete parent[path.at(-1)];
}

function stable(value) {
  if (Array.isArray(value)) return `[${value.map(stable).join(",")}]`;
  if (isObject(value)) {
    return `{${Object.keys(value)
      .sort()
      .map((k) => `${JSON.stringify(k)}:${stable(value[k])}`)
      .join(",")}}`;
  }
  return JSON.stringify(value ?? null);
}

/** Drops empty top-level sections so a reset key leaves no empty table. */
function tidy(doc) {
  const out = clone(doc ?? {});
  for (const key of TOP_SECTIONS) {
    if (isObject(out[key]) && Object.keys(out[key]).length === 0) delete out[key];
  }
  return out;
}

function display(value) {
  if (value == null || value === "") return "";
  if (typeof value === "boolean") return value ? "on" : "off";
  if (Array.isArray(value)) return value.join(", ");
  return String(value);
}

// TOML output for control.toml, which has no structured endpoint.

function tomlKey(key) {
  return /^[A-Za-z0-9_-]+$/.test(key) ? key : JSON.stringify(key);
}

function tomlValue(value) {
  if (typeof value === "string") return JSON.stringify(value);
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  if (Array.isArray(value)) return `[${value.map(tomlValue).join(", ")}]`;
  if (isObject(value)) {
    return `{ ${Object.entries(value)
      .map(([k, v]) => `${tomlKey(k)} = ${tomlValue(v)}`)
      .join(", ")} }`;
  }
  return '""';
}

export function toToml(doc) {
  const lines = [];
  const emit = (table, prefix) => {
    const scalars = [];
    const tables = [];
    for (const [key, value] of Object.entries(table)) {
      if (value == null) continue;
      (isObject(value) ? tables : scalars).push([key, value]);
    }
    if (prefix.length && (scalars.length || !tables.length)) {
      if (lines.length) lines.push("");
      lines.push(`[${prefix.map(tomlKey).join(".")}]`);
    }
    for (const [key, value] of scalars) lines.push(`${tomlKey(key)} = ${tomlValue(value)}`);
    for (const [key, value] of tables) emit(value, [...prefix, key]);
  };
  emit(doc ?? {}, []);
  return lines.length ? `${lines.join("\n")}\n` : "";
}

// Validation of the whole draft; returns path string -> message.

function checkInt(errors, doc, path, { min = 0, max = Number.MAX_SAFE_INTEGER, label = "Value" } = {}) {
  const value = getPath(doc, path);
  if (value === undefined) return;
  if (typeof value !== "number" || !Number.isInteger(value)) errors.set(path.join("."), `${label} must be a whole number`);
  else if (value < min) errors.set(path.join("."), `${label} must be at least ${min}`);
  else if (value > max) errors.set(path.join("."), `${label} must be at most ${max}`);
}

function checkFloat(errors, doc, path, { min, max, label = "Value" }) {
  const value = getPath(doc, path);
  if (value === undefined) return;
  if (typeof value !== "number" || Number.isNaN(value)) errors.set(path.join("."), `${label} must be a number`);
  else if (value < min || value > max) errors.set(path.join("."), `${label} must be between ${min} and ${max}`);
}

function checkUrl(errors, doc, path, schemes) {
  const value = getPath(doc, path);
  if (value === undefined || value === "") return;
  try {
    const url = new URL(value);
    if (!schemes.includes(url.protocol.replace(":", ""))) {
      errors.set(path.join("."), `Use ${schemes.map((s) => `${s}://`).join(" or ")}`);
    }
  } catch {
    errors.set(path.join("."), "Not a valid URL");
  }
}

function validateUser(doc) {
  const errors = new Map();
  checkInt(errors, doc, ["max_steps"], { min: 1, label: "Max steps" });
  checkInt(errors, doc, ["agents", "max_depth"], { min: 0, max: 16, label: "Max depth" });
  checkInt(errors, doc, ["agents", "max_concurrent"], { min: 1, max: 64, label: "Max concurrent" });
  checkInt(errors, doc, ["memory", "recall_limit"], { min: 1, max: 100, label: "Recall limit" });
  checkInt(errors, doc, ["web", "port"], { min: 0, max: 65535, label: "Port" });
  checkInt(errors, doc, ["daemon", "idle_minutes"], { min: 0, label: "Idle minutes" });
  if (isObject(doc.memory?.vector)) {
    checkUrl(errors, doc, ["memory", "vector", "url"], ["http", "https"]);
    if (doc.memory.vector.collection === "") errors.set("memory.vector.collection", "A collection name is required");
  }
  if (isObject(doc.memory?.embeddings)) {
    if (!doc.memory.embeddings.provider) errors.set("memory.embeddings.provider", "Choose a provider");
    if (!doc.memory.embeddings.model) errors.set("memory.embeddings.model", "Enter the embedding model id");
  }
  for (const [name, provider] of Object.entries(doc.providers ?? {})) {
    if (!NAME.test(name)) errors.set(`providers.${name}`, "Use letters, digits, - and _");
    if (!isObject(provider)) continue;
    if (!BUILT_IN_PROVIDERS.has(name) && !provider.base_url) errors.set(`providers.${name}.base_url`, "A base URL is required");
    checkUrl(errors, doc, ["providers", name, "base_url"], ["http", "https"]);
  }
  for (const [spec, model] of Object.entries(doc.models ?? {})) {
    if (!isObject(model)) continue;
    checkInt(errors, doc, ["models", spec, "context_window"], { min: 1, label: "Context window" });
    checkInt(errors, doc, ["models", spec, "max_output_tokens"], { min: 1, label: "Max output tokens" });
    checkInt(errors, doc, ["models", spec, "thinking_budget"], { min: 0, label: "Thinking budget" });
    checkFloat(errors, doc, ["models", spec, "temperature"], { min: 0, max: 2, label: "Temperature" });
  }
  for (const [name, server] of Object.entries(doc.mcp ?? {})) {
    if (!NAME.test(name)) errors.set(`mcp.${name}`, "Use letters, digits, - and _");
    if (!isObject(server)) continue;
    if (!server.command && !server.url) errors.set(`mcp.${name}.command`, "Set a command or a URL");
    if (server.command && server.url) errors.set(`mcp.${name}.url`, "Set either a command or a URL, not both");
    checkUrl(errors, doc, ["mcp", name, "url"], ["http", "https", "ws", "wss"]);
  }
  for (const list of ["allow", "deny"]) {
    (doc.permissions?.[list] ?? []).forEach((rule, index) => {
      if (!RULE.test(rule)) errors.set(`permissions.${list}.${index}`, `“${rule}” is not a rule like bash(cargo test*)`);
    });
  }
  for (const [name, profile] of Object.entries(doc.profiles ?? {})) {
    if (!NAME.test(name)) errors.set(`profiles.${name}`, "Use letters, digits, - and _");
    if (isObject(profile)) checkInt(errors, doc, ["profiles", name, "max_steps"], { min: 1, label: "Max steps" });
  }
  checkUrl(errors, doc, ["control", "url"], ["ws", "wss", "http", "https"]);
  return errors;
}

function validateControl(doc) {
  const errors = new Map();
  checkInt(errors, doc, ["computer", "max_screenshot_width"], { min: 320, label: "Width" });
  for (const [name, profile] of Object.entries(doc.browser?.profiles ?? {})) {
    if (!NAME.test(name)) errors.set(`browser.profiles.${name}`, "Use letters, digits, - and _");
    if (profile?.clone_from && profile?.cdp_url) errors.set(`browser.profiles.${name}.cdp_url`, "Use either clone_from or cdp_url");
    checkUrl(errors, doc, ["browser", "profiles", name, "cdp_url"], ["http", "https", "ws", "wss"]);
  }
  return errors;
}

// Controls

let uid = 0;
const nextId = () => `f${++uid}`;

function selectControl({ value, options, onChange, id, focusKey, disabled }) {
  const select = h(
    "select",
    { id, disabled, dataset: { focusKey } },
    options.map((o) => h("option", { value: o.value ?? "" }, o.label)),
  );
  select.value = value ?? "";
  select.addEventListener("change", () => onChange(select.value));
  return h("span", { class: "select" }, select, icon("chevronDown", 14, "select-arrow"));
}

function segmentedControl({ value, options, onChange, focusKey, label }) {
  return h(
    "div",
    { class: "segmented", role: "group", "aria-label": label },
    options.map((o) =>
      h(
        "button",
        {
          type: "button",
          "aria-pressed": String(value === o.value),
          dataset: { focusKey: `${focusKey}:${o.value}` },
          onclick: () => onChange(o.value),
        },
        h("span", { class: "segmented-label" }, o.label),
      ),
    ),
  );
}

function textInput({ value, placeholder, onInput, id, focusKey, type = "text", inputmode, autocomplete = "off" }) {
  const input = h("input", {
    class: "input",
    id,
    type,
    inputmode,
    value: value ?? "",
    placeholder: placeholder ?? "",
    autocomplete,
    spellcheck: "false",
    dataset: { focusKey },
  });
  input.addEventListener("input", () => onInput(input.value));
  return input;
}

// Settings page

export class SettingsPage {
  constructor({ onDirtyChange }) {
    this.onDirtyChange = onDirtyChange;
    this.section = "general";
    this.data = null;
    this.draft = {};
    this.control = {};
    this.errors = new Map();
    this.controlErrors = new Map();
    this.banner = null;
    this.saving = false;
    this.open = new Set();
    this.updaters = new Map();
    this.raw = { file: "user", texts: {}, loaded: {}, message: null };
    this.memory = { query: "", hits: null, scope: "all", expanded: new Set(), busy: false };
    this.nav = h("nav", { class: "settings-nav", "aria-label": "Settings sections" });
    this.head = h("header", { class: "settings-head" });
    this.bannerHost = h("div", { class: "settings-banner", "aria-live": "polite" });
    this.body = h("div", { class: "settings-body" });
    this.saveBar = h("div", { class: "save-bar", hidden: true });
    this.root = h(
      "div",
      { class: "settings" },
      this.nav,
      h("div", { class: "settings-main" }, this.head, this.bannerHost, this.body, this.saveBar),
    );
  }

  async load({ keepDraft = false } = {}) {
    const data = await refresh("settings");
    if (!data) {
      setChildren(this.body, h("p", { class: "empty-note error" }, resource("settings").error ?? "Could not load the settings"));
      return;
    }
    this.data = data;
    if (!keepDraft || !this.isDirty()) {
      this.draft = clone(data.user ?? {});
      this.control = clone(data.control ?? {});
    }
    this.validate();
    this.render();
  }

  show(section) {
    this.section = SECTIONS.some((s) => s.id === section) ? section : "general";
    this.banner = this.banner?.sticky ? this.banner : null;
    if (!this.data) {
      this.renderNav();
      setChildren(this.body, h("p", { class: "empty-note" }, h("span", { class: "spinner small" }), "Loading settings"));
      this.load();
      return;
    }
    this.render();
    this.root.querySelector(".settings-main")?.scrollTo?.(0, 0);
  }

  // Called when the server rebuilt its runtime (here or in another tab).
  reloaded() {
    if (this.saving) return;
    this.load({ keepDraft: true });
  }

  userDirty() {
    return this.data != null && stable(tidy(this.draft)) !== stable(tidy(this.data.user ?? {}));
  }

  controlDirty() {
    return this.data != null && stable(this.control) !== stable(this.data.control ?? {});
  }

  isDirty() {
    return this.userDirty() || this.controlDirty();
  }

  discard() {
    this.draft = clone(this.data?.user ?? {});
    this.control = clone(this.data?.control ?? {});
    this.banner = null;
    this.validate();
    this.render();
    this.onDirtyChange?.(false);
  }

  validate() {
    this.errors = validateUser(this.draft);
    this.controlErrors = validateControl(this.control);
    for (const [key, update] of this.updaters) update(this.errorFor(key));
  }

  errorFor(key) {
    return key.startsWith("control:") ? this.controlErrors.get(key.slice(8)) : this.errors.get(key);
  }

  changed({ rerender = false } = {}) {
    this.validate();
    if (rerender) this.render();
    else this.renderSaveBar();
    this.onDirtyChange?.(this.isDirty());
  }

  // Rendering keeps keyboard focus on the control that was focused.
  render() {
    const focusKey = document.activeElement?.dataset?.focusKey;
    const selection = document.activeElement?.selectionStart;
    this.updaters.clear();
    this.renderNav();
    const meta = SECTIONS.find((s) => s.id === this.section);
    setChildren(this.head, h("h1", { class: "page-title" }, meta.title), h("p", { class: "page-summary" }, meta.blurb));
    this.renderBanner();
    setChildren(this.body, this.sectionContent());
    this.renderSaveBar();
    if (focusKey) {
      const target = this.root.querySelector(`[data-focus-key="${CSS.escape(focusKey)}"]`);
      if (target) {
        target.focus();
        if (selection != null && typeof target.setSelectionRange === "function") {
          try {
            target.setSelectionRange(selection, selection);
          } catch {
            // Some input types do not support selection ranges.
          }
        }
      }
    }
  }

  renderNav() {
    const counts = this.errorCounts();
    const compact = selectControl({
      value: this.section,
      focusKey: "settings-nav",
      options: SECTIONS.map((s) => ({ value: s.id, label: counts[s.id] ? `${s.title} (${plural(counts[s.id], "problem")})` : s.title })),
      onChange: (id) => (location.hash = id === "general" ? "#settings" : `#settings=${id}`),
    });
    compact.querySelector("select").setAttribute("aria-label", "Settings section");
    setChildren(
      this.nav,
      compact,
      h(
        "ul",
        {},
        SECTIONS.map((s) =>
          h(
            "li",
            {},
            h(
              "a",
              {
                class: "settings-link",
                href: `#settings=${s.id}`,
                "aria-current": s.id === this.section ? "page" : null,
              },
              icon(s.icon, 16),
              h("span", {}, s.title),
              counts[s.id] ? h("span", { class: "nav-error", title: `${plural(counts[s.id], "problem")}` }, String(counts[s.id])) : null,
            ),
          ),
        ),
      ),
    );
  }

  errorCounts() {
    const counts = {};
    const bump = (id) => (counts[id] = (counts[id] ?? 0) + 1);
    for (const key of this.errors.keys()) {
      const head = key.split(".")[0];
      const map = { daemon: "interface", update: "about", max_steps: "general", agents: "agents", memory: "memory", web: "interface", providers: "providers", models: "models", mcp: "mcp", permissions: "permissions", profiles: "profiles", control: "control" };
      bump(map[head] ?? "general");
    }
    for (let i = 0; i < this.controlErrors.size; i += 1) bump("control");
    return counts;
  }

  renderBanner() {
    const banner = this.banner;
    if (!banner) {
      setChildren(this.bannerHost);
      return;
    }
    setChildren(
      this.bannerHost,
      h(
        "div",
        { class: `banner ${banner.kind}`, role: banner.kind === "error" ? "alert" : "status" },
        icon(banner.kind === "success" ? "check" : banner.kind === "error" ? "alert" : "info", 16),
        h("div", { class: "banner-text" }, h("strong", {}, banner.title), banner.message ? h("p", {}, banner.message) : null),
        banner.action ? h("button", { class: "button small", type: "button", onclick: banner.action.run }, banner.action.label) : null,
        h("button", { class: "icon-button small", type: "button", "aria-label": "Dismiss", onclick: () => ((this.banner = null), this.renderBanner()) }, icon("x", 14)),
      ),
    );
  }

  renderSaveBar() {
    const dirty = this.isDirty();
    const problems = this.errors.size + this.controlErrors.size;
    this.saveBar.hidden = !dirty;
    this.renderNav();
    if (!dirty) return;
    const files = [this.userDirty() ? "config.toml" : null, this.controlDirty() ? "control.toml" : null].filter(Boolean).join(" and ");
    setChildren(
      this.saveBar,
      h(
        "span",
        { class: "save-bar-text" },
        problems ? h("span", { class: "text-bad" }, `${plural(problems, "problem")} to fix`) : `Unsaved changes to ${files}`,
      ),
      h("button", { class: "button", type: "button", disabled: this.saving, onclick: () => this.discard() }, "Discard"),
      h(
        "button",
        { class: "button primary", type: "button", disabled: this.saving || problems > 0, onclick: () => this.save() },
        this.saving ? "Saving…" : "Save and apply",
      ),
    );
  }

  async save() {
    this.validate();
    if (this.errors.size || this.controlErrors.size) {
      this.renderSaveBar();
      return;
    }
    this.saving = true;
    this.renderSaveBar();
    let result = null;
    try {
      if (this.controlDirty()) {
        result = await api("/api/settings/raw/control", { method: "PUT", body: { text: toToml(this.control) } });
      }
      if (this.userDirty()) {
        result = await api("/api/settings", { method: "PUT", body: { user: tidy(this.draft) } });
      }
      this.showResult(result);
      markAllStale();
      this.saving = false;
      await this.load();
      refresh("models");
      refresh("tools");
    } catch (error) {
      this.saving = false;
      this.banner = { kind: "error", title: "Not saved", message: error.message };
      this.highlightServerError(error.message);
      this.render();
    }
    this.onDirtyChange?.(this.isDirty());
  }

  showResult(result) {
    if (!result) return;
    this.banner = result.applied
      ? { kind: "success", title: "Saved and applied", message: "New sessions use these settings." }
      : {
          kind: "warning",
          title: "Saved, not applied yet",
          message: result.message ?? "The running sessions keep the previous settings.",
          action: this.data?.reloadable === false ? null : { label: "Apply now", run: () => this.apply() },
          sticky: true,
        };
  }

  async apply() {
    try {
      const result = await api("/api/settings/apply", { method: "POST" });
      this.showResult(result);
      markAllStale();
    } catch (error) {
      this.banner = { kind: "error", title: "Could not apply", message: error.message };
    }
    this.renderBanner();
  }

  // Server messages name keys like `agents.max_depth`; point at that field.
  highlightServerError(message) {
    const match = /\bin `([^`]+)`\s*$/.exec(message) ?? /key `?([\w.-]+)/.exec(message);
    if (!match) return;
    const key = match[1].replace(/"/g, "");
    this.errors.set(key, message.split("\n").pop());
    const head = key.split(".")[0];
    const target = SECTIONS.find((s) => s.id === head || (head === "max_steps" && s.id === "general"));
    if (target) this.section = target.id;
  }

  // Field factory. `path` is relative to the user file unless `file` is
  // "control"; `base` supplies the inherited value shown as a placeholder.
  field(path, opts) {
    const file = opts.file ?? "user";
    const doc = file === "control" ? this.control : this.draft;
    const base = opts.base !== undefined ? opts.base : file === "control" ? getPath(CONTROL_DEFAULTS, path) : getPath(this.data?.effective, path);
    const key = path.join(".");
    const errorKey = file === "control" ? `control:${key}` : key;
    const focusKey = `${file}:${key}`;
    const id = nextId();
    const value = getPath(doc, path);
    const set = value !== undefined;
    const fromProject = file === "user" && getPath(this.data?.project, path) !== undefined;
    const update = (next, { rerender = false } = {}) => {
      if (next === undefined || next === "" || (Array.isArray(next) && next.length === 0 && opts.dropEmpty !== false)) deletePath(doc, path);
      else setPath(doc, path, next);
      this.changed({ rerender });
    };
    const error = h("p", { class: "field-error", id: `${id}-error`, role: "alert", hidden: true });
    this.updaters.set(errorKey, (message) => {
      error.textContent = message ?? "";
      error.hidden = !message;
      row.classList.toggle("invalid", Boolean(message));
    });
    let control;
    switch (opts.type) {
      case "toggle":
        control = toggle({
          checked: set ? Boolean(value) : Boolean(base ?? opts.default),
          id,
          onChange: (checked) => update(checked, { rerender: true }),
        });
        control.querySelector("input").dataset.focusKey = focusKey;
        break;
      case "select":
        control = selectControl({
          id,
          focusKey,
          value: set ? value : "",
          options: [{ value: "", label: opts.unsetLabel ?? `Default${base != null ? ` (${display(base)})` : ""}` }, ...opts.options],
          onChange: (next) => update(next === "" ? undefined : opts.parse ? opts.parse(next) : next, { rerender: true }),
        });
        break;
      case "segmented":
        control = segmentedControl({
          value: set ? value : base ?? opts.default,
          options: opts.options,
          focusKey,
          label: opts.label,
          onChange: (next) => update(next, { rerender: true }),
        });
        break;
      case "model": {
        const picker = modelPicker({
          value: set ? value : null,
          allowUnset: true,
          unsetLabel: opts.unsetLabel ?? (base ? `Default (${base})` : "Not set"),
          title: opts.label,
          onChange: (spec) => update(spec ?? undefined, { rerender: true }),
        });
        picker.el.id = id;
        picker.el.dataset.focusKey = focusKey;
        control = picker.el;
        break;
      }
      case "list":
        control = this.listEditor(doc, path, { focusKey, placeholder: opts.placeholder, examples: opts.examples, errorPrefix: errorKey });
        break;
      case "kv":
        control = this.kvEditor(doc, path, { focusKey, keyPlaceholder: opts.keyPlaceholder, valuePlaceholder: opts.valuePlaceholder });
        break;
      case "int":
      case "float":
        control = textInput({
          id,
          focusKey,
          inputmode: opts.type === "int" ? "numeric" : "decimal",
          value: set ? String(value) : "",
          placeholder: opts.placeholder ?? (base != null ? String(base) : opts.default != null ? String(opts.default) : ""),
          onInput: (text) => {
            const trimmed = text.trim();
            if (!trimmed) return update(undefined);
            const number = Number(trimmed);
            update(Number.isFinite(number) && trimmed !== "" ? number : trimmed);
          },
        });
        break;
      default:
        control = textInput({
          id,
          focusKey,
          type: opts.secret ? "password" : "text",
          value: set ? String(value) : "",
          placeholder: opts.placeholder ?? (base != null ? display(base) : ""),
          onInput: (text) => update(text === "" ? undefined : text),
        });
    }
    if (control instanceof HTMLInputElement || control.querySelector?.("input, select")) {
      const input = control instanceof HTMLInputElement ? control : control.querySelector("input, select");
      input.setAttribute("aria-describedby", `${id}-help ${id}-error`);
    }
    const canReset = set && opts.type !== "list" && opts.type !== "kv" && opts.reset !== false;
    const row = h(
      "div",
      { class: `field${opts.wide ? " wide" : ""}`, "data-path": key },
      h(
        "div",
        { class: "field-label" },
        opts.type === "segmented" || opts.type === "list" || opts.type === "kv"
          ? h("span", { class: "label", id: `${id}-label` }, opts.label)
          : h("label", { class: "label", for: id }, opts.label),
        opts.help ? h("p", { class: "field-help", id: `${id}-help` }, opts.help) : null,
        fromProject
          ? h("p", { class: "field-note" }, icon("info", 14), `The project file sets this to ${display(getPath(this.data.project, path))}, which takes precedence.`)
          : null,
      ),
      h(
        "div",
        { class: "field-control" },
        control,
        canReset
          ? h(
              "button",
              {
                class: "button ghost small",
                type: "button",
                title: "Remove this key and use the default",
                dataset: { focusKey: `${focusKey}:reset` },
                onclick: () => update(undefined, { rerender: true }),
              },
              "Reset",
            )
          : null,
        error,
      ),
    );
    this.updaters.get(errorKey)(this.errorFor(errorKey));
    return row;
  }

  listEditor(doc, path, { focusKey, placeholder, examples = [], errorPrefix, inherited = [] }) {
    const items = [...(getPath(doc, path) ?? inherited)];
    const commit = (next, rerender = true) => {
      if (next.length) setPath(doc, path, next);
      else deletePath(doc, path);
      this.changed({ rerender });
    };
    const adder = textInput({ focusKey: `${focusKey}:new`, placeholder: placeholder ?? "Add an item", onInput: () => {} });
    const add = () => {
      const value = adder.value.trim();
      if (!value) return;
      commit([...items, value]);
      this.root.querySelector(`[data-focus-key="${CSS.escape(`${focusKey}:new`)}"]`)?.focus();
    };
    adder.addEventListener("keydown", (event) => {
      if (event.key === "Enter") {
        event.preventDefault();
        add();
      }
    });
    const unused = examples.filter((example) => !items.includes(example));
    return h(
      "div",
      { class: "list-editor" },
      items.length
        ? h(
            "ul",
            { class: "list-items" },
            items.map((item, index) => {
              const input = textInput({
                value: item,
                focusKey: `${focusKey}:${index}`,
                onInput: (text) => {
                  items[index] = text;
                  commit([...items], false);
                },
              });
              const error = this.errorFor(`${errorPrefix}.${index}`);
              return h(
                "li",
                { class: error ? "invalid" : "" },
                input,
                h(
                  "button",
                  { class: "icon-button small", type: "button", "aria-label": `Remove ${item}`, onclick: () => commit(items.filter((_, i) => i !== index)) },
                  icon("x", 14),
                ),
                error ? h("p", { class: "field-error" }, error) : null,
              );
            }),
          )
        : null,
      h("div", { class: "list-add" }, adder, h("button", { class: "button small", type: "button", onclick: add }, "Add")),
      unused.length
        ? h(
            "div",
            { class: "examples" },
            h("span", { class: "muted small" }, "Examples"),
            unused.map((example) => h("button", { class: "chip", type: "button", onclick: () => commit([...items, example]) }, example)),
          )
        : null,
    );
  }

  kvEditor(doc, path, { focusKey, keyPlaceholder = "Name", valuePlaceholder = "Value" }) {
    const entries = Object.entries(getPath(doc, path) ?? {});
    const commit = (pairs, rerender = true) => {
      const object = {};
      for (const [k, v] of pairs) if (k) object[k] = v;
      if (Object.keys(object).length) setPath(doc, path, object);
      else deletePath(doc, path);
      this.changed({ rerender });
    };
    const rows = entries.map(([k, v], index) =>
      h(
        "div",
        { class: "kv-row" },
        textInput({
          value: k,
          placeholder: keyPlaceholder,
          focusKey: `${focusKey}:k${index}`,
          onInput: (text) => {
            entries[index] = [text, entries[index][1]];
            commit(entries, false);
          },
        }),
        textInput({
          value: v,
          placeholder: valuePlaceholder,
          focusKey: `${focusKey}:v${index}`,
          onInput: (text) => {
            entries[index] = [entries[index][0], text];
            commit(entries, false);
          },
        }),
        h("button", { class: "icon-button small", type: "button", "aria-label": `Remove ${k}`, onclick: () => commit(entries.filter((_, i) => i !== index)) }, icon("x", 14)),
      ),
    );
    const newKey = textInput({ placeholder: keyPlaceholder, focusKey: `${focusKey}:newk`, onInput: () => {} });
    const newValue = textInput({ placeholder: valuePlaceholder, focusKey: `${focusKey}:newv`, onInput: () => {} });
    const add = () => {
      const k = newKey.value.trim();
      if (!k) return newKey.focus();
      commit([...entries, [k, newValue.value]]);
      this.root.querySelector(`[data-focus-key="${CSS.escape(`${focusKey}:newk`)}"]`)?.focus();
    };
    for (const input of [newKey, newValue]) {
      input.addEventListener("keydown", (event) => {
        if (event.key === "Enter") {
          event.preventDefault();
          add();
        }
      });
    }
    return h(
      "div",
      { class: "kv-editor" },
      rows,
      h("div", { class: "kv-row add" }, newKey, newValue, h("button", { class: "button small", type: "button", onclick: add }, "Add")),
    );
  }

  group(title, children, { description, actions } = {}) {
    return h(
      "section",
      { class: "settings-group" },
      title || actions
        ? h(
            "header",
            { class: "group-head" },
            h("div", {}, title ? h("h2", { class: "group-title" }, title) : null, description ? h("p", { class: "group-blurb" }, description) : null),
            actions ?? null,
          )
        : null,
      h("div", { class: "group-body" }, children),
    );
  }

  sectionContent() {
    if (!this.data) return h("p", { class: "empty-note" }, "Loading settings");
    switch (this.section) {
      case "general":
        return this.general();
      case "providers":
        return this.providers();
      case "models":
        return this.models();
      case "permissions":
        return this.permissions();
      case "mcp":
        return this.mcp();
      case "control":
        return this.controlSection();
      case "memory":
        return this.memorySection();
      case "agents":
        return this.agents();
      case "profiles":
        return this.profiles();
      case "interface":
        return this.interfaceSection();
      case "remote":
        return remoteSection(this);
      case "advanced":
        return this.advanced();
      case "about":
        return this.about();
      default:
        return null;
    }
  }

  fileNote() {
    return h("p", { class: "file-note" }, "Saved to ", h("span", { class: "path", title: this.data.paths.user }, shortPath(this.data.paths.user, 2)));
  }

  // General

  general() {
    return [
      this.fileNote(),
      this.group("Models", [
        this.field(["model"], { type: "model", label: "Main model", help: "Used for new sessions unless you pick another.", unsetLabel: "Automatic" }),
        this.field(["small_model"], { type: "model", label: "Small model", help: "Cheaper model for compaction summaries.", unsetLabel: "Same as main model" }),
        this.field(["vision_model"], { type: "model", label: "Vision model", help: "Describes images for models that cannot see them.", unsetLabel: "None" }),
      ]),
      this.group("Behaviour", [
        this.field(["mode"], {
          type: "segmented",
          label: "Permission mode",
          help: "Ask confirms edits and commands, edits only confirms commands, autopilot never asks. Deny rules apply in every mode.",
          options: MODE_OPTIONS,
          default: "ask",
        }),
        this.field(["max_steps"], { type: "int", label: "Max steps", help: "Tool-call rounds per prompt before the agent is stopped.", placeholder: String(DEFAULTS.max_steps) }),
        this.field(["trust_project_config"], {
          type: "toggle",
          label: "Trust project config",
          help: "Lets a repository's .malac/config.toml set providers, MCP servers, permissions and the mode. Only enable this for repositories you trust.",
          default: false,
        }),
      ]),
    ];
  }

  // Providers

  providerNames() {
    return [...new Set([...Object.keys(this.data.effective.providers ?? {}), ...Object.keys(this.draft.providers ?? {})])].sort();
  }

  providers() {
    const names = this.providerNames();
    return [
      this.fileNote(),
      this.addProviderForm(),
      h("div", { class: "item-list" }, names.map((name) => this.providerCard(name))),
    ];
  }

  addProviderForm() {
    const open = this.open.has("add-provider");
    if (!open) {
      return h(
        "div",
        { class: "list-actions" },
        h("button", { class: "button", type: "button", dataset: { focusKey: "add-provider" }, onclick: () => (this.open.add("add-provider"), this.render()) }, icon("plus", 14), "Add provider"),
      );
    }
    const name = textInput({ placeholder: "e.g. openrouter", focusKey: "add-provider:name", onInput: () => check() });
    let preset = PRESETS[0];
    const url = textInput({ value: preset.base_url, placeholder: "https://…/v1", focusKey: "add-provider:url", onInput: () => check() });
    const message = h("p", { class: "field-error", hidden: true });
    const addButton = h("button", { class: "button primary", type: "button" }, "Add provider");
    const presetSelect = selectControl({
      value: preset.label,
      focusKey: "add-provider:preset",
      options: PRESETS.map((p) => ({ value: p.label, label: p.label })),
      onChange: (label) => {
        preset = PRESETS.find((p) => p.label === label) ?? PRESETS.at(-1);
        url.value = preset.base_url;
        if (!name.value) name.value = label.toLowerCase().split(" ")[0].replace(/[^a-z0-9_-]/g, "");
        check();
      },
    });
    const check = () => {
      const value = name.value.trim();
      let problem = "";
      if (!value) problem = "Name the provider";
      else if (!NAME.test(value)) problem = "Use letters, digits, - and _";
      else if (this.providerNames().includes(value)) problem = `A provider named ${value} already exists`;
      else if (!url.value.trim()) problem = "Enter the base URL";
      message.textContent = problem;
      message.hidden = !problem || !value;
      addButton.disabled = Boolean(problem);
      return !problem;
    };
    addButton.addEventListener("click", () => {
      if (!check()) return;
      const value = name.value.trim();
      const entry = { kind: preset.kind, base_url: url.value.trim() };
      if (preset.api_key_env) entry.api_key_env = preset.api_key_env;
      setPath(this.draft, ["providers", value], entry);
      this.open.delete("add-provider");
      this.open.add(`provider:${value}`);
      this.changed({ rerender: true });
    });
    check();
    setTimeout(() => name.focus());
    return this.group(
      "New provider",
      [
        h("div", { class: "inline-form" }, h("label", {}, h("span", { class: "label" }, "Preset"), presetSelect), h("label", {}, h("span", { class: "label" }, "Name"), name), h("label", { class: "grow" }, h("span", { class: "label" }, "Base URL"), url)),
        message,
        h("div", { class: "form-actions" }, h("button", { class: "button", type: "button", onclick: () => (this.open.delete("add-provider"), this.render()) }, "Cancel"), addButton),
      ],
      { description: "The provider is added to your user config; save to keep it." },
    );
  }

  providerStatus(name) {
    const models = resource("models").data?.providers?.find((p) => p.provider === name);
    const hasKey = this.data.keys?.[name] ?? models?.has_key ?? false;
    if (!models) return { hasKey, text: resource("models").loading ? "Checking" : "Not checked", kind: "muted" };
    if (models.error === "no API key") return { hasKey, text: "No key", kind: "warn" };
    if (models.error) return { hasKey, text: "Unreachable", kind: "bad", title: models.error };
    return { hasKey, text: `Responding, ${plural(models.available.length, "model")}`, kind: "ok", models };
  }

  providerCard(name) {
    const inUser = this.draft.providers?.[name] !== undefined;
    const inProject = this.data.project?.providers?.[name] !== undefined;
    const builtIn = BUILT_IN_PROVIDERS.has(name);
    const effective = this.data.effective.providers?.[name] ?? {};
    const draft = this.draft.providers?.[name] ?? {};
    const kind = draft.kind ?? effective.kind ?? "openai";
    const status = this.providerStatus(name);
    const open = this.open.has(`provider:${name}`);
    const toggleOpen = () => {
      if (open) this.open.delete(`provider:${name}`);
      else this.open.add(`provider:${name}`);
      this.render();
    };
    const errorCount = [...this.errors.keys()].filter((k) => k === `providers.${name}` || k.startsWith(`providers.${name}.`)).length;
    return h(
      "article",
      { class: `item${open ? " open" : ""}${errorCount ? " invalid" : ""}` },
      h(
        "button",
        { class: "item-head", type: "button", "aria-expanded": String(open), dataset: { focusKey: `provider:${name}` }, onclick: toggleOpen },
        icon(open ? "chevronDown" : "chevronRight", 14, "item-chevron"),
        h("span", { class: "item-title" }, name),
        h("span", { class: "pill" }, kind === "anthropic" ? "Anthropic" : "OpenAI compatible"),
        builtIn ? h("span", { class: "pill" }, "built-in") : null,
        inProject ? h("span", { class: "pill" }, "project") : null,
        h("span", { class: "item-spacer" }),
        status.hasKey || status.text === "No key"
          ? h("span", { class: `status ${status.kind}`, title: status.title ?? "" }, h("span", { class: `dot ${status.kind === "ok" ? "ok" : status.kind === "bad" ? "failed" : status.kind === "warn" ? "waiting" : "idle"}` }), status.text)
          : h("span", { class: "status warn" }, h("span", { class: "dot waiting" }), "No key"),
      ),
      open
        ? h(
            "div",
            { class: "item-body" },
            this.field(["providers", name, "kind"], { type: "segmented", label: "API", options: KIND_OPTIONS, default: "openai", help: "Chat completions (OpenAI and compatible servers) or the Anthropic messages API." }),
            this.field(["providers", name, "base_url"], { label: "Base URL", placeholder: effective.base_url || "https://…/v1" }),
            this.field(["providers", name, "api_key_env"], { label: "API key variable", help: "Environment variable read first. A stored key is used when it is unset.", placeholder: effective.api_key_env || "e.g. OPENAI_API_KEY" }),
            this.keyField(name, status),
            this.field(["providers", name, "headers"], { type: "kv", label: "Extra headers", help: "Sent with every request. ${VAR} is expanded from the environment.", keyPlaceholder: "Header", valuePlaceholder: "Value" }),
            this.modelsField(name, status),
            !inUser && !builtIn
              ? h("p", { class: "field-note" }, icon("info", 14), "Defined in the project file. Changes here are saved to your user file and merge over it.")
              : null,
            h(
              "div",
              { class: "item-actions" },
              inUser
                ? h(
                    "button",
                    {
                      class: "button danger",
                      type: "button",
                      onclick: async () => {
                        const ok = await confirmDialog({
                          title: builtIn ? `Reset ${name}?` : `Remove ${name}?`,
                          message: builtIn
                            ? "Your overrides are removed and the built-in settings apply again."
                            : "The provider is removed from your user config when you save. Its stored key stays until you remove it.",
                          confirm: builtIn ? "Reset" : "Remove",
                          danger: true,
                        });
                        if (!ok) return;
                        delete this.draft.providers[name];
                        if (!Object.keys(this.draft.providers).length) delete this.draft.providers;
                        this.changed({ rerender: true });
                      },
                    },
                    builtIn ? "Reset to built-in" : "Remove provider",
                  )
                : null,
            ),
          )
        : null,
    );
  }

  keyField(name, status) {
    const id = nextId();
    const input = h("input", {
      class: "input",
      id,
      type: "password",
      name: `${name}-api-key`,
      placeholder: status.hasKey ? "Paste a new key to replace it" : "Paste the API key",
      autocomplete: "new-password",
      spellcheck: "false",
      dataset: { focusKey: `key:${name}` },
    });
    const username = h("input", { type: "text", name: "username", value: name, autocomplete: "username", hidden: true, readonly: true });
    const saveKey = async () => {
      const key = input.value.trim();
      if (!key) return input.focus();
      try {
        await api(`/api/providers/${encodeURIComponent(name)}/key`, { method: "PUT", body: { api_key: key } });
        input.value = "";
        toast(`Stored the key for ${name}`, "success");
        await this.refreshKeys();
      } catch (error) {
        toast(error.message, "error");
      }
    };
    const removeKey = async () => {
      const ok = await confirmDialog({
        title: `Remove the stored key for ${name}?`,
        message: "The key is deleted from credentials.toml. A key in the environment variable keeps working.",
        confirm: "Remove key",
        danger: true,
      });
      if (!ok) return;
      try {
        await api(`/api/providers/${encodeURIComponent(name)}/key`, { method: "DELETE" });
        toast(`Removed the stored key for ${name}`, "success");
        await this.refreshKeys();
      } catch (error) {
        toast(error.message, "error");
      }
    };
    return h(
      "div",
      { class: "field" },
      h(
        "div",
        { class: "field-label" },
        h("label", { class: "label", for: id }, "API key"),
        h(
          "p",
          { class: "field-help" },
          status.hasKey ? "A key is available. " : "",
          "Keys are stored in ",
          h("span", { class: "path", title: this.data.paths.credentials }, "credentials.toml"),
          " (mode 0600) and are never sent back to the browser.",
        ),
      ),
      h(
        "div",
        { class: "field-control" },
        h(
          "form",
          {
            class: "input-row",
            onsubmit: (event) => {
              event.preventDefault();
              saveKey();
            },
          },
          username,
          input,
          h("button", { class: "button", type: "submit" }, "Store key"),
        ),
        status.hasKey ? h("button", { class: "button ghost small", type: "button", onclick: removeKey }, "Remove stored key") : null,
      ),
    );
  }

  async refreshKeys() {
    const data = await refresh("settings");
    if (data) this.data = { ...this.data, keys: data.keys };
    await refresh("models");
    this.render();
  }

  modelsField(name, status) {
    const path = ["providers", name, "models"];
    const listed = getPath(this.draft, path) ?? getPath(this.data.effective, path) ?? [];
    const available = status.models?.available ?? [];
    const fetchOpen = this.open.has(`fetch:${name}`);
    const setList = (next) => {
      if (next.length) setPath(this.draft, path, next);
      else deletePath(this.draft, path);
      this.changed({ rerender: true });
    };
    return h(
      "div",
      { class: "field wide" },
      h(
        "div",
        { class: "field-label" },
        h("span", { class: "label" }, "Models"),
        h("p", { class: "field-help" }, "Model ids offered in the pickers. Any other id still works when typed as provider/model."),
      ),
      h(
        "div",
        { class: "field-control" },
        this.listEditor(this.draft, path, { focusKey: `user:${path.join(".")}`, placeholder: "Model id, e.g. gpt-5", errorPrefix: path.join("."), inherited: listed }),
        h(
          "div",
          { class: "row-actions" },
          h(
            "button",
            {
              class: "button small",
              type: "button",
              dataset: { focusKey: `fetch:${name}` },
              onclick: async () => {
                this.open.add(`fetch:${name}`);
                this.render();
                await refresh("models");
                this.render();
              },
            },
            icon("refresh", 14),
            "Fetch from provider",
          ),
        ),
        fetchOpen
          ? h(
              "div",
              { class: "fetched" },
              resource("models").loading
                ? h("p", { class: "muted small" }, h("span", { class: "spinner small" }), "Asking the provider")
                : status.models
                  ? available.length
                    ? [
                        h("p", { class: "muted small" }, `${plural(available.length, "model")} available. Select to add or remove.`),
                        h(
                          "div",
                          { class: "chips" },
                          available.map((model) => {
                            const chosen = (getPath(this.draft, path) ?? listed).includes(model);
                            return h(
                              "button",
                              {
                                class: "chip",
                                type: "button",
                                "aria-pressed": String(chosen),
                                onclick: () => {
                                  const current = getPath(this.draft, path) ?? listed;
                                  setList(chosen ? current.filter((m) => m !== model) : [...current, model]);
                                },
                              },
                              chosen ? icon("check", 12) : null,
                              model,
                            );
                          }),
                        ),
                      ]
                    : h("p", { class: "muted small" }, "The provider listed no models.")
                  : h("p", { class: "field-error" }, status.title ?? (status.text === "No key" ? "Store a key first, then fetch again." : "The provider did not answer. Save the provider first if it is new.")),
            )
          : null,
      ),
    );
  }

  // Models

  models() {
    const specs = [...new Set([...Object.keys(this.data.effective.models ?? {}), ...Object.keys(this.draft.models ?? {})])].sort();
    const adding = this.open.has("add-model");
    const picker = modelPicker({
      value: null,
      placeholder: "Choose a model to override",
      onChange: (spec) => {
        if (!spec) return;
        if (!getPath(this.draft, ["models", spec])) setPath(this.draft, ["models", spec], {});
        this.open.delete("add-model");
        this.open.add(`model:${spec}`);
        this.changed({ rerender: true });
      },
    });
    return [
      this.fileNote(),
      h(
        "div",
        { class: "list-actions" },
        adding
          ? h("div", { class: "input-row" }, picker.el, h("button", { class: "button", type: "button", onclick: () => (this.open.delete("add-model"), this.render()) }, "Cancel"))
          : h("button", { class: "button", type: "button", dataset: { focusKey: "add-model" }, onclick: () => (this.open.add("add-model"), this.render()) }, icon("plus", 14), "Add override"),
      ),
      specs.length
        ? h(
            "div",
            { class: "item-list" },
            specs.map((spec) => {
              const open = this.open.has(`model:${spec}`);
              const own = this.draft.models?.[spec];
              const values = { ...(this.data.effective.models?.[spec] ?? {}), ...(own ?? {}) };
              const summary = Object.entries(values)
                .filter(([, v]) => v != null)
                .map(([k, v]) => `${k.replace(/_/g, " ")} ${v}`)
                .join(" · ");
              return h(
                "article",
                { class: `item${open ? " open" : ""}` },
                h(
                  "button",
                  {
                    class: "item-head",
                    type: "button",
                    "aria-expanded": String(open),
                    dataset: { focusKey: `model:${spec}` },
                    onclick: () => (open ? this.open.delete(`model:${spec}`) : this.open.add(`model:${spec}`), this.render()),
                  },
                  icon(open ? "chevronDown" : "chevronRight", 14, "item-chevron"),
                  h("span", { class: "item-title" }, spec),
                  own === undefined ? h("span", { class: "pill" }, "project") : null,
                  h("span", { class: "item-spacer" }),
                  h("span", { class: "muted small ellipsis", title: summary }, summary || "No overrides"),
                ),
                open
                  ? h(
                      "div",
                      { class: "item-body" },
                      this.field(["models", spec, "context_window"], { type: "int", label: "Context window", help: "Tokens; compaction starts at 80% of this.", placeholder: String(DEFAULTS.model.context_window) }),
                      this.field(["models", spec, "max_output_tokens"], { type: "int", label: "Max output tokens", placeholder: String(DEFAULTS.model.max_output_tokens) }),
                      this.field(["models", spec, "vision"], {
                        type: "select",
                        label: "Vision",
                        help: "Without vision, images are described by the vision model first.",
                        unsetLabel: "Assume it can see images",
                        options: [
                          { value: "true", label: "Sees images" },
                          { value: "false", label: "Text only" },
                        ],
                        parse: (v) => v === "true",
                      }),
                      this.field(["models", spec, "thinking_budget"], { type: "int", label: "Thinking budget", help: "Anthropic extended thinking, in tokens.", placeholder: "Off" }),
                      this.field(["models", spec, "temperature"], { type: "float", label: "Temperature", help: "Between 0 and 2.", placeholder: "Provider default" }),
                      own !== undefined
                        ? h(
                            "div",
                            { class: "item-actions" },
                            h(
                              "button",
                              {
                                class: "button danger",
                                type: "button",
                                onclick: () => {
                                  delete this.draft.models[spec];
                                  if (!Object.keys(this.draft.models).length) delete this.draft.models;
                                  this.changed({ rerender: true });
                                },
                              },
                              "Remove override",
                            ),
                          )
                        : null,
                    )
                  : null,
              );
            }),
          )
        : h("p", { class: "empty-note" }, "No overrides. Models use a 200k context window and 16k output tokens by default."),
    ];
  }

  // Permissions

  permissions() {
    return [
      this.fileNote(),
      h(
        "div",
        { class: "explainer" },
        h("p", {}, "A rule is a tool name, optionally followed by a pattern in parentheses that is matched against the tool's main argument. ", h("code", {}, "*"), " matches anything."),
        h(
          "ul",
          {},
          h("li", {}, h("code", {}, "bash(cargo test*)"), " matches each segment of a shell command"),
          h("li", {}, h("code", {}, "edit(src/*)"), " matches the path for ", h("code", {}, "write"), " and ", h("code", {}, "edit")),
          h("li", {}, h("code", {}, "web_fetch(*.rust-lang.org)"), " matches the host"),
          h("li", {}, h("code", {}, "browser_*"), " matches every browser tool"),
        ),
        h("p", {}, "Deny rules are checked first and apply in every mode, autopilot included."),
      ),
      this.group("Deny", [
        this.field(["permissions", "deny"], {
          type: "list",
          label: "Always deny",
          help: "Calls matching these never run.",
          placeholder: "e.g. bash(git push --force*)",
          examples: ["bash(rm -rf /*)", "bash(git push --force*)", "bash(sudo *)"],
        }),
      ]),
      this.group("Allow", [
        this.field(["permissions", "allow"], {
          type: "list",
          label: "Allow without asking",
          help: "Calls matching these run without a prompt in ask and edits mode.",
          placeholder: "e.g. bash(cargo test*)",
          examples: ["bash(cargo test*)", "bash(git status*)", "bash(git diff*)", "browser_*", "web_fetch(*.rust-lang.org)"],
        }),
      ]),
    ];
  }

  // MCP servers

  mcp() {
    const names = [...new Set([...Object.keys(this.data.effective.mcp ?? {}), ...Object.keys(this.draft.mcp ?? {})])].sort();
    const tools = resource("tools").data;
    return [
      this.fileNote(),
      this.addServerForm(),
      names.length
        ? h("div", { class: "item-list" }, names.map((name) => this.serverCard(name, tools)))
        : h("p", { class: "empty-note" }, "No MCP servers yet. Add one to give agents more tools."),
    ];
  }

  addServerForm() {
    if (!this.open.has("add-server")) {
      return h(
        "div",
        { class: "list-actions" },
        h("button", { class: "button", type: "button", dataset: { focusKey: "add-server" }, onclick: () => (this.open.add("add-server"), this.render()) }, icon("plus", 14), "Add server"),
      );
    }
    let transport = "command";
    const name = textInput({ placeholder: "e.g. github", focusKey: "add-server:name", onInput: () => check() });
    const target = textInput({ placeholder: "npx", focusKey: "add-server:target", onInput: () => check() });
    const message = h("p", { class: "field-error", hidden: true });
    const addButton = h("button", { class: "button primary", type: "button" }, "Add server");
    const targetLabel = h("span", { class: "label" }, "Command");
    const transportControl = segmentedControl({
      value: transport,
      focusKey: "add-server:transport",
      label: "Transport",
      options: [
        { value: "command", label: "Command" },
        { value: "url", label: "URL" },
      ],
      onChange: (next) => {
        transport = next;
        for (const button of transportControl.querySelectorAll("button")) {
          button.setAttribute("aria-pressed", String(button.dataset.focusKey.endsWith(`:${next}`)));
        }
        targetLabel.textContent = next === "url" ? "URL" : "Command";
        target.placeholder = next === "url" ? "https://example.com/mcp" : "npx";
        check();
      },
    });
    const check = () => {
      const value = name.value.trim();
      let problem = "";
      if (!value) problem = "Name the server";
      else if (!NAME.test(value)) problem = "Use letters, digits, - and _";
      else if (this.draft.mcp?.[value] || this.data.effective.mcp?.[value]) problem = `A server named ${value} already exists`;
      else if (!target.value.trim()) problem = transport === "url" ? "Enter the URL" : "Enter the command";
      message.textContent = problem;
      message.hidden = !problem || !value;
      addButton.disabled = Boolean(problem);
      return !problem;
    };
    addButton.addEventListener("click", () => {
      if (!check()) return;
      const value = name.value.trim();
      const entry = transport === "url" ? { url: target.value.trim() } : { command: target.value.trim() };
      setPath(this.draft, ["mcp", value], entry);
      this.open.delete("add-server");
      this.open.add(`mcp:${value}`);
      this.changed({ rerender: true });
    });
    check();
    setTimeout(() => name.focus());
    return this.group(
      "New MCP server",
      [
        h(
          "div",
          { class: "inline-form" },
          h("label", {}, h("span", { class: "label" }, "Name"), name),
          h("div", { class: "form-item" }, h("span", { class: "label" }, "Transport"), transportControl),
          h("label", { class: "grow" }, targetLabel, target),
        ),
        message,
        h("div", { class: "form-actions" }, h("button", { class: "button", type: "button", onclick: () => (this.open.delete("add-server"), this.render()) }, "Cancel"), addButton),
      ],
      { description: "Arguments, environment and headers can be set after adding." },
    );
  }

  serverCard(name, tools) {
    const own = this.draft.mcp?.[name];
    const effective = this.data.effective.mcp?.[name] ?? {};
    const merged = { enabled: true, prefix: true, ...effective, ...(own ?? {}) };
    const transport = merged.url ? "url" : "command";
    const live = tools?.servers?.find((s) => s.name === name);
    const toolNames = (tools?.tools ?? []).filter((t) => t.name.startsWith(`${name}__`)).map((t) => t.name);
    const open = this.open.has(`mcp:${name}`);
    const status = !merged.enabled
      ? { kind: "muted", dot: "idle", text: "Disabled" }
      : live
        ? live.error
          ? { kind: "bad", dot: "failed", text: "Unavailable", title: live.error }
          : { kind: "ok", dot: "ok", text: `Connected, ${plural(live.tools, "tool")}` }
        : { kind: "muted", dot: "idle", text: "Not connected" };
    const errorCount = [...this.errors.keys()].filter((k) => k.startsWith(`mcp.${name}`)).length;
    const setTransport = (next) => {
      if (next === transport) return;
      const entry = { ...(own ?? {}) };
      if (next === "url") {
        delete entry.command;
        delete entry.args;
        delete entry.env;
        entry.url = "";
      } else {
        delete entry.url;
        delete entry.headers;
        delete entry.token_env;
        entry.command = "";
      }
      setPath(this.draft, ["mcp", name], entry);
      this.changed({ rerender: true });
    };
    return h(
      "article",
      { class: `item${open ? " open" : ""}${errorCount ? " invalid" : ""}` },
      h(
        "button",
        {
          class: "item-head",
          type: "button",
          "aria-expanded": String(open),
          dataset: { focusKey: `mcp:${name}` },
          onclick: () => (open ? this.open.delete(`mcp:${name}`) : this.open.add(`mcp:${name}`), this.render()),
        },
        icon(open ? "chevronDown" : "chevronRight", 14, "item-chevron"),
        h("span", { class: "item-title" }, name),
        h("span", { class: "pill" }, transport === "url" ? (String(merged.url).startsWith("ws") ? "WebSocket" : "HTTP") : "stdio"),
        own === undefined ? h("span", { class: "pill" }, "project") : null,
        h("span", { class: "item-spacer" }),
        h("span", { class: `status ${status.kind}`, title: status.title ?? "" }, h("span", { class: `dot ${status.dot}` }), status.text),
      ),
      open
        ? h(
            "div",
            { class: "item-body" },
            name === "control" ? h("p", { class: "field-note" }, icon("info", 14), "A server named control replaces the built-in control service.") : null,
            live?.error ? h("p", { class: "field-error block" }, live.error) : null,
            this.field(["mcp", name, "enabled"], { type: "toggle", label: "Enabled", default: true, base: effective.enabled ?? true }),
            h(
              "div",
              { class: "field" },
              h("div", { class: "field-label" }, h("span", { class: "label" }, "Transport"), h("p", { class: "field-help" }, "Spawn a local process over stdio, or connect to http(s):// or ws(s)://.")),
              h(
                "div",
                { class: "field-control" },
                segmentedControl({
                  value: transport,
                  focusKey: `mcp:${name}:transport`,
                  label: "Transport",
                  options: [
                    { value: "command", label: "Command" },
                    { value: "url", label: "URL" },
                  ],
                  onChange: setTransport,
                }),
              ),
            ),
            transport === "command"
              ? [
                  this.field(["mcp", name, "command"], { label: "Command", base: effective.command, placeholder: "e.g. npx" }),
                  this.field(["mcp", name, "args"], { type: "list", label: "Arguments", placeholder: "Add an argument" }),
                  this.field(["mcp", name, "env"], { type: "kv", label: "Environment", help: "${VAR} is expanded from malac's environment.", keyPlaceholder: "Variable" }),
                ]
              : [
                  this.field(["mcp", name, "url"], { label: "URL", base: effective.url, placeholder: "https://example.com/mcp" }),
                  this.field(["mcp", name, "headers"], { type: "kv", label: "Headers", help: "For example Authorization = Bearer ${GITHUB_TOKEN}.", keyPlaceholder: "Header" }),
                  this.field(["mcp", name, "token_env"], { label: "Token variable", help: "Environment variable sent as a bearer token.", placeholder: "e.g. GITHUB_TOKEN" }),
                ],
            this.field(["mcp", name, "prefix"], { type: "toggle", label: "Prefix tool names", help: `Expose tools as ${name}__tool to avoid clashes.`, default: true, base: effective.prefix ?? true }),
            toolNames.length
              ? h(
                  "div",
                  { class: "field wide" },
                  h("div", { class: "field-label" }, h("span", { class: "label" }, "Tools"), h("p", { class: "field-help" }, "Reported by the running server.")),
                  h("div", { class: "field-control" }, h("div", { class: "chips static" }, toolNames.map((tool) => h("span", { class: "chip" }, tool.slice(name.length + 2))))),
                )
              : null,
            own !== undefined
              ? h(
                  "div",
                  { class: "item-actions" },
                  h(
                    "button",
                    {
                      class: "button danger",
                      type: "button",
                      onclick: async () => {
                        const ok = await confirmDialog({ title: `Remove ${name}?`, message: "The server is removed from your user config when you save.", confirm: "Remove", danger: true });
                        if (!ok) return;
                        delete this.draft.mcp[name];
                        if (!Object.keys(this.draft.mcp).length) delete this.draft.mcp;
                        this.changed({ rerender: true });
                      },
                    },
                    "Remove server",
                  ),
                )
              : null,
          )
        : null,
    );
  }

  // Control service

  controlSection() {
    const tools = resource("tools").data;
    const control = state.control;
    const summary = controlSummary(control);
    const controlTools = (tools?.tools ?? []).filter((t) => /^(browser|computer)_/.test(t.name));
    const profiles = Object.entries(this.control.browser?.profiles ?? {});
    return [
      this.group(
        "Status",
        [
          h(
            "div",
            { class: "card-row" },
            h("span", {}, "Control service"),
            h("span", { class: `status ${summary.kind}` }, h("span", { class: `dot ${summary.dot}` }), summary.text),
          ),
          control?.error ? h("p", { class: "field-note" }, icon("info", 14), control.error) : null,
          this.permissionsBlock(control),
          controlTools.length ? h("div", { class: "chips static" }, controlTools.map((t) => h("span", { class: "chip", title: t.description }, t.name))) : null,
        ],
        { description: "malac-control drives a browser and the desktop for agents. It runs next to malac or as a separate service." },
      ),
      this.group(
        "Connection",
        [
          this.field(["control", "enabled"], { type: "toggle", label: "Enabled", default: true, help: "Connect the control service when a session starts." }),
          this.field(["control", "command"], { label: "Command", help: "Path to the malac-control binary.", placeholder: "malac-control next to malac, else on PATH" }),
          this.field(["control", "url"], { label: "URL", help: "Connect to a running service instead of spawning one.", placeholder: "e.g. ws://127.0.0.1:7878" }),
          this.field(["control", "token_env"], { label: "Token variable", help: "Environment variable with that service's token.", placeholder: "e.g. MALAC_CONTROL_TOKEN" }),
        ],
        { description: "Saved to config.toml." },
      ),
      this.group(
        "Computer",
        [
          this.field(["computer", "enabled"], { file: "control", type: "toggle", label: "Computer control", help: "Screenshots, mouse and keyboard on this machine.", default: true }),
          this.field(["computer", "max_screenshot_width"], { file: "control", type: "int", label: "Max screenshot width", help: "Wider screenshots are scaled down. At least 320.", placeholder: "1366" }),
        ],
        { description: "Saved to control.toml. Comments in that file are not kept; edit it in Advanced to keep them." },
      ),
      this.group(
        "Browser",
        [
          this.field(["browser", "enabled"], { file: "control", type: "toggle", label: "Browser control", default: true }),
          this.field(["browser", "executable"], { file: "control", label: "Browser executable", help: "Chrome, Chromium, Brave or Edge.", placeholder: "Detected automatically" }),
          this.field(["browser", "headless"], { file: "control", type: "toggle", label: "Headless", help: "Run browsers without a window.", default: false }),
        ],
      ),
      this.group(
        "Browser profiles",
        [
          h("p", { class: "field-help" }, "A managed profile keeps its own cookies and logins. The default profile always exists."),
          profiles.length ? h("div", { class: "item-list" }, profiles.map(([name, profile]) => this.profileRow(name, profile))) : h("p", { class: "empty-note" }, "Only the default managed profile."),
          this.addBrowserProfile(),
        ],
      ),
    ];
  }

  // macOS grants Screen Recording and Accessibility to the app that runs
  // malac-control; requesting them opens the system prompts.
  permissionsBlock(control) {
    if (!control?.connected) return null;
    const p = control.permissions;
    const state3 = (value) =>
      value === true
        ? h("span", { class: "status ok" }, h("span", { class: "dot ok" }), "Granted")
        : value === false
          ? h("span", { class: "status warn" }, h("span", { class: "dot waiting" }), "Not granted")
          : h("span", { class: "status muted" }, "Not needed here");
    const check = async (request) => {
      if (request) {
        const ok = await confirmDialog({
          title: "Request permissions?",
          message: "macOS shows its Screen Recording and Accessibility prompts and opens System Settings at the missing ones.",
          confirm: "Request permissions",
        });
        if (!ok) return;
      }
      try {
        await checkControl(request);
        this.render();
      } catch (error) {
        toast(error.message, "error");
      }
    };
    const missing = p && (p.screen_recording === false || p.accessibility === false);
    return h(
      "div",
      { class: "permissions-block" },
      p
        ? h(
            "dl",
            { class: "kv" },
            h("dt", {}, "Platform"),
            h("dd", {}, { macos: "macOS", linux: "Linux", windows: "Windows" }[p.platform] ?? p.platform ?? "Unknown"),
            h("dt", {}, "Screen Recording"),
            h("dd", {}, state3(p.screen_recording)),
            h("dt", {}, "Accessibility"),
            h("dd", {}, state3(p.accessibility)),
            p.app ? [h("dt", {}, "Granted to"), h("dd", { class: "path", title: p.app.path }, p.app.name ?? p.app.path)] : null,
          )
        : h("p", { class: "muted small" }, "The control service did not report its permissions."),
      p?.hint ? h("p", { class: "field-note" }, icon("info", 14), p.hint) : null,
      h(
        "div",
        { class: "row-actions" },
        h("button", { class: "button small", type: "button", onclick: () => check(false) }, icon("refresh", 14), "Check again"),
        missing ? h("button", { class: "button small primary", type: "button", onclick: () => check(true) }, "Request permissions") : null,
      ),
    );
  }

  profileRow(name, profile) {
    const type = profile?.clone_from != null ? "clone" : profile?.cdp_url != null ? "cdp" : "managed";
    const setType = (next) => {
      const entry = { ...(profile ?? {}) };
      delete entry.clone_from;
      delete entry.cdp_url;
      if (next === "clone") entry.clone_from = "";
      if (next === "cdp") entry.cdp_url = "http://127.0.0.1:9222";
      setPath(this.control, ["browser", "profiles", name], entry);
      this.changed({ rerender: true });
    };
    const extra = Object.keys(profile ?? {}).filter((k) => k !== "clone_from" && k !== "cdp_url");
    return h(
      "article",
      { class: "item open" },
      h(
        "div",
        { class: "item-head static" },
        h("span", { class: "item-title" }, name),
        h("span", { class: "item-spacer" }),
        h(
          "button",
          {
            class: "icon-button small",
            type: "button",
            "aria-label": `Remove profile ${name}`,
            onclick: () => {
              delete this.control.browser.profiles[name];
              this.changed({ rerender: true });
            },
          },
          icon("trash", 14),
        ),
      ),
      h(
        "div",
        { class: "item-body" },
        h(
          "div",
          { class: "field" },
          h("div", { class: "field-label" }, h("span", { class: "label" }, "Source")),
          h(
            "div",
            { class: "field-control" },
            segmentedControl({
              value: type,
              focusKey: `profile:${name}:type`,
              label: "Profile source",
              options: [
                { value: "managed", label: "Managed" },
                { value: "clone", label: "Clone system profile" },
                { value: "cdp", label: "Attach to browser" },
              ],
              onChange: setType,
            }),
          ),
        ),
        type === "clone"
          ? this.field(["browser", "profiles", name, "clone_from"], { file: "control", label: "System profile", help: "Directory name, display name or path, copied on first launch.", placeholder: "e.g. Profile 1", reset: false })
          : null,
        type === "cdp"
          ? this.field(["browser", "profiles", name, "cdp_url"], { file: "control", label: "DevTools URL", help: "Endpoint of a browser started with --remote-debugging-port.", placeholder: "http://127.0.0.1:9222", reset: false })
          : null,
        extra.length ? h("p", { class: "field-note" }, icon("info", 14), `Also sets ${extra.join(", ")}; edit those in Advanced.`) : null,
      ),
    );
  }

  addBrowserProfile() {
    const input = textInput({ placeholder: "Profile name, e.g. work", focusKey: "add-profile", onInput: () => check() });
    const message = h("p", { class: "field-error", hidden: true });
    const button = h("button", { class: "button small", type: "button" }, "Add profile");
    const check = () => {
      const value = input.value.trim();
      const problem = !value ? "" : !NAME.test(value) ? "Use letters, digits, - and _" : this.control.browser?.profiles?.[value] ? "That profile exists" : "";
      message.textContent = problem;
      message.hidden = !problem;
      button.disabled = !value || Boolean(problem);
      return value && !problem;
    };
    const add = () => {
      if (!check()) return;
      setPath(this.control, ["browser", "profiles", input.value.trim()], {});
      this.changed({ rerender: true });
    };
    button.addEventListener("click", add);
    input.addEventListener("keydown", (event) => {
      if (event.key === "Enter") {
        event.preventDefault();
        add();
      }
    });
    check();
    return h("div", {}, h("div", { class: "input-row" }, input, button), message);
  }

  // Memory

  memorySection() {
    const memoryConfig = this.draft.memory ?? {};
    const hasVector = isObject(memoryConfig.vector);
    const hasEmbeddings = isObject(memoryConfig.embeddings);
    const providerOptions = this.providerNames()
      .filter((name) => (this.draft.providers?.[name]?.kind ?? this.data.effective.providers?.[name]?.kind ?? "openai") === "openai")
      .map((name) => ({ value: name, label: name }));
    const switchRow = (label, help, on, onChange, focusKey) =>
      h(
        "div",
        { class: "field" },
        h("div", { class: "field-label" }, h("span", { class: "label" }, label), h("p", { class: "field-help" }, help)),
        h("div", { class: "field-control" }, (() => {
          const control = toggle({ checked: on, onChange });
          control.querySelector("input").dataset.focusKey = focusKey;
          control.querySelector("input").setAttribute("aria-label", label);
          return control;
        })()),
      );
    return [
      this.fileNote(),
      this.group("Recall", [
        this.field(["memory", "enabled"], { type: "toggle", label: "Memory", help: "Memory tools and the index in the system prompt.", default: true }),
        this.field(["memory", "auto_recall"], { type: "toggle", label: "Auto recall", help: "Search memories with each prompt and attach the best matches.", default: true }),
        this.field(["memory", "recall_limit"], { type: "int", label: "Recall limit", help: "Matches considered per prompt.", placeholder: String(DEFAULTS.memory.recall_limit) }),
      ]),
      this.group("Vector store", [
        switchRow(
          "Use a vector store",
          "Milvus or Zilliz Cloud for semantic search. Without one, search matches keywords.",
          hasVector,
          (on) => {
            if (on) setPath(this.draft, ["memory", "vector"], {});
            else deletePath(this.draft, ["memory", "vector"]);
            this.changed({ rerender: true });
          },
          "vector-toggle",
        ),
        hasVector
          ? [
              this.field(["memory", "vector", "url"], { label: "URL", base: null, placeholder: DEFAULTS.vector.url }),
              this.field(["memory", "vector", "token_env"], { label: "Token variable", base: null, help: "Holds user:password or an API key.", placeholder: "e.g. MILVUS_TOKEN" }),
              this.field(["memory", "vector", "database"], { label: "Database", base: null, placeholder: "default" }),
              this.field(["memory", "vector", "collection"], { label: "Collection", base: null, placeholder: DEFAULTS.vector.collection }),
            ]
          : null,
      ]),
      this.group("Embeddings", [
        switchRow(
          "Custom embeddings",
          "An OpenAI-compatible provider with an /embeddings endpoint.",
          hasEmbeddings,
          (on) => {
            if (on) setPath(this.draft, ["memory", "embeddings"], { provider: providerOptions[0]?.value ?? "", model: "" });
            else deletePath(this.draft, ["memory", "embeddings"]);
            this.changed({ rerender: true });
          },
          "embeddings-toggle",
        ),
        hasEmbeddings
          ? [
              this.field(["memory", "embeddings", "provider"], { type: "select", label: "Provider", options: providerOptions, unsetLabel: "Choose a provider", base: null, reset: false }),
              this.field(["memory", "embeddings", "model"], { label: "Model", base: null, placeholder: "e.g. text-embedding-3-small" }),
            ]
          : null,
      ]),
      this.memoryBrowser(),
    ];
  }

  memoryBrowser() {
    const memory = resource("memory");
    if (!memory.data && !memory.loading && !memory.error) refresh("memory").then(() => this.section === "memory" && this.render());
    const m = this.memory;
    const entries = memory.data?.entries ?? [];
    const rows = m.hits
      ? m.hits.map((hit) => ({ ...hit.entry, score: hit.score }))
      : entries.filter((e) => m.scope === "all" || e.scope === m.scope);
    const search = textInput({ value: m.query, placeholder: "Search memories", focusKey: "memory-search", type: "search", onInput: (text) => (m.query = text) });
    const runSearch = async () => {
      if (!m.query.trim()) {
        m.hits = null;
        this.render();
        return;
      }
      m.busy = true;
      this.render();
      try {
        const result = await api("/api/memory/search", { method: "POST", body: { query: m.query.trim() } });
        m.hits = result.hits ?? [];
      } catch (error) {
        toast(error.message, "error");
      }
      m.busy = false;
      this.render();
    };
    search.addEventListener("keydown", (event) => {
      if (event.key === "Enter") {
        event.preventDefault();
        runSearch();
      }
    });
    const reindex = async () => {
      m.busy = true;
      this.render();
      try {
        const result = await api("/api/memory/reindex", { method: "POST" });
        toast(`Indexed ${plural(result.indexed ?? 0, "memory", "memories")}`, "success");
      } catch (error) {
        toast(error.message, "error");
      }
      m.busy = false;
      this.render();
    };
    const remove = async (entry) => {
      const ok = await confirmDialog({ title: `Forget ${entry.name}?`, message: `The memory file ${entry.path} is deleted.`, confirm: "Forget", danger: true });
      if (!ok) return;
      try {
        await api(`/api/memory/${encodeURIComponent(entry.scope)}/${encodeURIComponent(entry.name)}`, { method: "DELETE" });
        m.hits = m.hits?.filter((hit) => hit.entry.name !== entry.name || hit.entry.scope !== entry.scope) ?? null;
        await refresh("memory");
        this.render();
      } catch (error) {
        toast(error.message, "error");
      }
    };
    return this.group(
      "Saved memories",
      [
        h(
          "div",
          { class: "toolbar" },
          h("div", { class: "input-row grow" }, search, h("button", { class: "button", type: "button", disabled: m.busy, onclick: runSearch }, "Search")),
          m.hits
            ? h("button", { class: "button ghost", type: "button", onclick: () => ((m.hits = null), (m.query = ""), this.render()) }, "Clear")
            : segmentedControl({
                value: m.scope,
                focusKey: "memory-scope",
                label: "Scope",
                options: [
                  { value: "all", label: "All" },
                  { value: "project", label: "Project" },
                  { value: "global", label: "Global" },
                ],
                onChange: (scope) => ((m.scope = scope), this.render()),
              }),
        ),
        memory.loading && !entries.length
          ? h("p", { class: "empty-note" }, h("span", { class: "spinner small" }), "Loading memories")
          : rows.length
            ? h(
                "table",
                { class: "table memory-table" },
                h(
                  "thead",
                  {},
                  h("tr", {}, h("th", {}, "Name"), h("th", { class: "hide-narrow" }, "Description"), h("th", {}, "Scope"), m.hits ? h("th", { class: "num" }, "Score") : null, h("th", { class: "col-actions" }, h("span", { class: "sr-only" }, "Actions"))),
                ),
                h(
                  "tbody",
                  {},
                  rows.flatMap((entry) => {
                    const key = `${entry.scope}/${entry.name}`;
                    const expanded = m.expanded.has(key);
                    return [
                      h(
                        "tr",
                        { class: expanded ? "selected" : "" },
                        h(
                          "td",
                          { class: "cell-name" },
                          h(
                            "button",
                            {
                              class: "row-link",
                              type: "button",
                              "aria-expanded": String(expanded),
                              title: entry.name,
                              onclick: () => (expanded ? m.expanded.delete(key) : m.expanded.add(key), this.render()),
                            },
                            entry.name,
                          ),
                        ),
                        h("td", { class: "hide-narrow muted cell-main", title: entry.description }, entry.description),
                        h("td", {}, entry.scope),
                        m.hits ? h("td", { class: "num" }, entry.score.toFixed(2)) : null,
                        h(
                          "td",
                          { class: "col-actions" },
                          h("button", { class: "icon-button small", type: "button", "aria-label": `Forget ${entry.name}`, title: "Forget", onclick: () => remove(entry) }, icon("trash", 14)),
                        ),
                      ),
                      expanded
                        ? h(
                            "tr",
                            { class: "detail-row" },
                            h(
                              "td",
                              { colspan: m.hits ? 5 : 4 },
                              h("p", { class: "muted small" }, entry.description),
                              markdownNode(entry.body, "small"),
                              h("p", { class: "path small" }, entry.path),
                            ),
                          )
                        : null,
                    ];
                  }),
                ),
              )
            : h("p", { class: "empty-note" }, m.hits ? "No memories match." : "Nothing remembered yet."),
        memory.error ? h("p", { class: "field-error block" }, memory.error) : null,
      ],
      {
        description: memory.data ? `${plural(entries.length, "memory", "memories")}. Vector search is ${memory.data.vector ? "on" : "off"}.` : null,
        actions: h(
          "button",
          { class: "button small", type: "button", disabled: m.busy || !memory.data?.vector, title: memory.data?.vector ? "Rebuild the vector index from the memory files" : "Needs a vector store", onclick: reindex },
          icon("refresh", 14),
          "Reindex",
        ),
      },
    );
  }

  // Agents

  agents() {
    const types = this.data.agent_types ?? [];
    const skills = this.data.skills ?? [];
    const instructions = this.data.instructions ?? [];
    return [
      this.fileNote(),
      this.group("Subagents", [
        this.field(["agents", "max_depth"], { type: "int", label: "Max depth", help: "How deep subagents may nest.", placeholder: String(DEFAULTS.agents.max_depth) }),
        this.field(["agents", "max_concurrent"], { type: "int", label: "Max concurrent", help: "Subagents running at the same time.", placeholder: String(DEFAULTS.agents.max_concurrent) }),
      ]),
      this.group(
        "Agent types",
        types.length
          ? h(
              "table",
              { class: "table" },
              h("thead", {}, h("tr", {}, h("th", {}, "Name"), h("th", {}, "Description"), h("th", { class: "hide-narrow" }, "Model"), h("th", { class: "hide-narrow" }, "Tools"))),
              h(
                "tbody",
                {},
                types.map((t) =>
                  h(
                    "tr",
                    {},
                    h("td", { class: "strong" }, t.name),
                    h("td", { class: "ellipsis-cell", title: t.description }, t.description),
                    h("td", { class: "hide-narrow" }, t.model ?? "Inherited"),
                    h("td", { class: "hide-narrow ellipsis-cell", title: (t.tools ?? []).join(", ") }, t.tools?.length ? t.tools.join(", ") : "All"),
                  ),
                ),
              ),
            )
          : h("p", { class: "empty-note" }, "No agent types. Add markdown files with name and description frontmatter to ~/.malac/agents/ or .malac/agents/."),
        { description: "Read from ~/.malac/agents/*.md and <project>/.malac/agents/*.md." },
      ),
      this.group(
        "Skills",
        skills.length
          ? h(
              "table",
              { class: "table" },
              h("thead", {}, h("tr", {}, h("th", {}, "Name"), h("th", {}, "Description"), h("th", { class: "hide-narrow" }, "File"))),
              h(
                "tbody",
                {},
                skills.map((s) =>
                  h(
                    "tr",
                    {},
                    h("td", { class: "strong" }, s.name),
                    h("td", { class: "ellipsis-cell", title: s.description }, s.description),
                    h("td", { class: "hide-narrow path ellipsis-cell", title: s.path }, s.path),
                  ),
                ),
              ),
            )
          : h("p", { class: "empty-note" }, "No skills found."),
        { description: "SKILL.md files in ~/.malac/skills/, ~/.claude/skills/ and the project's .malac/skills/ or .claude/skills/." },
      ),
      this.group(
        "Instruction files",
        instructions.length
          ? h("ul", { class: "path-list" }, instructions.map((path) => h("li", { class: "path", title: path }, path)))
          : h("p", { class: "empty-note" }, "No AGENTS.md, MALAC.md or CLAUDE.md found."),
        { description: "Added to every agent's system prompt." },
      ),
    ];
  }

  // Profiles

  profiles() {
    const names = Object.keys(this.draft.profiles ?? {}).sort();
    const input = textInput({ placeholder: "Profile name, e.g. fast", focusKey: "add-config-profile", onInput: () => check() });
    const message = h("p", { class: "field-error", hidden: true });
    const button = h("button", { class: "button", type: "button" }, icon("plus", 14), "Add profile");
    const check = () => {
      const value = input.value.trim();
      const problem = !value ? "" : !NAME.test(value) ? "Use letters, digits, - and _" : names.includes(value) ? "That profile exists" : "";
      message.textContent = problem;
      message.hidden = !problem;
      button.disabled = !value || Boolean(problem);
      return value && !problem;
    };
    const add = () => {
      if (!check()) return;
      setPath(this.draft, ["profiles", input.value.trim()], {});
      this.open.add(`profile:${input.value.trim()}`);
      this.changed({ rerender: true });
    };
    button.addEventListener("click", add);
    input.addEventListener("keydown", (event) => {
      if (event.key === "Enter") {
        event.preventDefault();
        add();
      }
    });
    check();
    return [
      this.fileNote(),
      h("p", { class: "explainer" }, "A profile overrides any keys when malac starts with ", h("code", {}, "--profile <name>"), ". The common keys are below; other keys are kept and can be edited in Advanced."),
      h("div", { class: "list-actions" }, h("div", { class: "input-row" }, input, button), message),
      names.length
        ? h(
            "div",
            { class: "item-list" },
            names.map((name) => {
              const open = this.open.has(`profile:${name}`);
              const profile = this.draft.profiles[name] ?? {};
              const known = ["model", "small_model", "vision_model", "mode", "max_steps"];
              const other = Object.keys(profile).filter((k) => !known.includes(k));
              const summary = Object.entries(profile)
                .filter(([k]) => known.includes(k))
                .map(([k, v]) => `${k} ${display(v)}`)
                .join(" · ");
              return h(
                "article",
                { class: `item${open ? " open" : ""}` },
                h(
                  "button",
                  {
                    class: "item-head",
                    type: "button",
                    "aria-expanded": String(open),
                    dataset: { focusKey: `profile:${name}` },
                    onclick: () => (open ? this.open.delete(`profile:${name}`) : this.open.add(`profile:${name}`), this.render()),
                  },
                  icon(open ? "chevronDown" : "chevronRight", 14, "item-chevron"),
                  h("span", { class: "item-title" }, name),
                  h("span", { class: "item-spacer" }),
                  h("span", { class: "muted small ellipsis", title: summary }, summary || "Empty"),
                ),
                open
                  ? h(
                      "div",
                      { class: "item-body" },
                      this.field(["profiles", name, "model"], { type: "model", label: "Main model", base: null, unsetLabel: "Not changed" }),
                      this.field(["profiles", name, "small_model"], { type: "model", label: "Small model", base: null, unsetLabel: "Not changed" }),
                      this.field(["profiles", name, "vision_model"], { type: "model", label: "Vision model", base: null, unsetLabel: "Not changed" }),
                      this.field(["profiles", name, "mode"], { type: "select", label: "Permission mode", base: null, unsetLabel: "Not changed", options: MODE_OPTIONS }),
                      this.field(["profiles", name, "max_steps"], { type: "int", label: "Max steps", base: null, placeholder: "Not changed" }),
                      other.length ? h("p", { class: "field-note" }, icon("info", 14), `Also sets ${other.join(", ")}.`) : null,
                      h(
                        "div",
                        { class: "item-actions" },
                        h(
                          "button",
                          {
                            class: "button danger",
                            type: "button",
                            onclick: () => {
                              delete this.draft.profiles[name];
                              if (!Object.keys(this.draft.profiles).length) delete this.draft.profiles;
                              this.changed({ rerender: true });
                            },
                          },
                          "Remove profile",
                        ),
                      ),
                    )
                  : null,
              );
            }),
          )
        : h("p", { class: "empty-note" }, "No profiles."),
    ];
  }

  // Interface

  interfaceSection() {
    const theme = local.get("malac.theme") ?? "system";
    return [
      this.group(
        "This browser",
        [
          h(
            "div",
            { class: "field" },
            h("div", { class: "field-label" }, h("span", { class: "label" }, "Theme"), h("p", { class: "field-help" }, "Remembered in this browser only.")),
            h(
              "div",
              { class: "field-control" },
              segmentedControl({
                value: theme,
                focusKey: "theme",
                label: "Theme",
                options: [
                  { value: "system", label: "System" },
                  { value: "light", label: "Light" },
                  { value: "dark", label: "Dark" },
                ],
                onChange: (value) => {
                  document.dispatchEvent(new CustomEvent("malac:theme", { detail: value }));
                  this.render();
                },
              }),
            ),
          ),
        ],
      ),
      this.fileNote(),
      this.group("Terminal", [
        this.field(["ui", "show_thinking"], { type: "toggle", label: "Show thinking", help: "Stream model reasoning in the terminal, like -v.", default: false }),
        this.field(["ui", "tui"], { type: "toggle", label: "Full-screen terminal UI", help: "Use the full-screen interface in a terminal. Off keeps the line-based REPL.", default: true }),
      ]),
      this.group(
        "Daemon",
        [
          this.field(["daemon", "idle_minutes"], {
            type: "int",
            label: "Stop when idle",
            help: "Minutes without clients, running agents or jobs before the daemon exits. 0 keeps it running.",
            placeholder: "120",
          }),
          this.field(["daemon", "browse_root"], {
            label: "Folder picker root",
            help: "New tabs can only be opened below this folder.",
            placeholder: "The home directory",
          }),
        ],
        { description: "Applies the next time the daemon starts." },
      ),
      this.group("Web", [
        h(
          "p",
          { class: "field-note" },
          icon("info", 14),
          state.web.remote ? "This daemon accepts paired devices from other machines." : "This daemon only accepts this machine and SSH tunnels.",
          " Its address, port and remote access are under ",
          h("a", { href: "#settings=remote" }, "Remote access"),
          ".",
        ),
      ]),
    ];
  }

  // About and updates

  about() {
    return [
      h(
        "div",
        { class: "about large" },
        pigLogo(40),
        h(
          "div",
          {},
          h("p", { class: "about-name" }, "malac", state.version ? h("span", { class: "muted tabular" }, ` v${state.version}`) : null),
          h("p", { class: "muted" }, TAGLINE),
        ),
      ),
      this.daemonGroup(),
      this.group("Software update", [updatePanel().el], {
        description: "Updating never interrupts running agents. The daemon keeps the version it started with until it is restarted.",
      }),
      this.fileNote(),
      this.group("Update settings", [
        this.field(["update", "check"], { type: "toggle", label: "Check for updates", help: "Look for a newer release in the background, at most once a day.", default: true }),
        this.field(["update", "auto"], { type: "toggle", label: "Install automatically", help: "Install new releases in the background. Running sessions keep their version until restarted.", default: false }),
      ]),
    ];
  }

  // The daemon serving this page: from the snapshot, plus GET /api/daemon.
  daemonGroup() {
    const d = state.daemon;
    if (!d) return null;
    const info = resource("daemon");
    if (!info.loading && (!info.at || Date.now() - info.at > 5000)) refresh("daemon").then(() => this.section === "about" && this.render());
    const i = info.data;
    const uptime = d.started ? formatDuration(Date.now() - Date.parse(d.started)) : "–";
    const stop = async () => {
      const busy = i && (i.agents_running || i.jobs_running);
      const ok = await confirmDialog({
        title: "Stop the daemon?",
        message: busy
          ? `${plural(i.agents_running, "agent")} and ${plural(i.jobs_running, "job")} are running and will be stopped. Workspaces and transcripts are kept; malac starts the daemon again when needed.`
          : "Workspaces and transcripts are kept; malac starts the daemon again when needed. This page reconnects once it runs again.",
        confirm: "Stop daemon",
        danger: true,
      });
      if (!ok) return;
      try {
        await api("/api/daemon/stop", { method: "POST", body: { force: Boolean(busy) } });
      } catch (error) {
        toast(error.message, "error");
      }
    };
    return this.group(
      "Daemon",
      [
        h(
          "dl",
          { class: "kv" },
          h("dt", {}, "Version"),
          h("dd", { class: "tabular" }, d.version, d.version_mismatch ? h("span", { class: "muted" }, " (differs from this client)") : null),
          h("dt", {}, "Process"),
          h("dd", { class: "tabular" }, `pid ${d.pid}`),
          h("dt", {}, "Running for"),
          h("dd", { class: "tabular", title: fullTime(d.started) }, uptime),
          h("dt", {}, "Clients"),
          h("dd", { class: "tabular" }, `${plural(d.clients ?? i?.clients, "client")} connected`),
          h("dt", {}, "Listening"),
          h("dd", {}, i?.url ?? location.origin, d.remote ? h("span", { class: "muted" }, " and on the network") : null),
          i
            ? [
                h("dt", {}, "Now"),
                h(
                  "dd",
                  { class: "tabular" },
                  [
                    plural(i.workspaces, "workspace"),
                    `${plural(i.tabs_loaded, "tab")} loaded`,
                    `${plural(i.agents_running, "agent")} running`,
                    `${plural(i.jobs_running, "job")} running`,
                    i.pending_approvals ? `${plural(i.pending_approvals, "approval")} waiting` : null,
                  ]
                    .filter(Boolean)
                    .join(" · "),
                ),
              ]
            : null,
        ),
        d.attached ? h("p", { class: "field-note" }, icon("info", 14), "This page belongs to a terminal session, not the daemon; workspaces and tabs are not available here.") : null,
        h(
          "div",
          { class: "row-actions" },
          h("button", { class: "button small", type: "button", onclick: () => refresh("daemon").then(() => this.render()) }, icon("refresh", 14), "Refresh"),
          d.attached ? null : h("button", { class: "button small danger", type: "button", onclick: stop }, "Stop daemon"),
        ),
      ],
      { description: "One daemon per user holds every workspace, tab and agent. Terminals, browsers and editors are its clients." },
    );
  }

  // Advanced: raw TOML

  advanced() {
    const raw = this.raw;
    const files = [
      { id: "user", label: "User config", path: this.data.paths.user },
      { id: "project", label: "Project config", path: this.data.paths.project },
      { id: "control", label: "control.toml", path: this.data.paths.control },
    ];
    const current = files.find((f) => f.id === raw.file);
    if (!(raw.file in raw.loaded)) {
      raw.loaded[raw.file] = null;
      api(`/api/settings/raw/${raw.file}`)
        .then((data) => {
          raw.loaded[raw.file] = data.text;
          raw.texts[raw.file] ??= data.text;
          if (this.section === "advanced") this.render();
        })
        .catch((error) => {
          raw.message = { kind: "error", text: error.message };
          this.render();
        });
    }
    const loaded = raw.loaded[raw.file];
    const text = raw.texts[raw.file] ?? loaded ?? "";
    const editor = h("textarea", {
      class: "code-editor",
      spellcheck: "false",
      autocomplete: "off",
      "aria-label": `${current.label} TOML`,
      dataset: { focusKey: `raw:${raw.file}` },
      disabled: loaded == null,
      placeholder: raw.file === "project" ? "# No project config yet" : "",
    });
    editor.value = text;
    const lines = h("div", { class: "code-gutter", "aria-hidden": "true" });
    const paintLines = () => {
      const count = editor.value.split("\n").length;
      lines.textContent = Array.from({ length: count }, (_, i) => i + 1).join("\n");
    };
    paintLines();
    editor.addEventListener("input", () => {
      raw.texts[raw.file] = editor.value;
      paintLines();
      dirtyNote.hidden = editor.value === loaded;
    });
    editor.addEventListener("scroll", () => (lines.scrollTop = editor.scrollTop));
    editor.addEventListener("keydown", (event) => {
      if ((event.metaKey || event.ctrlKey) && event.key === "s") {
        event.preventDefault();
        saveRaw();
      }
    });
    const dirtyNote = h("span", { class: "muted small", hidden: text === loaded }, "Edited");
    const saveRaw = async () => {
      if (this.isDirty()) {
        const ok = await confirmDialog({
          title: "Discard unsaved form changes?",
          message: "Saving this file reloads the settings, which drops the unsaved changes in the other sections.",
          confirm: "Save file",
        });
        if (!ok) return;
      }
      try {
        const result = await api(`/api/settings/raw/${raw.file}`, { method: "PUT", body: { text: editor.value } });
        raw.loaded[raw.file] = editor.value;
        raw.message = result.applied
          ? { kind: "success", text: "Saved and applied." }
          : { kind: "warning", text: `Saved, not applied: ${result.message ?? "restart to apply"}` };
        markAllStale();
        await this.load();
      } catch (error) {
        raw.message = { kind: "error", text: error.message };
        this.render();
      }
    };
    const reload = () => {
      delete raw.loaded[raw.file];
      delete raw.texts[raw.file];
      raw.message = null;
      this.render();
    };
    // Arrow keys move between the file tabs.
    const onTabKey = (event) => {
      if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
      const index = files.findIndex((f) => f.id === raw.file);
      const next = files[(index + (event.key === "ArrowRight" ? 1 : files.length - 1)) % files.length];
      event.preventDefault();
      raw.file = next.id;
      raw.message = null;
      this.render();
      this.root.querySelector(`[data-focus-key="rawtab:${next.id}"]`)?.focus();
    };
    return [
      h(
        "div",
        { class: "tabs", role: "tablist", "aria-label": "Configuration file", onkeydown: onTabKey },
        files.map((f) =>
          h(
            "button",
            {
              class: "tab",
              role: "tab",
              type: "button",
              "aria-selected": String(f.id === raw.file),
              tabindex: f.id === raw.file ? "0" : "-1",
              dataset: { focusKey: `rawtab:${f.id}` },
              onclick: () => {
                raw.file = f.id;
                raw.message = null;
                this.render();
              },
            },
            f.label,
          ),
        ),
      ),
      h("p", { class: "file-note" }, h("span", { class: "path", title: current.path }, current.path), dirtyNote),
      raw.file === "project"
        ? h("p", { class: "field-note" }, icon("info", 14), "Without trust_project_config, the project file cannot set mode, providers, mcp, control or permissions.")
        : null,
      raw.message ? h("div", { class: `banner ${raw.message.kind}`, role: raw.message.kind === "error" ? "alert" : "status" }, icon(raw.message.kind === "error" ? "alert" : raw.message.kind === "success" ? "check" : "info", 16), h("pre", { class: "banner-text" }, raw.message.text)) : null,
      h("div", { class: "code-editor-wrap" }, lines, editor),
      h(
        "div",
        { class: "form-actions" },
        h("span", { class: "muted small" }, raw.file === "control" ? "Checked as TOML when saved." : "Validated before it is written."),
        h("button", { class: "button", type: "button", onclick: reload }, "Reload from disk"),
        h("button", { class: "button primary", type: "button", disabled: loaded == null, onclick: saveRaw }, "Save file"),
      ),
      this.group(
        "Files",
        h(
          "dl",
          { class: "kv paths" },
          Object.entries(this.data.paths).map(([name, path]) => [h("dt", {}, name), h("dd", { class: "path", title: path }, path)]),
        ),
        {
          actions: h(
            "button",
            {
              class: "button small",
              type: "button",
              disabled: this.data.reloadable === false,
              title: this.data.reloadable === false ? "This page belongs to a terminal session; restart it to apply" : "Rebuild the runtime from the files",
              onclick: () => this.apply(),
            },
            icon("refresh", 14),
            "Apply from files",
          ),
        },
      ),
    ];
  }
}


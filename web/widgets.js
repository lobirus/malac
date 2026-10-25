// Reusable controls: the model picker, mode switch, toggles and usage meter.

import { closePopover, formatCost, formatTokens, h, popover, setChildren, totalTokens } from "./dom.js";
import { icon } from "./icons.js";
import { contextWindow, ensure, modelGroups, refresh, resource, state } from "./store.js";

export const MODES = [
  { mode: "ask", label: "Ask", icon: "hand", help: "Confirm edits and commands" },
  { mode: "edits", label: "Edits", icon: "pencil", help: "Edit files freely, confirm commands" },
  { mode: "autopilot", label: "Autopilot", icon: "zap", help: "Never ask; deny rules still apply" },
];

function splitSpec(spec) {
  const index = String(spec ?? "").indexOf("/");
  return index < 0 ? ["", spec ?? ""] : [spec.slice(0, index), spec.slice(index + 1)];
}

/** Model text with a dimmed provider prefix. */
export function modelLabel(spec, fallback = "Choose a model") {
  if (!spec) return h("span", { class: "model-label muted" }, fallback);
  const [provider, name] = splitSpec(spec);
  return h(
    "span",
    { class: "model-label", title: spec },
    provider ? h("span", { class: "model-provider" }, `${provider}/`) : null,
    h("span", { class: "model-name" }, name),
  );
}

/**
 * A button that opens a searchable list of models grouped by provider.
 * `allowUnset` adds a first row that clears the value (shown as `unsetLabel`).
 */
export function modelPicker({
  value = null,
  onChange,
  allowUnset = false,
  unsetLabel = "Default",
  placeholder = "Choose a model",
  className = "",
  disabled = false,
  title = "Model",
  align = "start",
}) {
  let current = value;
  const button = h("button", {
    class: `model-picker ${className}`.trim(),
    type: "button",
    title,
    "aria-haspopup": "listbox",
    "aria-expanded": "false",
    disabled,
  });
  const paint = () => {
    setChildren(
      button,
      icon("cpu", 14, "model-picker-icon"),
      current ? modelLabel(current) : h("span", { class: "model-label muted" }, allowUnset ? unsetLabel : placeholder),
      icon("chevronDown", 14, "model-picker-chevron"),
    );
  };
  paint();
  button.addEventListener("click", () => {
    if (button.getAttribute("aria-expanded") === "true") {
      closePopover();
      return;
    }
    openModelMenu(button, {
      value: current,
      allowUnset,
      unsetLabel,
      align,
      onPick(spec) {
        current = spec;
        paint();
        onChange?.(spec);
      },
    });
  });
  return {
    el: button,
    get value() {
      return current;
    },
    set(spec) {
      current = spec;
      paint();
    },
  };
}

function openModelMenu(anchor, { value, allowUnset, unsetLabel, align, onPick }) {
  const search = h("input", {
    class: "model-search",
    type: "search",
    placeholder: "Search models, or type provider/model",
    "aria-label": "Search models",
    autocomplete: "off",
    spellcheck: "false",
  });
  const list = h("div", { class: "model-list", role: "listbox" });
  const footer = h("div", { class: "model-menu-foot" });
  const panel = h("div", { class: "model-menu" }, h("div", { class: "model-search-wrap" }, icon("search", 14), search), list, footer);
  let rows = [];
  let active = 0;

  const pick = (spec) => {
    handle.close();
    onPick(spec);
  };

  const render = () => {
    const query = search.value.trim().toLowerCase();
    const groups = modelGroups();
    const items = [];
    list.replaceChildren();
    if (allowUnset && !query) {
      items.push({ spec: null });
      list.append(row({ spec: null, label: unsetLabel, selected: value == null }, () => pick(null)));
    }
    for (const group of groups) {
      const models = group.models.filter((m) => !query || m.spec.toLowerCase().includes(query));
      if (!models.length && !(group.error && !query)) continue;
      list.append(
        h(
          "div",
          { class: "model-group" },
          h("span", { class: "model-group-name" }, group.provider),
          group.kind ? h("span", { class: "model-group-kind" }, group.kind) : null,
          group.error ? h("span", { class: "model-group-error", title: group.error }, group.error === "no API key" ? "no key" : "unreachable") : null,
        ),
      );
      for (const model of models) {
        items.push(model);
        list.append(
          row(
            {
              spec: model.spec,
              label: model.name,
              selected: model.spec === value,
              isDefault: model.spec === state.defaultModel,
              configured: model.configured,
            },
            () => pick(model.spec),
          ),
        );
      }
    }
    const custom = search.value.trim();
    if (custom.includes("/") && !items.some((item) => item.spec === custom)) {
      items.push({ spec: custom });
      list.append(row({ spec: custom, label: `Use ${custom}`, custom: true }, () => pick(custom)));
    }
    if (!items.length) list.append(h("div", { class: "model-empty" }, query ? "No matching models" : "No models configured"));
    rows = [...list.querySelectorAll(".model-option")];
    active = Math.max(0, rows.findIndex((r) => r.classList.contains("selected")));
    highlight();
    const models = resource("models");
    setChildren(
      footer,
      models.loading
        ? h("span", { class: "muted" }, "Asking providers for their models…")
        : h(
            "button",
            { class: "link", type: "button", onclick: () => refresh("models").then(render) },
            icon("refresh", 12),
            " Refresh from providers",
          ),
    );
  };

  const highlight = () => {
    rows.forEach((r, i) => r.classList.toggle("active", i === active));
    rows[active]?.scrollIntoView({ block: "nearest" });
  };

  search.addEventListener("input", render);
  search.addEventListener("keydown", (event) => {
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      if (!rows.length) return;
      active = (active + (event.key === "ArrowDown" ? 1 : rows.length - 1)) % rows.length;
      highlight();
    } else if (event.key === "Enter") {
      event.preventDefault();
      rows[active]?.click();
    }
  });

  const handle = popover(anchor, panel, { align, className: "model-popover" });
  const entry = ensure("models");
  render();
  entry.promise?.then(() => panel.isConnected && render());
  if (resource("models").loading) resource("models").promise?.then(() => panel.isConnected && render());
  search.focus();
}

function row({ label, selected, isDefault, configured, custom }, onclick) {
  return h(
    "button",
    {
      class: `model-option${selected ? " selected" : ""}${configured === false ? " available" : ""}`,
      type: "button",
      role: "option",
      "aria-selected": String(Boolean(selected)),
      onclick,
    },
    h("span", { class: "model-option-check" }, selected ? icon("check", 14) : null),
    h("span", { class: "model-option-name" }, label),
    isDefault ? h("span", { class: "pill accent" }, "default") : null,
    configured === false ? h("span", { class: "pill" }, "available") : null,
    custom ? h("span", { class: "pill" }, "custom") : null,
  );
}

/** Segmented ask / edits / autopilot control bound to the server mode. */
export function modeSwitch(onPick, { compact = false } = {}) {
  return h(
    "div",
    { class: `segmented mode-switch${compact ? " compact" : ""}`, role: "group", "aria-label": "Permission mode" },
    MODES.map((m) =>
      h(
        "button",
        {
          type: "button",
          class: `mode-${m.mode}`,
          "aria-pressed": String(state.mode === m.mode),
          title: `${m.label}: ${m.help}`,
          onclick: () => onPick(m.mode),
        },
        icon(m.icon, 14),
        h("span", { class: "segmented-label" }, m.label),
      ),
    ),
  );
}

export function toggle({ checked, onChange, label, disabled = false, id }) {
  const input = h("input", { type: "checkbox", role: "switch", checked, disabled, id });
  input.addEventListener("change", () => onChange?.(input.checked));
  return h(
    "label",
    { class: `switch${disabled ? " disabled" : ""}` },
    input,
    h("span", { class: "switch-track", "aria-hidden": "true" }, h("span", { class: "switch-thumb" })),
    label ? h("span", { class: "switch-label" }, label) : null,
  );
}

/** A ring showing how full the context window is. */
export function contextRing(fraction, size = 18) {
  const r = 7;
  const c = 2 * Math.PI * r;
  const clamped = Math.max(0, Math.min(1, fraction || 0));
  const level = clamped > 0.8 ? "high" : clamped > 0.6 ? "mid" : "low";
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 18 18");
  svg.setAttribute("width", String(size));
  svg.setAttribute("height", String(size));
  svg.setAttribute("class", `context-ring ${level}`);
  svg.setAttribute("aria-hidden", "true");
  svg.innerHTML =
    `<circle cx="9" cy="9" r="${r}" class="ring-track"/>` +
    `<circle cx="9" cy="9" r="${r}" class="ring-fill" stroke-dasharray="${(clamped * c).toFixed(2)} ${c.toFixed(2)}" transform="rotate(-90 9 9)"/>`;
  return svg;
}

/** Token, cost and context summary for an agent. */
export function usageMeter(agent) {
  const usage = agent.usage;
  const window = contextWindow(agent.model);
  const fraction = agent.contextTokens ? agent.contextTokens / window : 0;
  const details = [
    `Input ${formatTokens(usage?.input_tokens)}`,
    `Output ${formatTokens(usage?.output_tokens)}`,
    `Cache read ${formatTokens(usage?.cache_read_tokens)}`,
    `Cache write ${formatTokens(usage?.cache_write_tokens)}`,
    usage?.cost ? `Cost ${formatCost(usage.cost)}` : null,
    agent.contextTokens ? `Context ${formatTokens(agent.contextTokens)} of ${formatTokens(window)} (${Math.round(fraction * 100)}%)` : null,
  ]
    .filter(Boolean)
    .join("\n");
  if (!totalTokens(usage) && !agent.contextTokens) return null;
  return h(
    "div",
    { class: "usage-meter", title: details },
    agent.contextTokens ? contextRing(fraction) : null,
    agent.contextTokens ? h("span", { class: "usage-context" }, `${Math.round(fraction * 100)}%`) : null,
    h("span", { class: "usage-tokens" }, `${formatTokens(totalTokens(usage))} tok`),
    usage?.cost ? h("span", { class: "usage-cost" }, formatCost(usage.cost)) : null,
  );
}

/** Status dot; `running` pulses. */
export function statusDot(status, title) {
  return h("span", { class: `dot ${status}`, title: title ?? status });
}

export function pill(text, kind = "") {
  return h("span", { class: `pill ${kind}`.trim() }, text);
}

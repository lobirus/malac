// Workspaces and tabs: the workspace switcher, the tab bar, the folder
// picker for new tabs and the file picker for prompt attachments.

import { baseName, closePopover, confirmDialog, h, modal, plural, popover, promptDialog, relativeTime, setChildren, shortPath, toast } from "./dom.js";
import { icon } from "./icons.js";
import {
  api,
  closeTab,
  createTab,
  createWorkspace,
  deleteWorkspace,
  moveTab,
  navigate,
  openWorkspace,
  renameTab,
  renameWorkspace,
  state,
  tabAgents,
  tabStatus,
  workspaceAgents,
} from "./store.js";
import { statusDot } from "./widgets.js";

const MARKER_LABELS = {
  ".git": "git",
  ".malac": "malac",
  "AGENTS.md": "AGENTS.md",
  "Cargo.toml": "Rust",
  "package.json": "Node",
  "pyproject.toml": "Python",
  "go.mod": "Go",
};

export function markerPills(markers = []) {
  return markers.map((m) => h("span", { class: "pill marker" }, MARKER_LABELS[m] ?? m));
}

const STATUS_LABEL = { running: "Running", waiting: "Approval needed", completed: "Done", failed: "Failed", idle: "" };

// Workspace switcher

export function workspaceButton() {
  const ws = state.workspace;
  const button = h(
    "button",
    {
      class: "workspace-button",
      type: "button",
      "aria-haspopup": "dialog",
      "aria-expanded": "false",
      title: ws ? `Workspace ${ws.name}` : "Choose a workspace",
    },
    h(
      "span",
      { class: "workspace-button-text" },
      h("span", { class: "workspace-kicker" }, "Workspace"),
      h("span", { class: "workspace-name" }, ws?.name ?? "None"),
    ),
    icon("chevronDown", 14),
  );
  button.addEventListener("click", () => {
    if (button.getAttribute("aria-expanded") === "true") closePopover();
    else openWorkspaceMenu(button);
  });
  return button;
}

export function openWorkspaceMenu(anchor) {
  const list = h("ul", { class: "workspace-list", role: "list" });
  const render = () => {
    const items = [...state.workspaces].sort((a, b) => String(b.updated).localeCompare(String(a.updated)));
    setChildren(
      list,
      items.length
        ? items.map((w) =>
            h(
              "li",
              { class: `workspace-item${w.id === state.workspace?.id ? " current" : ""}` },
              h(
                "button",
                {
                  class: "workspace-open",
                  type: "button",
                  "aria-current": w.id === state.workspace?.id ? "true" : null,
                  onclick: () => {
                    handle.close();
                    switchWorkspace(w.id);
                  },
                },
                h("span", { class: "workspace-check" }, w.id === state.workspace?.id ? icon("check", 14) : null),
                h("span", { class: "workspace-item-name" }, w.name),
                h(
                  "span",
                  { class: "workspace-item-meta" },
                  runningIn(w) ? [statusDot("running"), `${runningIn(w)} running · `] : null,
                  plural(w.tabs, "tab"),
                  " · ",
                  relativeTime(w.updated),
                ),
              ),
              h(
                "button",
                { class: "icon-button small", type: "button", "aria-label": `Rename workspace ${w.name}`, title: "Rename", onclick: () => (handle.close(), renameWorkspaceFlow(w)) },
                icon("pencil", 14),
              ),
              h(
                "button",
                { class: "icon-button small danger-hover", type: "button", "aria-label": `Delete workspace ${w.name}`, title: "Delete", onclick: () => (handle.close(), deleteWorkspaceFlow(w)) },
                icon("trash", 14),
              ),
            ),
          )
        : h("li", { class: "empty-note" }, "No workspaces yet."),
    );
  };
  render();
  const panel = h(
    "div",
    { class: "workspace-menu" },
    h("h2", { class: "popover-title" }, "Workspaces"),
    h("p", { class: "popover-blurb" }, "A workspace is a set of tabs. Every client sees the same ones."),
    list,
    h(
      "div",
      { class: "popover-foot" },
      h("button", { class: "button small", type: "button", onclick: () => (handle.close(), newWorkspaceFlow()) }, icon("plus", 14), "New workspace"),
    ),
  );
  const handle = popover(anchor, panel, { className: "workspace-popover" });
  panel.querySelector(".workspace-item.current .workspace-open, .workspace-open, button")?.focus();
}

export async function switchWorkspace(id) {
  if (id === state.workspace?.id) return;
  try {
    const view = await openWorkspace(id);
    navigate(view.active_tab ? { name: "tab", tab: view.active_tab } : { name: "home" });
  } catch (error) {
    toast(error.message, "error");
  }
}

export async function newWorkspaceFlow() {
  const name = await promptDialog({ title: "New workspace", label: "Name", placeholder: "e.g. release-2.0", confirm: "Create" });
  if (!name) return;
  try {
    await createWorkspace(name);
    navigate({ name: "home" });
  } catch (error) {
    toast(error.message, "error");
  }
}

export async function renameWorkspaceFlow(w) {
  const name = await promptDialog({ title: "Rename workspace", label: "Name", value: w.name });
  if (!name || name === w.name) return;
  try {
    await renameWorkspace(w.id, name);
  } catch (error) {
    toast(error.message, "error");
  }
}

/** Agents working in a workspace: counted live for the shown one, from its summary otherwise. */
function runningIn(w) {
  if (w.id !== state.workspace?.id) return w.running ?? 0;
  return workspaceAgents().filter((a) => a.live && a.status === "running").length;
}

export async function deleteWorkspaceFlow(w) {
  const running = runningIn(w);
  const ok = await confirmDialog({
    title: `Delete workspace ${w.name}?`,
    message: running
      ? `${plural(running, "agent")} ${running === 1 ? "is" : "are"} working in it and will be stopped. Its tabs close; transcripts stay on disk.`
      : "Its tabs close. Transcripts stay on disk and can be opened again from History.",
    confirm: running ? "Stop agents and delete" : "Delete",
    danger: true,
  });
  if (!ok) return;
  try {
    await deleteWorkspace(w.id, running > 0);
    toast(`Deleted workspace ${w.name}`);
  } catch (error) {
    toast(error.message, "error");
  }
}

// Tab bar

/**
 * Tabs of the shown workspace. Arrow keys move focus, Enter opens,
 * Alt+Shift+arrows reorder, F2 renames, Delete closes; tabs can be dragged.
 */
export function renderTabBar(host, { onNewTab }) {
  const tabs = state.workspace?.tabs ?? [];
  const selected = state.route.name === "tab" ? state.tab : null;
  const items = tabs.map((tab, index) => tabItem(tab, index, tab.id === state.tab, selected === tab.id));
  const strip = h("div", { class: "tab-strip", role: "tablist", "aria-label": "Tabs", "aria-orientation": "horizontal" }, items);
  strip.addEventListener("keydown", (event) => onTabKey(event, tabs));
  setChildren(
    host,
    strip,
    h("button", { class: "icon-button tab-add", type: "button", "aria-label": "New tab", title: "New tab (T)", onclick: onNewTab }, icon("plus", 16)),
  );
  host.querySelector(".tab.current")?.scrollIntoView({ block: "nearest", inline: "nearest" });
}

function tabItem(tab, index, current, shown) {
  const status = tabStatus(tab);
  const agents = tabAgents(tab).length;
  const jobs = [...state.jobs.values()].filter((j) => j.tab === tab.id && j.status === "running").length;
  const label = [tab.name, tab.cwd, STATUS_LABEL[status], plural(agents, "agent"), jobs ? plural(jobs, "job") : null].filter(Boolean).join(", ");
  const main = h(
    "button",
    {
      class: "tab-main",
      type: "button",
      role: "tab",
      id: `tab-${tab.id}`,
      "aria-selected": String(current),
      tabindex: current ? "0" : "-1",
      title: `${tab.name}\n${tab.cwd}`,
      "aria-label": label,
      dataset: { tab: tab.id },
      onclick: () => navigate({ name: "tab", tab: tab.id }),
      ondblclick: () => renameTabFlow(tab),
    },
    h("span", { class: `tab-status ${status}`, "aria-hidden": "true" }),
    h("span", { class: "tab-text" }, h("span", { class: "tab-name" }, tab.name), h("span", { class: "tab-path" }, shortPath(tab.cwd, 2))),
    agents ? h("span", { class: "tab-count", "aria-hidden": "true", title: plural(agents, "agent") }, String(agents)) : null,
  );
  const close = h(
    "button",
    { class: "tab-close", type: "button", tabindex: "-1", "aria-label": `Close tab ${tab.name}`, title: "Close tab", onclick: () => closeTabFlow(tab) },
    icon("x", 12),
  );
  const item = h("div", { class: `tab${current ? " current" : ""}${shown ? " shown" : ""}`, draggable: "true", dataset: { tab: tab.id, index } }, main, close);
  item.addEventListener("dragstart", (event) => {
    event.dataTransfer.setData("text/x-malac-tab", tab.id);
    event.dataTransfer.effectAllowed = "move";
    item.classList.add("dragging");
  });
  item.addEventListener("dragend", () => item.classList.remove("dragging"));
  item.addEventListener("dragover", (event) => {
    if (!event.dataTransfer.types.includes("text/x-malac-tab")) return;
    event.preventDefault();
    const rect = item.getBoundingClientRect();
    item.classList.toggle("drop-before", event.clientX < rect.left + rect.width / 2);
    item.classList.toggle("drop-after", event.clientX >= rect.left + rect.width / 2);
  });
  item.addEventListener("dragleave", () => item.classList.remove("drop-before", "drop-after"));
  item.addEventListener("drop", (event) => {
    const id = event.dataTransfer.getData("text/x-malac-tab");
    item.classList.remove("drop-before", "drop-after");
    if (!id || id === tab.id) return;
    event.preventDefault();
    const tabs = state.workspace?.tabs ?? [];
    const from = tabs.findIndex((t) => t.id === id);
    const rect = item.getBoundingClientRect();
    let to = index + (event.clientX >= rect.left + rect.width / 2 ? 1 : 0);
    if (from < to) to -= 1;
    moveTab(id, to);
  });
  return item;
}

function onTabKey(event, tabs) {
  const target = event.target.closest?.(".tab-main");
  if (!target) return;
  const index = tabs.findIndex((t) => t.id === target.dataset.tab);
  const tab = tabs[index];
  const focusTab = (i) => document.getElementById(`tab-${tabs[i]?.id}`)?.focus();
  if ((event.key === "ArrowLeft" || event.key === "ArrowRight") && event.altKey && event.shiftKey) {
    event.preventDefault();
    const to = index + (event.key === "ArrowRight" ? 1 : -1);
    if (to < 0 || to >= tabs.length) return;
    moveTab(tab.id, to);
    requestAnimationFrame(() => document.getElementById(`tab-${tab.id}`)?.focus());
  } else if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
    event.preventDefault();
    focusTab((index + (event.key === "ArrowRight" ? 1 : tabs.length - 1)) % tabs.length);
  } else if (event.key === "Home" || event.key === "End") {
    event.preventDefault();
    focusTab(event.key === "Home" ? 0 : tabs.length - 1);
  } else if (event.key === "F2") {
    event.preventDefault();
    renameTabFlow(tab);
  } else if (event.key === "Delete") {
    event.preventDefault();
    closeTabFlow(tab);
  }
}

export async function renameTabFlow(tab) {
  const name = await promptDialog({ title: "Rename tab", label: "Name", value: tab.name });
  if (!name || name === tab.name) return;
  try {
    await renameTab(tab.id, name);
  } catch (error) {
    toast(error.message, "error");
  }
}

export async function closeTabFlow(tab) {
  const running = tabAgents(tab).filter((a) => a.live && a.status === "running").length;
  if (running) {
    const ok = await confirmDialog({
      title: `Close ${tab.name}?`,
      message: `${plural(running, "agent")} ${running === 1 ? "is" : "are"} working in this tab and will be stopped. Transcripts stay on disk.`,
      confirm: "Stop agents and close",
      danger: true,
    });
    if (!ok) return;
  }
  try {
    await closeTab(tab.id, running > 0);
  } catch (error) {
    toast(error.message, "error");
  }
}

// Folder picker

/** Recent folders: open tabs first, then where saved sessions ran. */
export function recentFolders() {
  const seen = new Set();
  const out = [];
  const add = (path) => {
    if (!path || seen.has(path)) return;
    seen.add(path);
    out.push(path);
  };
  for (const tab of state.workspace?.tabs ?? []) add(tab.cwd);
  for (const session of state.sessions) add(session.cwd);
  return out.slice(0, 8);
}

/**
 * Lets the user browse to a folder (`GET /api/fs`) and opens a tab there.
 * Resolves to the new tab id, or null.
 */
export function openFolderPicker({ start } = {}) {
  return new Promise((resolve) => {
    const ui = { listing: null, hidden: false, filter: "", error: null, busy: false };
    const pathInput = h("input", { class: "input", type: "text", "aria-label": "Folder path", placeholder: "Type a path, or ~/ for the home directory", spellcheck: "false", autocomplete: "off" });
    const filterInput = h("input", { class: "input small", type: "search", placeholder: "Filter", "aria-label": "Filter folders", autocomplete: "off" });
    const nameInput = h("input", { class: "input", type: "text", "aria-label": "Tab name", placeholder: "Tab name (default: the folder name)", autocomplete: "off" });
    const crumbs = h("nav", { class: "fs-crumbs", "aria-label": "Folder path" });
    const list = h("ul", { class: "fs-list", role: "listbox", "aria-label": "Folders" });
    const message = h("p", { class: "field-error", role: "alert", hidden: true });
    const hereMarkers = h("span", { class: "fs-here-markers" });
    const recent = h("div", { class: "fs-recent" });
    const hidden = h("input", { type: "checkbox" });
    const openButton = h("button", { class: "button primary", type: "button", disabled: true }, "Open folder");

    const load = async (path, { fromList = false } = {}) => {
      ui.busy = true;
      ui.error = null;
      render();
      try {
        const query = new URLSearchParams();
        if (path) query.set("path", path);
        if (ui.hidden) query.set("hidden", "true");
        ui.listing = await api(`/api/fs?${query}`);
        ui.filter = "";
        filterInput.value = "";
        pathInput.value = ui.listing.path;
        requestAnimationFrame(() => (pathInput.scrollLeft = pathInput.scrollWidth));
      } catch (error) {
        ui.error =
          error.status === 403
            ? `${path} is outside the folders malac may browse${ui.listing ? ` (below ${ui.listing.root})` : ""}.`
            : error.message;
        if (!ui.listing && path) return load(null);
      } finally {
        ui.busy = false;
      }
      render();
      // Keyboard users moving through the list keep their place in it.
      if (fromList) (list.querySelector(".fs-entry:not(.parent)") ?? list.querySelector(".fs-entry"))?.focus({ preventScroll: true });
    };

    const render = () => {
      const listing = ui.listing;
      message.textContent = ui.error ?? "";
      message.hidden = !ui.error;
      openButton.disabled = !listing || ui.busy;
      setChildren(openButton, listing ? `Open ${baseName(listing.path) || listing.path}` : "Open folder");
      if (!listing) {
        setChildren(list, h("li", { class: "empty-note" }, h("span", { class: "spinner small" }), "Loading"));
        return;
      }
      const root = listing.root;
      const relative = listing.path.startsWith(root) ? listing.path.slice(root.length).split("/").filter(Boolean) : [];
      setChildren(
        crumbs,
        h("button", { class: "crumb-button", type: "button", onclick: () => load(root), title: root }, baseName(root) || root),
        relative.map((part, i) => [
          h("span", { class: "crumb-sep", "aria-hidden": "true" }, icon("chevronRight", 12)),
          h(
            "button",
            { class: "crumb-button", type: "button", onclick: () => load(`${root}/${relative.slice(0, i + 1).join("/")}`) },
            part,
          ),
        ]),
      );
      setChildren(hereMarkers, markerPills(listing.markers));
      const filter = ui.filter.toLowerCase();
      const entries = listing.entries.filter((e) => !filter || e.name.toLowerCase().includes(filter));
      setChildren(
        list,
        listing.parent
          ? h(
              "li",
              {},
              h("button", { class: "fs-entry parent", type: "button", role: "option", onclick: () => load(listing.parent, { fromList: true }) }, icon("chevronLeft", 14), h("span", {}, "Up one level")),
            )
          : null,
        entries.map((entry) =>
          h(
            "li",
            {},
            h(
              "button",
              { class: "fs-entry", type: "button", role: "option", title: entry.path, onclick: () => load(entry.path, { fromList: true }) },
              icon("folder", 14),
              h("span", { class: "fs-name" }, entry.name),
              h("span", { class: "fs-markers" }, markerPills(entry.markers)),
            ),
          ),
        ),
        !entries.length ? h("li", { class: "empty-note" }, filter ? "No folders match." : "No folders here.") : null,
        listing.truncated ? h("li", { class: "empty-note" }, "Only the first 1000 folders are shown.") : null,
      );
      const folders = recentFolders();
      setChildren(
        recent,
        folders.length
          ? [
              h("span", { class: "muted small" }, "Recent"),
              folders.map((path) => h("button", { class: "chip", type: "button", title: path, onclick: () => load(path) }, baseName(path))),
            ]
          : null,
      );
    };

    const submit = async () => {
      if (!ui.listing) return;
      openButton.disabled = true;
      try {
        const tab = await createTab(ui.listing.path, nameInput.value.trim() || undefined);
        handle.close(tab);
      } catch (error) {
        ui.error = error.message;
        render();
      }
    };

    pathInput.addEventListener("keydown", (event) => {
      if (event.key === "Enter") {
        event.preventDefault();
        if (pathInput.value.trim()) load(pathInput.value.trim());
      }
    });
    filterInput.addEventListener("input", () => {
      ui.filter = filterInput.value;
      render();
    });
    hidden.addEventListener("change", () => {
      ui.hidden = hidden.checked;
      load(ui.listing?.path);
    });
    nameInput.addEventListener("keydown", (event) => {
      if (event.key === "Enter") {
        event.preventDefault();
        submit();
      }
    });
    // Arrow keys move through the folders; Backspace goes up; Ctrl/Cmd+Enter opens the folder shown.
    list.addEventListener("keydown", (event) => {
      const entries = [...list.querySelectorAll(".fs-entry")];
      const index = entries.indexOf(document.activeElement);
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        entries[Math.max(0, Math.min(entries.length - 1, index + (event.key === "ArrowDown" ? 1 : -1)))]?.focus();
      } else if (event.key === "Backspace" && ui.listing?.parent) {
        event.preventDefault();
        load(ui.listing.parent, { fromList: true });
      }
    });
    openButton.addEventListener("click", submit);

    const body = h(
      "div",
      { class: "folder-picker" },
      h("div", { class: "input-row" }, pathInput),
      h("div", { class: "fs-bar" }, crumbs, hereMarkers),
      recent,
      h(
        "div",
        { class: "fs-tools" },
        filterInput,
        h("label", { class: "check" }, hidden, h("span", {}, "Show hidden")),
      ),
      list,
      message,
      h("label", { class: "fs-name-field" }, h("span", { class: "label" }, "Tab name"), nameInput),
    );
    const handle = modal({
      title: "Open a folder in a new tab",
      body,
      className: "folder-modal",
      footer: [h("span", { class: "muted small modal-hint" }, "Ctrl+Enter opens the folder shown"), h("button", { class: "button", type: "button", onclick: () => handle.close(null) }, "Cancel"), openButton],
      onClose: (tab) => resolve(tab ?? null),
      initialFocus: () => pathInput,
    });
    handle.box.addEventListener("keydown", (event) => {
      if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
        event.preventDefault();
        submit();
      }
    });
    render();
    load(start ?? state.workspace?.tabs.find((t) => t.id === state.tab)?.cwd ?? null);
  });
}

// Attachments from the tab's folder

/**
 * A popover for attaching files or folders by path, relative to `cwd`.
 * It lists the folder's contents (`GET /api/fs?files=true`): a folder opens
 * for browsing, a file attaches. A typed path is read by the daemon when
 * the prompt is sent.
 */
/** `512 B`, `2.1 KB`, `3.0 MB`, as the daemon writes sizes. */
function byteSize(bytes) {
  const units = ["B", "KB", "MB", "GB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return unit === 0 ? `${bytes} B` : `${value.toFixed(1)} ${units[unit]}`;
}

export function openAttachPicker(anchor, { cwd, onAdd }) {
  const input = h("input", { class: "input", type: "text", placeholder: "src/main.rs or a folder", "aria-label": "Path to attach", autocomplete: "off", spellcheck: "false" });
  const list = h("ul", { class: "fs-list compact", role: "listbox", "aria-label": "Files and folders" });
  const note = h("p", { class: "muted small" }, "Paths are relative to ", h("span", { class: "path", title: cwd }, shortPath(cwd, 2)), ". Files are read when you send; a folder attaches as a listing.");
  let timer = 0;
  let base = "";
  const suggest = async () => {
    const value = input.value;
    base = value.includes("/") ? value.slice(0, value.lastIndexOf("/") + 1) : "";
    const prefix = value.slice(base.length).toLowerCase();
    const dir = `${cwd}/${base}`.replace(/\/+$/, "");
    try {
      const listing = await api(`/api/fs?${new URLSearchParams({ path: dir, files: "true" })}`);
      const entries = listing.entries.filter((e) => e.name.toLowerCase().startsWith(prefix)).slice(0, 40);
      setChildren(
        list,
        entries.length
          ? entries.map((entry) => {
              const folder = entry.kind === "directory";
              const open = () => {
                input.value = `${base}${entry.name}/`;
                input.focus();
                suggest();
              };
              const attachFile = () => {
                onAdd(`${base}${entry.name}`);
                input.value = base;
                input.focus();
                suggest();
              };
              return h(
                "li",
                {},
                h(
                  "button",
                  { class: "fs-entry", type: "button", role: "option", title: folder ? "Open this folder" : "Attach this file", onclick: folder ? open : attachFile },
                  icon(folder ? "folder" : "file", 14),
                  h("span", { class: "fs-name" }, folder ? `${entry.name}/` : entry.name),
                  folder ? null : h("span", { class: "fs-size muted tabular" }, byteSize(entry.size ?? 0)),
                ),
              );
            })
          : h("li", { class: "empty-note" }, value ? "Nothing here matches; a typed path still works." : "This folder is empty."),
      );
    } catch (error) {
      setChildren(list, h("li", { class: "empty-note" }, error.status === 403 ? "That folder is outside what malac may browse." : "Type a path to attach."));
    }
  };
  const add = () => {
    const path = input.value.trim().replace(/\/$/, "") || ".";
    onAdd(path);
    input.value = "";
    suggest();
  };
  input.addEventListener("input", () => {
    clearTimeout(timer);
    timer = setTimeout(suggest, 150);
  });
  input.addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      add();
    } else if (event.key === "ArrowDown") {
      event.preventDefault();
      list.querySelector(".fs-entry")?.focus();
    }
  });
  list.addEventListener("keydown", (event) => {
    const entries = [...list.querySelectorAll(".fs-entry")];
    const index = entries.indexOf(document.activeElement);
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const next = index + (event.key === "ArrowDown" ? 1 : -1);
      if (next < 0) input.focus();
      else entries[Math.min(entries.length - 1, next)]?.focus();
    }
  });
  const panel = h(
    "div",
    { class: "attach-menu" },
    h("h2", { class: "popover-title" }, "Attach from this tab's folder"),
    h("div", { class: "attach-body" }, h("div", { class: "input-row" }, input, h("button", { class: "button", type: "button", onclick: add }, "Attach")), note, list),
  );
  popover(anchor, panel, { className: "attach-popover" });
  input.focus();
  suggest();
}

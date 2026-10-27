// DOM and text helpers for the chat webview, plus malac's own line icons
// (the same set as the web UI, stroked with currentColor).

type Child = Node | string | number | null | undefined | false | Child[];
type Props = Record<string, unknown> | null;

export function h<K extends keyof HTMLElementTagNameMap>(tag: K, props?: Props, ...children: Child[]): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(props ?? {})) {
    if (value === null || value === undefined || value === false) continue;
    if (key.startsWith("on") && typeof value === "function") {
      node.addEventListener(key.slice(2).toLowerCase(), value as EventListener);
    } else if (key === "class") {
      node.className = String(value);
    } else if (key === "dataset" && typeof value === "object") {
      Object.assign(node.dataset, value);
    } else if (value === true) {
      node.setAttribute(key, "");
    } else {
      node.setAttribute(key, String(value));
    }
  }
  append(node, children);
  return node;
}

export function append(node: Node, children: Child[]): void {
  for (const child of children) {
    if (child === null || child === undefined || child === false) continue;
    if (Array.isArray(child)) append(node, child);
    else node.appendChild(child instanceof Node ? child : document.createTextNode(String(child)));
  }
}

export function setChildren(node: Node, ...children: Child[]): void {
  while (node.firstChild) node.removeChild(node.firstChild);
  append(node, children);
}

const SVG_NS = "http://www.w3.org/2000/svg";

const PATHS: Record<string, string> = {
  plus: "M12 5v14M5 12h14",
  search: "M10.5 17a6.5 6.5 0 1 0 0-13 6.5 6.5 0 0 0 0 13zM15.5 15.5 20 20",
  agents: "M6 7a2 2 0 1 0 0-4 2 2 0 0 0 0 4zM6 7v12h0M6 12h8M6 19h8M16 14a2 2 0 1 0 0-4 2 2 0 0 0 0 4zM16 21a2 2 0 1 0 0-4 2 2 0 0 0 0 4z",
  terminal: "M5 4h14a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2zM7 9l3 3-3 3M12.5 15H17",
  file: "M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8zM14 3v5h5M9 13h6M9 17h4",
  filePlus: "M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8zM14 3v5h5M12 11v6M9 14h6",
  pencil: "M4 20h4L19 9a2.8 2.8 0 0 0-4-4L4 16zM13.5 6.5l4 4",
  folder: "M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z",
  grep: "M4 6h11M4 11h6M4 16h5M16 18a3.5 3.5 0 1 0 0-7 3.5 3.5 0 0 0 0 7zM18.5 17l2.5 2.5",
  globe: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM3 12h18M12 3c2.5 2.4 3.8 5.4 3.8 9s-1.3 6.6-3.8 9c-2.5-2.4-3.8-5.4-3.8-9S9.5 5.4 12 3z",
  browser: "M5 4h14a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2zM3 9h18M6.5 6.5h.01M9 6.5h.01",
  monitor: "M5 4h14a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2zM8.5 20h7M12 16v4",
  bookmark: "M7 3.5h10a1 1 0 0 1 1 1V21l-6-4-6 4V4.5a1 1 0 0 1 1-1z",
  todo: "M11 6h9M11 12h9M11 18h9M4 6l1.5 1.5L8 5M4 12l1.5 1.5L8 11M4 18l1.5 1.5L8 17",
  plug: "M9 3v5M15 3v5M7 8h10v3a5 5 0 0 1-10 0zM12 16v5",
  chevronDown: "M6 9l6 6 6-6",
  chevronRight: "M9 6l6 6-6 6",
  x: "M6 6l12 12M18 6 6 18",
  check: "M5 12.5l4.5 4.5L19 7",
  copy: "M10 8h9a1 1 0 0 1 1 1v10a1 1 0 0 1-1 1h-9a1 1 0 0 1-1-1V9a1 1 0 0 1 1-1zM15 8V5a1 1 0 0 0-1-1H5a1 1 0 0 0-1 1v10a1 1 0 0 0 1 1h4",
  stop: "M7 6h10a1 1 0 0 1 1 1v10a1 1 0 0 1-1 1H7a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1z",
  send: "M12 19V5M6 11l6-6 6 6",
  image: "M5 4h14a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2zM9 11a2 2 0 1 0 0-4 2 2 0 0 0 0 4zM21 16l-5-5L6 20",
  alert: "M12 4 21 20H3zM12 10v4.5M12 17.5v.01",
  info: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM12 11v6M12 7.5v.01",
  external: "M14 4h6v6M20 4l-9 9M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5",
  shield: "M12 3l8 3v6c0 4.5-3.4 8-8 9-4.6-1-8-4.5-8-9V6z",
  cpu: "M8 6h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2zM10 10h4v4h-4zM9.5 3v3M14.5 3v3M9.5 18v3M14.5 18v3M3 9.5h3M3 14.5h3M18 9.5h3M18 14.5h3",
  eye: "M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12zM12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6z",
  eyeOff: "M3 3l18 18M10.6 5.6A9.6 9.6 0 0 1 12 5.5c6 0 9.5 6.5 9.5 6.5a17 17 0 0 1-3 3.8M6.6 6.6C4 8.3 2.5 12 2.5 12S6 18.5 12 18.5c1.6 0 3-.4 4.2-1.1M9.9 9.9a3 3 0 0 0 4.2 4.2",
  bulb: "M9.5 18h5M10.5 21h3M12 3a6 6 0 0 0-3.8 10.6c.7.6 1.3 1.5 1.3 2.4h5c0-.9.6-1.8 1.3-2.4A6 6 0 0 0 12 3z",
  more: "M5 12h.01M12 12h.01M19 12h.01",
  arrowDown: "M12 5v14M6 13l6 6 6-6",
  compress: "M4 14h6v6M20 10h-6V4M14 10l6.5-6.5M3.5 20.5 10 14",
  zap: "M13 2.5 4.5 13.5H11l-1 8 8.5-11H12z",
  hand: "M8 12V5.5a1.5 1.5 0 0 1 3 0V11M11 10V4.5a1.5 1.5 0 0 1 3 0V11M14 10.5V6a1.5 1.5 0 0 1 3 0v7c0 4-2.5 7.5-6.5 7.5-2.5 0-4-1.2-5.3-3.3L3.7 14a1.5 1.5 0 0 1 2.6-1.5L8 15",
  sparkle: "M12 3c.6 4.2 2.8 6.4 7 7-4.2.6-6.4 2.8-7 7-.6-4.2-2.8-6.4-7-7 4.2-.6 6.4-2.8 7-7zM19 16c.3 1.6 1 2.3 2.5 2.5-1.5.3-2.2 1-2.5 2.5-.3-1.5-1-2.2-2.5-2.5 1.5-.2 2.2-.9 2.5-2.5z",
  history: "M3.5 12a8.5 8.5 0 1 0 2.5-6M3.5 4v4.5H8M12 8v4l3 2",
  wait: "M7 3h10M7 21h10M8 3c0 4 8 5 8 9s-8 5-8 9M16 3c0 4-8 5-8 9",
  at: "M16 12a4 4 0 1 1-8 0 4 4 0 0 1 8 0zM16 12v1.5a2.5 2.5 0 0 0 5 0V12a9 9 0 1 0-3.5 7.1",
  paperclip: "M20 11.5l-8.2 8.2a5 5 0 0 1-7.1-7.1l8.5-8.5a3.3 3.3 0 0 1 4.7 4.7l-8.5 8.5a1.7 1.7 0 0 1-2.4-2.4l7.8-7.8",
  mic: "M12 3a3 3 0 0 0-3 3v5.5a3 3 0 0 0 6 0V6a3 3 0 0 0-3-3zM5.5 11a6.5 6.5 0 0 0 13 0M12 17.5V21M9 21h6",
  headphones: "M4 16v-4a8 8 0 0 1 16 0v4M4 16a2 2 0 0 1 2-2h1.5v6H6a2 2 0 0 1-2-2zM20 16a2 2 0 0 0-2-2h-1.5v6H18a2 2 0 0 0 2-2z",
  zoomIn: "M10.5 17a6.5 6.5 0 1 0 0-13 6.5 6.5 0 0 0 0 13zM15.5 15.5 20 20M8 10.5h5M10.5 8v5",
  zoomOut: "M10.5 17a6.5 6.5 0 1 0 0-13 6.5 6.5 0 0 0 0 13zM15.5 15.5 20 20M8 10.5h5",
  fit: "M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5",
  download: "M12 4v11M7 10.5l5 5 5-5M5 20h14",
};

const FILLED = new Set(["stop"]);

export function icon(name: string, size = 16, className = ""): SVGSVGElement {
  const node = document.createElementNS(SVG_NS, "svg");
  node.setAttribute("viewBox", "0 0 24 24");
  node.setAttribute("width", String(size));
  node.setAttribute("height", String(size));
  node.setAttribute("aria-hidden", "true");
  node.setAttribute("focusable", "false");
  node.setAttribute("class", `icon icon-${name} ${className}`.trim());
  const path = document.createElementNS(SVG_NS, "path");
  path.setAttribute("d", PATHS[name] ?? PATHS.more);
  if (FILLED.has(name)) {
    path.setAttribute("fill", "currentColor");
  } else {
    path.setAttribute("fill", "none");
    path.setAttribute("stroke", "currentColor");
    path.setAttribute("stroke-width", "1.8");
    path.setAttribute("stroke-linecap", "round");
    path.setAttribute("stroke-linejoin", "round");
  }
  node.append(path);
  return node;
}

/** The malac pig, a line drawing in currentColor. */
export function pig(size = 32): SVGSVGElement {
  const node = document.createElementNS(SVG_NS, "svg");
  node.setAttribute("viewBox", "0 0 32 32");
  node.setAttribute("width", String(size));
  node.setAttribute("height", String(size));
  node.setAttribute("aria-hidden", "true");
  node.setAttribute("class", "pig");
  const line = document.createElementNS(SVG_NS, "g");
  for (const [k, v] of Object.entries({ fill: "none", stroke: "currentColor", "stroke-width": "2", "stroke-linecap": "round", "stroke-linejoin": "round" })) {
    line.setAttribute(k, v);
  }
  const parts: [string, Record<string, string>][] = [
    ["path", { d: "M7.3 11.9 6.4 5.2l6.2 3.8M24.7 11.9l.9-6.7-6.2 3.8" }],
    ["ellipse", { cx: "16", cy: "18", rx: "11.5", ry: "9.5" }],
    ["ellipse", { cx: "16", cy: "20.6", rx: "4.4", ry: "3.2", class: "pig-snout" }],
  ];
  for (const [tag, attrs] of parts) {
    const el = document.createElementNS(SVG_NS, tag);
    for (const [k, v] of Object.entries(attrs)) el.setAttribute(k, v);
    line.append(el);
  }
  const solid = document.createElementNS(SVG_NS, "g");
  solid.setAttribute("fill", "currentColor");
  for (const [cx, cy, rx, ry] of [
    [11.6, 15.4, 1.2, 1.2],
    [20.4, 15.4, 1.2, 1.2],
    [14.5, 20.6, 0.75, 1.05],
    [17.5, 20.6, 0.75, 1.05],
  ]) {
    const el = document.createElementNS(SVG_NS, "ellipse");
    el.setAttribute("cx", String(cx));
    el.setAttribute("cy", String(cy));
    el.setAttribute("rx", String(rx));
    el.setAttribute("ry", String(ry));
    solid.append(el);
  }
  node.append(line, solid);
  return node;
}

export function toolIcon(name: string): string {
  if (name === "bash") return "terminal";
  if (name === "read") return "file";
  if (name === "write") return "filePlus";
  if (name === "edit") return "pencil";
  if (name === "glob") return "folder";
  if (name === "grep") return "grep";
  if (name === "web_fetch") return "globe";
  if (name === "todo") return "todo";
  if (name === "agent_spawn") return "agents";
  if (name === "agent_wait") return "wait";
  if (name.startsWith("memory_")) return "bookmark";
  if (name.startsWith("browser_")) return "browser";
  if (name.startsWith("computer_")) return "monitor";
  if (name.includes("__")) return "plug";
  return "sparkle";
}

// Text

export function formatTokens(n: number | undefined): string {
  const value = Number(n) || 0;
  if (value < 1000) return String(value);
  if (value < 1_000_000) return `${(value / 1000).toFixed(1).replace(/\.0$/, "")}k`;
  return `${(value / 1_000_000).toFixed(1).replace(/\.0$/, "")}M`;
}

export function formatCost(cost: number | undefined): string {
  if (!cost) return "";
  return `$${cost < 0.01 ? cost.toFixed(4) : cost.toFixed(2)}`;
}

export function formatDuration(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) return "";
  if (ms < 1000) return `${Math.round(ms)} ms`;
  const seconds = ms / 1000;
  if (seconds < 60) return `${seconds < 10 ? seconds.toFixed(1) : Math.round(seconds)} s`;
  const minutes = Math.floor(seconds / 60);
  const rest = Math.round(seconds % 60);
  return rest ? `${minutes} min ${rest} s` : `${minutes} min`;
}

export function formatBytes(bytes: number): string {
  const value = Number(bytes) || 0;
  if (value < 1024) return `${value} B`;
  if (value < 1024 * 1024) return `${Math.round(value / 1024)} KB`;
  return `${(value / 1024 / 1024).toFixed(1).replace(/\.0$/, "")} MB`;
}

/** 7 → "0:07", 75 → "1:15". */
export function formatClock(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

export function plural(count: number, word: string, many = `${word}s`): string {
  return `${count} ${count === 1 ? word : many}`;
}

export function truncate(text: string, max: number): string {
  return text.length > max ? `${text.slice(0, max - 1)}…` : text;
}

export function firstLine(text: unknown): string {
  return (String(text ?? "").split("\n").find((line) => line.trim()) ?? "").trim();
}

export function baseName(path: string): string {
  const parts = String(path).replace(/[\\/]+$/, "").split(/[\\/]/);
  return parts.at(-1) || path;
}

export function tildePath(path: string, home: string | null): string {
  if (home && (path === home || path.startsWith(`${home}/`))) return `~${path.slice(home.length)}`;
  return path;
}

export function relativeTo(path: string, base: string): string {
  if (base && path.startsWith(`${base}/`)) return path.slice(base.length + 1);
  return path;
}

export function clockTime(iso: string | null | undefined): string {
  if (!iso) return "";
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? "" : date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

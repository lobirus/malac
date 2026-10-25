// Inline SVG icons (24px grid, stroked with currentColor) and the pig logo.

const SVG_NS = "http://www.w3.org/2000/svg";

const PATHS = {
  plus: "M12 5v14M5 12h14",
  search: "M10.5 17a6.5 6.5 0 1 0 0-13 6.5 6.5 0 0 0 0 13zM15.5 15.5 20 20",
  settings: "M4 7h9M17 7h3M4 17h3M11 17h9M15 9a2 2 0 1 0 0-4 2 2 0 0 0 0 4zM9 19a2 2 0 1 0 0-4 2 2 0 0 0 0 4z",
  sun: "M12 16a4 4 0 1 0 0-8 4 4 0 0 0 0 8zM12 2.5v2M12 19.5v2M2.5 12h2M19.5 12h2M5.3 5.3l1.4 1.4M17.3 17.3l1.4 1.4M5.3 18.7l1.4-1.4M17.3 6.7l1.4-1.4",
  moon: "M20 14.5A8 8 0 0 1 9.5 4 8 8 0 1 0 20 14.5z",
  sidebar: "M5 4h14a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2zM9.5 4v16",
  home: "M4 10.5 12 4l8 6.5V19a1 1 0 0 1-1 1h-4.5v-6h-5v6H5a1 1 0 0 1-1-1z",
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
  database: "M12 8c4.4 0 8-1.3 8-3s-3.6-3-8-3-8 1.3-8 3 3.6 3 8 3zM4 5v14c0 1.7 3.6 3 8 3s8-1.3 8-3V5M4 12c0 1.7 3.6 3 8 3s8-1.3 8-3",
  todo: "M11 6h9M11 12h9M11 18h9M4 6l1.5 1.5L8 5M4 12l1.5 1.5L8 11M4 18l1.5 1.5L8 17",
  plug: "M9 3v5M15 3v5M7 8h10v3a5 5 0 0 1-10 0zM12 16v5",
  chevronDown: "M6 9l6 6 6-6",
  chevronRight: "M9 6l6 6-6 6",
  chevronLeft: "M15 6l-6 6 6 6",
  x: "M6 6l12 12M18 6 6 18",
  check: "M5 12.5l4.5 4.5L19 7",
  copy: "M10 8h9a1 1 0 0 1 1 1v10a1 1 0 0 1-1 1h-9a1 1 0 0 1-1-1V9a1 1 0 0 1 1-1zM15 8V5a1 1 0 0 0-1-1H5a1 1 0 0 0-1 1v10a1 1 0 0 0 1 1h4",
  trash: "M4 7h16M9 7V4.5h6V7M6.5 7l.9 12.2a1 1 0 0 0 1 .8h7.2a1 1 0 0 0 1-.8L17.5 7M10 11v5.5M14 11v5.5",
  stop: "M7 6h10a1 1 0 0 1 1 1v10a1 1 0 0 1-1 1H7a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1z",
  send: "M12 19V5M6 11l6-6 6 6",
  image: "M5 4h14a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2zM9 11a2 2 0 1 0 0-4 2 2 0 0 0 0 4zM21 16l-5-5L6 20",
  refresh: "M20 12a8 8 0 1 1-2.3-5.7M20 4v4.5h-4.5",
  key: "M8 20a4.5 4.5 0 1 0 0-9 4.5 4.5 0 0 0 0 9zM11.2 12.3 20 3.5M16.5 7l3 3M14.5 9l2 2",
  alert: "M12 4 21 20H3zM12 10v4.5M12 17.5v.01",
  info: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM12 11v6M12 7.5v.01",
  clock: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM12 7v5l3.5 2",
  external: "M14 4h6v6M20 4l-9 9M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5",
  keyboard: "M4 6h16a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2zM6 10h.01M10 10h.01M14 10h.01M18 10h.01M7.5 14h9",
  shield: "M12 3l8 3v6c0 4.5-3.4 8-8 9-4.6-1-8-4.5-8-9V6z",
  cpu: "M8 6h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2zM10 10h4v4h-4zM9.5 3v3M14.5 3v3M9.5 18v3M14.5 18v3M3 9.5h3M3 14.5h3M18 9.5h3M18 14.5h3",
  layers: "M12 3l9 5-9 5-9-5zM3 13l9 5 9-5",
  code: "M8.5 7 3.5 12l5 5M15.5 7l5 5-5 5M13.5 4.5l-3 15",
  eye: "M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12zM12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6z",
  server: "M5 4h14a2 2 0 0 1 2 2v3a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2zM5 13h14a2 2 0 0 1 2 2v3a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-3a2 2 0 0 1 2-2zM7 7.5h.01M7 16.5h.01",
  bulb: "M9.5 18h5M10.5 21h3M12 3a6 6 0 0 0-3.8 10.6c.7.6 1.3 1.5 1.3 2.4h5c0-.9.6-1.8 1.3-2.4A6 6 0 0 0 12 3z",
  user: "M12 12a4 4 0 1 0 0-8 4 4 0 0 0 0 8zM4.5 20.5a7.5 7.5 0 0 1 15 0",
  more: "M5 12h.01M12 12h.01M19 12h.01",
  arrowDown: "M12 5v14M6 13l6 6 6-6",
  compress: "M4 14h6v6M20 10h-6V4M14 10l6.5-6.5M3.5 20.5 10 14",
  zap: "M13 2.5 4.5 13.5H11l-1 8 8.5-11H12z",
  lock: "M7 11h10a1 1 0 0 1 1 1v7a1 1 0 0 1-1 1H7a1 1 0 0 1-1-1v-7a1 1 0 0 1 1-1zM8.5 11V7.5a3.5 3.5 0 0 1 7 0V11",
  hand: "M8 12V5.5a1.5 1.5 0 0 1 3 0V11M11 10V4.5a1.5 1.5 0 0 1 3 0V11M14 10.5V6a1.5 1.5 0 0 1 3 0v7c0 4-2.5 7.5-6.5 7.5-2.5 0-4-1.2-5.3-3.3L3.7 14a1.5 1.5 0 0 1 2.6-1.5L8 15",
  sparkle: "M12 3c.6 4.2 2.8 6.4 7 7-4.2.6-6.4 2.8-7 7-.6-4.2-2.8-6.4-7-7 4.2-.6 6.4-2.8 7-7zM19 16c.3 1.6 1 2.3 2.5 2.5-1.5.3-2.2 1-2.5 2.5-.3-1.5-1-2.2-2.5-2.5 1.5-.2 2.2-.9 2.5-2.5z",
  bug: "M9 7.5V6a3 3 0 0 1 6 0v1.5M8 9h8a1 1 0 0 1 1 1v4a5 5 0 0 1-10 0v-4a1 1 0 0 1 1-1zM12 9v10M4 13h3M17 13h3M5 8l2.5 2M19 8l-2.5 2M5 19l2.5-2M19 19l-2.5-2",
  test: "M9 3h6M10 3v6l-5 9a2 2 0 0 0 1.7 3h10.6a2 2 0 0 0 1.7-3l-5-9V3M7.5 14h9",
  branch: "M6 3v12M6 21a2 2 0 1 0 0-4 2 2 0 0 0 0 4zM18 9a2 2 0 1 0 0-4 2 2 0 0 0 0 4zM18 9c0 5-6 4-11.5 8",
  book: "M4 5.5A2.5 2.5 0 0 1 6.5 3H20v15H6.5A2.5 2.5 0 0 0 4 20.5zM4 20.5A2.5 2.5 0 0 0 6.5 23H20v-5",
  history: "M3.5 12a8.5 8.5 0 1 0 2.5-6M3.5 4v4.5H8M12 8v4l3 2",
  wait: "M7 3h10M7 21h10M8 3c0 4 8 5 8 9s-8 5-8 9M16 3c0 4-8 5-8 9",
};

const FILLED = new Set(["stop"]);

export function icon(name, size = 16, className = "") {
  const node = document.createElementNS(SVG_NS, "svg");
  node.setAttribute("viewBox", "0 0 24 24");
  node.setAttribute("width", String(size));
  node.setAttribute("height", String(size));
  node.setAttribute("aria-hidden", "true");
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

// The pig: a monochrome line drawing in currentColor, the same as
// favicon.svg. The snout carries a class so CSS can make it sniff while
// agents are working.
const PIG = `
  <g fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
    <path d="M7.3 11.9 6.4 5.2l6.2 3.8M24.7 11.9l.9-6.7-6.2 3.8"/>
    <ellipse cx="16" cy="18" rx="11.5" ry="9.5"/>
    <ellipse class="pig-snout" cx="16" cy="20.6" rx="4.4" ry="3.2"/>
  </g>
  <g fill="currentColor">
    <circle cx="11.6" cy="15.4" r="1.2"/>
    <circle cx="20.4" cy="15.4" r="1.2"/>
    <ellipse cx="14.5" cy="20.6" rx=".75" ry="1.05"/>
    <ellipse cx="17.5" cy="20.6" rx=".75" ry="1.05"/>
  </g>`;

export function pigLogo(size = 24, className = "") {
  const node = document.createElementNS(SVG_NS, "svg");
  node.setAttribute("viewBox", "0 0 32 32");
  node.setAttribute("width", String(size));
  node.setAttribute("height", String(size));
  node.setAttribute("class", `pig ${className}`.trim());
  node.setAttribute("aria-hidden", "true");
  node.innerHTML = PIG;
  return node;
}

/** Icon for a tool by name; MCP tools (`server__tool`) get a plug. */
export function toolIcon(name) {
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

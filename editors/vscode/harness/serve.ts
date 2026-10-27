// A browser harness for the chat webview: runs the extension's real chat
// controller, store and daemon client in Node against a real daemon, and
// serves the webview page with a stubbed `acquireVsCodeApi` that relays
// messages over HTTP (POST /bridge) and server-sent events (GET /events).
// It also renders the tree view models and the Usage view as HTML.
//
//   MALAC_HOME=/tmp/home MALAC_BIN=target/debug/malac HARNESS_CWD=/some/project \
//     node out/harness/serve.js            # http://127.0.0.1:7811/?theme=dark
//
// Environment: HARNESS_PORT (7811), HARNESS_CWD (the project the fake editor
// and "@" search use), HARNESS_FALLBACK_WS=1 (use the built-in WebSocket
// client instead of the runtime's), HARNESS_CONNECTION (a saved connection in
// connections.json, which makes the daemon count as remote), HARNESS_VOICE_BIN
// (a malac with the voice commands, or a stand-in, for the microphone button).

import { randomBytes } from "node:crypto";
import * as fs from "node:fs";
import * as http from "node:http";
import * as path from "node:path";
import { ChatController, ChatHost, ChatVoice } from "../src/chat/controller";
import { chatHtml } from "../src/chat/html";
import type { FromWebview, ToWebview } from "../src/chat/messages";
import { DaemonClient } from "../src/connection/client";
import { LocalEndpoints, malacHome, SavedEndpoints } from "../src/connection/endpoint";
import { Launcher, parseVersion, runMalac } from "../src/connection/launcher";
import type { EditorSnapshot } from "../src/model/context";
import { FileHit, rankFiles } from "../src/model/files";
import { statusSummary } from "../src/model/status";
import { DaemonStore } from "../src/model/store";
import { jobNodes, sessionNodes, TreeNode, workspaceTree } from "../src/model/tree";
import { usageHtml, usageModel } from "../src/model/usage";
import { featureReady, type SessionSummary, type UsageReport } from "../src/protocol";
import { Dictation } from "../src/voice/cli";
import { DictationManager } from "../src/voice/dictation";

const ROOT = path.resolve(__dirname, "..", "..");
const PORT = Number(process.env.HARNESS_PORT ?? 7811);
const CWD = path.resolve(process.env.HARNESS_CWD ?? process.cwd());
const BIN = process.env.MALAC_BIN ?? "malac";
const HOME = malacHome(undefined);

const store = new DaemonStore();
const launcher = new Launcher(() => ({ command: BIN, extraArgs: [], cwd: CWD, home: HOME }), {
  appendLine: (line) => console.log(`[malac] ${line}`),
  append: (text) => process.stdout.write(`[malac] ${text}`),
});
let version: string | undefined;
const client = new DaemonClient({
  // HARNESS_CONNECTION names a saved connection (connections.json), as malac.connection does.
  provider: () => (process.env.HARNESS_CONNECTION ? new SavedEndpoints(process.env.HARNESS_CONNECTION, () => HOME) : new LocalEndpoints(() => HOME, launcher)),
  version: () => version,
  log: (line) => console.log(`[client] ${line}`),
  forceFallbackSocket: process.env.HARNESS_FALLBACK_WS === "1",
});
client.onMessage.event((message) => store.apply(message));

const listeners = new Set<http.ServerResponse>();
const backlog: ToWebview[] = [];
let editor: { path: string; selection: EditorSnapshot["selection"]; dirty?: boolean } | null = null;
const memory = new Map<string, unknown>();
const posted: ToWebview[] = [];
let files: FileHit[] | null = null;

const host: ChatHost = {
  post(message) {
    posted.push(message);
    if (posted.length > 500) posted.shift();
    const data = `data: ${JSON.stringify(message)}\n\n`;
    if (!listeners.size) backlog.push(message);
    for (const response of listeners) response.write(data);
  },
  editor() {
    if (!editor) return null;
    const text = fs.existsSync(editor.path) ? fs.readFileSync(editor.path, "utf8") : "";
    const selected = (() => {
      const s = editor!.selection;
      if (!s) return "";
      const lines = text.split("\n");
      const chunk = lines.slice(s.startLine, s.endLine + 1);
      if (!chunk.length) return "";
      chunk[chunk.length - 1] = chunk[chunk.length - 1].slice(0, s.endLine === s.startLine ? s.endCharacter : s.endCharacter);
      chunk[0] = chunk[0].slice(s.startCharacter);
      return chunk.join("\n");
    })();
    return {
      path: editor.path,
      displayPath: path.relative(CWD, editor.path),
      languageId: path.extname(editor.path).slice(1) || "plaintext",
      selection: editor.selection,
      selectedText: selected,
      dirty: Boolean(editor.dirty),
      text: () => text,
    };
  },
  async findFiles(query) {
    files ??= walk(CWD).map((p) => ({ path: p, relative: path.relative(CWD, p) }));
    return rankFiles(files, query, 30);
  },
  openFile(file, line) {
    console.log(`[host] open ${file}${line ? `:${line}` : ""}`);
  },
  async saveUpload(name, data) {
    const dir = path.join(HOME, "harness-uploads");
    fs.mkdirSync(dir, { recursive: true });
    const target = path.join(dir, path.basename(name));
    fs.writeFileSync(target, data);
    return target;
  },
  // Opening and saving write to the scratch home; VS Code opens or asks instead.
  async openMedia(name, data, key) {
    const dir = path.join(HOME, "harness-open", key || "image");
    fs.mkdirSync(dir, { recursive: true });
    fs.writeFileSync(path.join(dir, path.basename(name)), data);
    console.log(`[host] open ${path.join(dir, path.basename(name))} (${data.length} bytes)`);
  },
  async saveMedia(name, data) {
    const dir = path.join(HOME, "harness-saved");
    fs.mkdirSync(dir, { recursive: true });
    const target = path.join(dir, path.basename(name));
    fs.writeFileSync(target, data);
    return target;
  },
  runCommand(command, args) {
    console.log(`[host] command ${command} ${JSON.stringify(args)}`);
    if (command === "malac.reconnect") client.reconnect({ allowStart: true });
    if (command === "malac.folderTab.open" && typeof args[0] === "string") {
      void client.request({ type: "tab_create", cwd: args[0], reuse: true }).then((reply) => chat.selectTab(reply.tab as string));
    }
    if (command === "malac.newTabForFolder") {
      void client.request({ type: "tab_create", cwd: CWD, reuse: true }).then((reply) => chat.selectTab(reply.tab as string));
    }
  },
  remember(key, value) {
    memory.set(key, value);
  },
  recall<T>(key: string) {
    return memory.get(key) as T | undefined;
  },
  log(line) {
    console.log(`[chat] ${line}`);
  },
  clientVersion: () => version,
  dropHint: "Drop files here (hold Shift while dragging in VS Code)",
};

// Dictation through HARNESS_VOICE_BIN; talk mode needs VS Code's terminal, so the button only logs.
const dictation = new DictationManager(async () => {
  if (!process.env.HARNESS_VOICE_BIN) throw new Error("Set HARNESS_VOICE_BIN to a malac with voice commands to dictate in the harness.");
  return new Dictation({ command: process.env.HARNESS_VOICE_BIN, home: HOME, connect: process.env.HARNESS_CONNECTION ?? null }, { log: (line) => console.log(`[dictate] ${line}`) });
});
const voice: ChatVoice = {
  offer: () => ({ dictation: featureReady(store.voice?.dictation), talk: featureReady(store.voice?.talk), blocked: null }),
  dictation,
  onChange: () => ({ dispose() {} }),
};

const chat = new ChatController(store, client, host, { voice });

// When the snapshot has no tab for the project, make one (what the folder offer does in VS Code).
store.onChange.event((change) => {
  if (!change.snapshot) return;
  const tab = store.tabForCwd(CWD);
  if (tab) {
    if (!chat.currentTab) chat.selectTab(tab.id);
    return;
  }
  void client
    .request({ type: "tab_create", cwd: CWD, reuse: true })
    .then((reply) => chat.selectTab(reply.tab as string))
    .catch((error: Error) => console.log(`[harness] tab_create failed: ${error.message}`));
});

function walk(dir: string, out: string[] = [], depth = 0): string[] {
  if (depth > 8 || out.length > 5000) return out;
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    if (entry.name.startsWith(".") || entry.name === "node_modules" || entry.name === "target") continue;
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) walk(full, out, depth + 1);
    else out.push(full);
  }
  return out;
}

// Theme variables VS Code injects into webviews (a subset of Dark Modern and Light Modern).
const THEMES: Record<string, Record<string, string>> = {
  dark: {
    "font-family": "-apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif",
    "font-size": "13px",
    "font-weight": "normal",
    "editor-font-family": "Menlo, Monaco, 'Courier New', monospace",
    "editor-font-size": "12px",
    foreground: "#cccccc",
    descriptionForeground: "#9d9d9d",
    disabledForeground: "#cccccc80",
    errorForeground: "#f85149",
    "sideBar-background": "#181818",
    "editor-background": "#1f1f1f",
    "input-background": "#313131",
    "input-foreground": "#cccccc",
    "input-border": "#3c3c3c",
    "input-placeholderForeground": "#989898",
    focusBorder: "#0078d4",
    "button-background": "#0078d4",
    "button-foreground": "#ffffff",
    "button-hoverBackground": "#026ec1",
    "button-border": "#ffffff12",
    "button-secondaryBackground": "#313131",
    "button-secondaryForeground": "#cccccc",
    "button-secondaryHoverBackground": "#3c3c3c",
    "toolbar-hoverBackground": "#5a5d5e50",
    "widget-border": "#313131",
    "widget-shadow": "#0000005c",
    "editorWidget-background": "#202020",
    "textCodeBlock-background": "#2b2b2b",
    "textLink-foreground": "#4daafc",
    "textLink-activeForeground": "#4daafc",
    "textPreformat-foreground": "#d0d0d0",
    "badge-background": "#616161",
    "badge-foreground": "#f8f8f8",
    "charts-green": "#89d185",
    "charts-yellow": "#cca700",
    "editorWarning-foreground": "#cca700",
    "testing-iconPassed": "#73c991",
    "inputValidation-infoBackground": "#063b49",
    "inputValidation-infoBorder": "#007acc",
    "inputValidation-warningBackground": "#352a05",
    "inputValidation-warningBorder": "#b89500",
    "inputValidation-errorBackground": "#5a1d1d",
    "inputValidation-errorBorder": "#be1100",
    "menu-background": "#1f1f1f",
    "menu-foreground": "#cccccc",
    "menu-selectionBackground": "#0078d4",
    "menu-selectionForeground": "#ffffff",
    "menu-border": "#454545",
    "menu-separatorBackground": "#454545",
    "quickInput-background": "#222222",
    "quickInput-foreground": "#cccccc",
    "quickInputList-focusBackground": "#04395e",
    "quickInputList-focusForeground": "#ffffff",
    "diffEditor-insertedLineBackground": "#9bb95533",
    "diffEditor-removedLineBackground": "#ff000033",
    "notifications-background": "#1f1f1f",
    "notifications-border": "#2b2b2b",
    "notifications-foreground": "#cccccc",
    "icon-foreground": "#cccccc",
    "sideBarSectionHeader-border": "#2b2b2b",
    "panel-border": "#2b2b2b",
    "chat-requestBackground": "#ffffff08",
    "chat-requestBorder": "#ffffff1a",
    "list-hoverBackground": "#2a2d2e",
    "list-activeSelectionBackground": "#04395e",
    "gitDecoration-addedResourceForeground": "#81b88b",
    "gitDecoration-deletedResourceForeground": "#c74e39",
    "tree-indentGuidesStroke": "#585858",
  },
  light: {
    "font-family": "-apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif",
    "font-size": "13px",
    "font-weight": "normal",
    "editor-font-family": "Menlo, Monaco, 'Courier New', monospace",
    "editor-font-size": "12px",
    foreground: "#3b3b3b",
    descriptionForeground: "#717171",
    disabledForeground: "#61616180",
    errorForeground: "#f85149",
    "sideBar-background": "#f8f8f8",
    "editor-background": "#ffffff",
    "input-background": "#ffffff",
    "input-foreground": "#3b3b3b",
    "input-border": "#cecece",
    "input-placeholderForeground": "#767676",
    focusBorder: "#005fb8",
    "button-background": "#005fb8",
    "button-foreground": "#ffffff",
    "button-hoverBackground": "#0258a8",
    "button-border": "#0000001a",
    "button-secondaryBackground": "#e5e5e5",
    "button-secondaryForeground": "#3b3b3b",
    "button-secondaryHoverBackground": "#cccccc",
    "toolbar-hoverBackground": "#b8b8b850",
    "widget-border": "#e5e5e5",
    "widget-shadow": "#00000029",
    "editorWidget-background": "#f8f8f8",
    "textCodeBlock-background": "#f2f2f2",
    "textLink-foreground": "#005fb8",
    "textLink-activeForeground": "#005fb8",
    "textPreformat-foreground": "#3b3b3b",
    "badge-background": "#cccccc",
    "badge-foreground": "#3b3b3b",
    "charts-green": "#388a34",
    "charts-yellow": "#bf8803",
    "editorWarning-foreground": "#bf8803",
    "testing-iconPassed": "#388a34",
    "inputValidation-infoBackground": "#d6ecf2",
    "inputValidation-infoBorder": "#007acc",
    "inputValidation-warningBackground": "#f6f5d2",
    "inputValidation-warningBorder": "#b89500",
    "inputValidation-errorBackground": "#f2dede",
    "inputValidation-errorBorder": "#be1100",
    "menu-background": "#ffffff",
    "menu-foreground": "#3b3b3b",
    "menu-selectionBackground": "#005fb8",
    "menu-selectionForeground": "#ffffff",
    "menu-border": "#cecece",
    "menu-separatorBackground": "#d4d4d4",
    "quickInput-background": "#f8f8f8",
    "quickInput-foreground": "#3b3b3b",
    "quickInputList-focusBackground": "#e8e8e8",
    "quickInputList-focusForeground": "#000000",
    "diffEditor-insertedLineBackground": "#9bb95533",
    "diffEditor-removedLineBackground": "#ff000033",
    "notifications-background": "#ffffff",
    "notifications-border": "#e5e5e5",
    "notifications-foreground": "#3b3b3b",
    "icon-foreground": "#3b3b3b",
    "sideBarSectionHeader-border": "#e5e5e5",
    "panel-border": "#e5e5e5",
    "chat-requestBackground": "#0000000a",
    "chat-requestBorder": "#0000001a",
    "list-hoverBackground": "#f2f2f2",
    "list-activeSelectionBackground": "#e8e8e8",
    "gitDecoration-addedResourceForeground": "#587c0c",
    "gitDecoration-deletedResourceForeground": "#ad0707",
    "tree-indentGuidesStroke": "#a9a9a9",
  },
};

function themeCss(theme: string): string {
  const vars = THEMES[theme] ?? THEMES.dark;
  return `:root{${Object.entries(vars)
    .map(([k, v]) => `--vscode-${k}:${v}`)
    .join(";")}} body{background:var(--vscode-sideBar-background)}`;
}

function stub(nonce: string, theme: string): string {
  return `<style nonce="${nonce}">${themeCss(theme)}</style>
<script nonce="${nonce}">
  (() => {
    let saved = null;
    try { saved = JSON.parse(sessionStorage.getItem("vscode-state") || "null"); } catch {}
    window.__posted = [];
    window.acquireVsCodeApi = () => ({
      postMessage(message) {
        window.__posted.push(message);
        fetch("/bridge", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(message) });
      },
      getState: () => saved,
      setState(state) { saved = state; sessionStorage.setItem("vscode-state", JSON.stringify(state)); },
    });
    const events = new EventSource("/events");
    window.__received = [];
    events.onmessage = (event) => {
      const data = JSON.parse(event.data);
      window.__received.push(data);
      window.dispatchEvent(new MessageEvent("message", { data }));
    };
    document.documentElement.classList.add("${theme === "light" ? "vscode-light" : "vscode-dark"}");
  })();
</script>`;
}

function page(theme: string): string {
  const nonce = randomBytes(16).toString("base64");
  const origin = `http://127.0.0.1:${PORT}`;
  return chatHtml({
    cspSource: origin,
    nonce,
    scriptUri: `${origin}/out/src/webview/main.mjs`,
    styleUri: `${origin}/media/chat.css`,
    head: stub(nonce, theme),
  }).replace("default-src 'none'", "default-src 'none'; connect-src 'self'");
}

// Tree views as HTML, in the look of VS Code's tree widget.

const GLYPHS: Record<string, string> = {
  "sync~spin": '<span class="ring spin"></span>',
  "bell-dot": "&#9873;",
  pass: "&#10003;",
  error: "&#10007;",
  "circle-slash": "&#8856;",
  "circle-outline": "&#9675;",
  history: "&#8634;",
  folder: "&#128193;",
  "folder-opened": "&#128194;",
  layers: "&#9776;",
  "layers-active": "&#9776;",
  comment: "&#9633;",
  "comment-discussion": "&#9635;",
  question: "?",
};

function treeHtml(title: string, nodes: TreeNode[], theme: string): string {
  const vars = THEMES[theme] ?? THEMES.dark;
  const colors: Record<string, string> = {
    "charts.green": vars["charts-green"],
    "charts.yellow": vars["charts-yellow"],
    "testing.iconPassed": vars["testing-iconPassed"],
    errorForeground: vars.errorForeground,
  };
  const render = (node: TreeNode, depth: number): string => {
    const hasChildren = Boolean(node.children?.length) || node.kind === "workspace";
    const open = hasChildren && !node.collapsed;
    const color = node.icon?.color ? ` style="color:${colors[node.icon.color] ?? "inherit"}"` : "";
    const icon = node.icon ? `<span class="icon"${color}>${GLYPHS[node.icon.id] ?? "&#8226;"}</span>` : `<span class="icon"></span>`;
    const twisty = hasChildren ? (open ? "&#9662;" : "&#9656;") : "";
    const row = `<div class="row" style="padding-left:${8 + depth * 16}px" title="${escape(node.tooltip ?? "")}" data-context="${node.contextValue ?? ""}"><span class="twisty">${twisty}</span>${icon}<span class="label">${escape(node.label)}</span><span class="desc">${escape(node.description ?? "")}</span></div>`;
    return row + (open ? (node.children ?? []).map((c) => render(c, depth + 1)).join("") : "");
  };
  return `<section><h3>${escape(title)}</h3>${nodes.map((n) => render(n, 0)).join("")}</section>`;
}

function escape(text: string): string {
  return text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

async function treesPage(theme: string): Promise<string> {
  let sessions: SessionSummary[] = [];
  try {
    sessions = (await client.get<{ sessions: SessionSummary[] }>("/api/sessions?limit=50")).sessions;
  } catch {
    // Not connected.
  }
  const live = new Set([...store.agents.values()].filter((a) => a.live).map((a) => a.id));
  const status = statusSummary(store, client.connection, version);
  const body = [
    `<div class="statusbar">${escape(status.text.replace(/\$\(([^)]+)\)/g, "[$1]"))} <span class="desc">${escape(status.tooltip.join(" · "))}</span></div>`,
    treeHtml("Workspaces", workspaceTree(store, { chatTab: chat.currentTab, chatAgent: chat.currentAgent }), theme),
    treeHtml("Jobs", jobNodes(store.jobs.values()), theme),
    treeHtml("Sessions", sessionNodes(sessions, new Date(), live), theme),
  ].join("");
  return `<!doctype html><html><head><meta charset="utf-8"><style>${themeCss(theme)}
body{margin:0;font:13px var(--vscode-font-family);color:var(--vscode-foreground);width:340px}
h3{margin:0;padding:4px 12px;font-size:11px;font-weight:700;text-transform:uppercase;border-top:1px solid var(--vscode-sideBarSectionHeader-border)}
.row{display:flex;align-items:center;gap:6px;height:22px;white-space:nowrap;overflow:hidden}
.row:hover{background:var(--vscode-list-hoverBackground)}
.twisty{flex:none;width:10px;color:var(--vscode-foreground);font-size:10px}
.icon{flex:none;width:16px;text-align:center}
.label{flex:0 1 auto;min-width:40%;overflow:hidden;text-overflow:ellipsis}
.desc{flex:0 1 auto;min-width:0;color:var(--vscode-descriptionForeground);font-size:12px;overflow:hidden;text-overflow:ellipsis}
.ring{display:inline-block;width:9px;height:9px;border:1.5px solid currentColor;border-right-color:transparent;border-radius:50%}
.spin{animation:spin 1s linear infinite}@keyframes spin{to{transform:rotate(360deg)}}
.statusbar{padding:4px 12px;background:#007acc;color:#fff;font-size:12px}.statusbar .desc{color:#e6f2fa}
</style></head><body>${body}</body></html>`;
}

const MIME: Record<string, string> = { ".mjs": "text/javascript", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".map": "application/json" };

const server = http.createServer(async (request, response) => {
  const url = new URL(request.url ?? "/", `http://127.0.0.1:${PORT}`);
  const theme = url.searchParams.get("theme") ?? "dark";
  try {
    if (request.method === "GET" && url.pathname === "/") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" });
      response.end(page(theme));
    } else if (request.method === "GET" && url.pathname === "/events") {
      response.writeHead(200, { "Content-Type": "text/event-stream", "Cache-Control": "no-cache", Connection: "keep-alive" });
      response.write(":ok\n\n");
      listeners.add(response);
      for (const message of backlog.splice(0)) response.write(`data: ${JSON.stringify(message)}\n\n`);
      request.on("close", () => listeners.delete(response));
    } else if (request.method === "POST" && url.pathname === "/bridge") {
      const body = await readBody(request);
      chat.receive(JSON.parse(body) as FromWebview);
      response.writeHead(204).end();
    } else if (request.method === "POST" && url.pathname === "/harness/editor") {
      editor = JSON.parse(await readBody(request));
      chat.editorChanged();
      response.writeHead(204).end();
    } else if (request.method === "POST" && url.pathname === "/harness/message") {
      // A daemon message, as if broadcast (e.g. an approval request to look at).
      store.apply(JSON.parse(await readBody(request)));
      response.writeHead(204).end();
    } else if (request.method === "POST" && url.pathname === "/harness/attach") {
      const { paths } = JSON.parse(await readBody(request)) as { paths: string[] };
      chat.addAttachments(paths.map((p) => ({ path: p, label: path.relative(CWD, p) })));
      response.writeHead(204).end();
    } else if (request.method === "POST" && url.pathname === "/harness/command") {
      const { command } = JSON.parse(await readBody(request)) as { command: string };
      if (command === "newAgent") chat.newAgent();
      if (command === "reconnect") client.reconnect({ allowStart: true });
      response.writeHead(204).end();
    } else if (request.method === "GET" && url.pathname === "/harness/state") {
      response.writeHead(200, { "Content-Type": "application/json" });
      const { endpoint, ...connection } = client.connection;
      const where = endpoint ? { url: endpoint.url, source: endpoint.source } : null;
      response.end(JSON.stringify({ chat: chat.state(), connection: { ...connection, endpoint: where }, posted: posted.slice(-50).map((m) => m.type) }));
    } else if (request.method === "GET" && url.pathname === "/trees") {
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" });
      response.end(await treesPage(theme));
    } else if (request.method === "GET" && url.pathname === "/usage") {
      let html: string;
      try {
        const report = await client.get<UsageReport>("/api/usage?days=7");
        html = usageHtml(usageModel(report), { nonce: "harness" });
      } catch (error) {
        html = usageHtml(null, { nonce: "harness", message: String(error) });
      }
      response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" });
      response.end(html.replace("<body>", `<body><style nonce="harness">${themeCss(theme)}</style>`));
    } else if (request.method === "GET" && (url.pathname.startsWith("/media/") || url.pathname.startsWith("/out/src/webview/"))) {
      const file = path.join(ROOT, path.normalize(url.pathname));
      if (!file.startsWith(ROOT) || !fs.existsSync(file)) {
        response.writeHead(404).end();
        return;
      }
      response.writeHead(200, { "Content-Type": MIME[path.extname(file)] ?? "application/octet-stream", "Cache-Control": "no-cache" });
      response.end(fs.readFileSync(file));
    } else {
      response.writeHead(404).end();
    }
  } catch (error) {
    response.writeHead(500, { "Content-Type": "text/plain" });
    response.end(String(error));
  }
});

function readBody(request: http.IncomingMessage): Promise<string> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = [];
    request.on("data", (chunk: Buffer) => chunks.push(chunk));
    request.on("end", () => resolve(Buffer.concat(chunks).toString("utf8")));
    request.on("error", reject);
  });
}

void (async () => {
  try {
    version = parseVersion(await runMalac(BIN, ["--version"], HOME, 5000)) ?? undefined;
  } catch {
    version = undefined;
  }
  client.start({ autoStart: true });
  server.listen(PORT, "127.0.0.1", () => console.log(`harness on http://127.0.0.1:${PORT}/ (MALAC_HOME=${HOME}, project ${CWD})`));
})();

const stop = () => {
  client.dispose();
  launcher.dispose();
  server.close();
  process.exit(0);
};
process.on("SIGINT", stop);
process.on("SIGTERM", stop);

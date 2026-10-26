import { randomBytes } from "node:crypto";
import * as vscode from "vscode";

/** Where the web UI is served: the daemon's base URL and token. */
export interface ServerAddress {
  url: string;
  token: string;
}

/**
 * "Open full UI": a webview panel that frames the whole malac web UI. Prompts are queued until the
 * webview script reports that it is running, and the script holds them until
 * the frame has loaded.
 */
export class MalacPanel implements vscode.Disposable {
  private readonly panel: vscode.WebviewPanel;
  private address: ServerAddress | undefined;
  private ready = false;
  private readonly outbox: string[] = [];

  constructor(onDispose: () => void) {
    this.panel = vscode.window.createWebviewPanel("malac.fullUi", "malac", vscode.ViewColumn.Beside, {
      enableScripts: true,
      retainContextWhenHidden: true,
      localResourceRoots: [],
    });
    this.panel.onDidDispose(onDispose);
    this.panel.webview.onDidReceiveMessage((message: { type?: string }) => {
      if (message?.type !== "ready") return;
      this.ready = true;
      this.flush();
    });
  }

  async show(address: ServerAddress): Promise<void> {
    if (address.url !== this.address?.url || address.token !== this.address?.token) {
      this.address = address;
      this.ready = false;
      this.panel.webview.html = await frameHtml(address);
    }
    this.panel.reveal();
  }

  prompt(text: string): void {
    this.outbox.push(text);
    this.flush();
  }

  dispose(): void {
    this.panel.dispose();
  }

  private flush(): void {
    if (!this.ready) return;
    for (const text of this.outbox.splice(0)) {
      void this.panel.webview.postMessage({ type: "malac.prompt", text });
    }
  }
}

async function frameHtml(address: ServerAddress): Promise<string> {
  const base = await vscode.env.asExternalUri(vscode.Uri.parse(address.url));
  const origin = `${base.scheme}://${base.authority}`;
  const src = base.with({ path: "/", query: address.token ? `token=${address.token}&embed=1` : "embed=1" }).toString(true);
  const nonce = randomBytes(16).toString("base64");
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; frame-src ${origin}; style-src 'nonce-${nonce}'; script-src 'nonce-${nonce}';">
<style nonce="${nonce}">
  html, body, iframe { width: 100%; height: 100%; margin: 0; padding: 0; border: 0; overflow: hidden; }
  iframe { display: block; }
</style>
</head>
<body>
<iframe id="malac" title="malac" src="${escapeAttribute(src)}"></iframe>
<script nonce="${nonce}">
  const vscode = acquireVsCodeApi();
  const frame = document.getElementById("malac");
  const origin = ${JSON.stringify(origin)};
  const waiting = [];
  let loaded = false;
  const relay = (message) => frame.contentWindow.postMessage(message, origin);
  frame.addEventListener("load", () => {
    loaded = true;
    waiting.splice(0).forEach(relay);
  });
  window.addEventListener("message", (event) => {
    if (event.origin === origin || event.data?.type !== "malac.prompt") return;
    if (loaded) relay(event.data);
    else waiting.push(event.data);
  });
  vscode.postMessage({ type: "ready" });
</script>
</body>
</html>`;
}

function escapeAttribute(value: string): string {
  return value.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;");
}

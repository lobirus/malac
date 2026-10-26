// The chat webview's page. The script is an ES module from the extension's
// own files; it talks only to the extension host (postMessage), which holds
// the daemon connection, so the page needs no network access at all.

export interface ChatPage {
  /** `webview.cspSource`. */
  cspSource: string;
  nonce: string;
  scriptUri: string;
  styleUri: string;
  /** Extra markup before the script, used by the browser harness. */
  head?: string;
}

export function chatHtml({ cspSource, nonce, scriptUri, styleUri, head = "" }: ChatPage): string {
  const csp = [
    "default-src 'none'",
    `img-src ${cspSource} data: blob:`,
    `style-src ${cspSource} 'nonce-${nonce}'`,
    `font-src ${cspSource}`,
    `script-src 'nonce-${nonce}' ${cspSource}`,
  ].join("; ");
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta http-equiv="Content-Security-Policy" content="${csp}">
<link rel="stylesheet" href="${escapeAttribute(styleUri)}">
<title>malac</title>
${head}
</head>
<body>
<div id="app" class="app" aria-busy="true">
  <header class="header" id="header"></header>
  <div class="banners" id="banners" role="status" aria-live="polite"></div>
  <main class="scroller" id="scroller" tabindex="-1">
    <div class="transcript" id="transcript" role="log" aria-live="polite" aria-label="Conversation"></div>
  </main>
  <button class="jump" id="jump" type="button" hidden aria-label="Jump to the latest message"></button>
  <section class="approvals" id="approvals" aria-label="Approvals waiting"></section>
  <form class="composer" id="composer" aria-label="Message malac"></form>
</div>
<div class="toasts" id="toasts" role="status" aria-live="polite"></div>
<script type="module" nonce="${nonce}" src="${escapeAttribute(scriptUri)}"></script>
</body>
</html>`;
}

function escapeAttribute(value: string): string {
  return value.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;");
}

// The service worker: shows the approvals the daemon pushes, also with no
// malac page open, and answers them from the notification where the
// browser offers buttons (Android and desktop; on iPhone a tap opens
// malac). The browser has decrypted each push already; what one says is in
// docs/protocol.md (Push). Every push shows a notification: Safari takes
// the permission back from sites whose pushes show nothing.

const TOKEN_CACHE = "malac-push";
const TOKEN_URL = "/push-token";

self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", (event) => event.waitUntil(self.clients.claim()));

self.addEventListener("push", (event) => {
  let message = {};
  try {
    message = event.data?.json() ?? {};
  } catch {
    // Not JSON: still show something.
  }
  event.waitUntil(show(message));
});

async function show(message) {
  const requests = (message.requests ?? []).filter((request) => request && typeof request === "object");
  const detailed = requests.filter((request) => request.tool);
  for (const request of detailed) {
    const body = [request.summary, request.place].filter(Boolean).join("\n");
    await self.registration.showNotification(`${request.agent_label || "agent"} wants to run ${request.tool}`, {
      body,
      tag: `approval-${request.id}`,
      icon: "/icon-192.png",
      data: { request: request.id, tab: request.tab ?? null, agent: request.agent ?? null },
      actions: [
        { action: "allow", title: "Allow" },
        { action: "deny", title: "Deny" },
      ],
    });
  }
  const rest = requests.length - detailed.length;
  if (rest > 0 || message.more || detailed.length === 0) {
    await self.registration.showNotification(detailed.length ? "More approvals are waiting" : "malac needs your approval", {
      body: "Open malac to answer.",
      tag: "approvals",
      icon: "/icon-192.png",
      data: {},
    });
  }
}

self.addEventListener("notificationclick", (event) => {
  const { request, tab, agent } = event.notification.data ?? {};
  event.notification.close();
  const page = tab && agent ? `/#tab=${tab}&agent=${agent}` : "/";
  if (request && (event.action === "allow" || event.action === "deny")) {
    event.waitUntil(answer(request, event.action).then((done) => done || open(page)));
  } else {
    event.waitUntil(open(page));
  }
});

/** Answers with this device's token, which the page keeps here; `false` when that did not work. */
async function answer(request, decision) {
  const cached = await (await caches.open(TOKEN_CACHE)).match(TOKEN_URL);
  const token = cached ? await cached.text() : "";
  if (!token) return false;
  try {
    const response = await fetch("/api/command", {
      method: "POST",
      headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
      body: JSON.stringify({ type: "approve", request_id: request, decision }),
    });
    return response.ok;
  } catch {
    return false;
  }
}

/** Shows `page` in an open malac window, which routes itself (keeping what was typed), or a new one. */
async function open(page) {
  const windows = await self.clients.matchAll({ type: "window", includeUncontrolled: true });
  const existing = windows.find((client) => new URL(client.url).origin === self.location.origin);
  if (existing) {
    existing.postMessage({ type: "open", hash: new URL(page, self.location.origin).hash });
    return existing.focus();
  }
  return self.clients.openWindow(page);
}

// Notifications on this device: approvals the daemon pushes, shown by the
// service worker (sw.js) even with no malac page open. Push needs a secure
// page (https, or this machine), a paired device token and, on iPhone and
// iPad, malac opened from the Home Screen. See docs/remote.md.

import { h, toast } from "./dom.js";
import { icon } from "./icons.js";
import { api, state } from "./store.js";

/** Where the service worker finds this device's token, to answer from a notification. */
const TOKEN_CACHE = "malac-push";
const TOKEN_URL = "/push-token";

/** What the settings group shows, read once per visit. */
const cache = { status: null, subscribed: false, loading: false, busy: false, error: null };

/** Why this page cannot get pushes, or null when it can. */
function blocker() {
  if (!window.isSecureContext) return "insecure";
  const apple = /iPhone|iPad|iPod/.test(navigator.userAgent) || (navigator.platform === "MacIntel" && navigator.maxTouchPoints > 1);
  const installed = matchMedia("(display-mode: standalone)").matches || navigator.standalone === true;
  if (apple && !installed) return "home-screen";
  if (!("serviceWorker" in navigator) || !("PushManager" in window) || !("Notification" in window)) return "unsupported";
  return null;
}

/**
 * Registers the service worker, keeps its copy of the token current, and
 * routes the page when a notification is tapped.
 */
export async function startPush() {
  if (blocker()) return;
  navigator.serviceWorker.addEventListener("message", (event) => {
    if (event.data?.type === "open" && typeof event.data.hash === "string") location.hash = event.data.hash;
  });
  // Browsers get no push when a request is answered elsewhere (each push
  // must show something), so an open page takes those notifications down.
  document.addEventListener("malac:pending", closeAnswered);
  try {
    await navigator.serviceWorker.register("/sw.js");
    const registration = await navigator.serviceWorker.ready;
    if (await registration.pushManager.getSubscription()) await keepToken();
  } catch (error) {
    console.warn("malac: no service worker", error);
  }
}

async function closeAnswered() {
  const registration = await navigator.serviceWorker.getRegistration();
  for (const notification of (await registration?.getNotifications()) ?? []) {
    const request = notification.data?.request;
    if (request ? !state.pending.has(request) : state.pending.size === 0) notification.close();
  }
}

async function keepToken() {
  const store = await caches.open(TOKEN_CACHE);
  await store.put(TOKEN_URL, new Response(state.token ?? ""));
}

function keyBytes(base64url) {
  const text = atob(base64url.replace(/-/g, "+").replace(/_/g, "/"));
  return Uint8Array.from(text, (c) => c.charCodeAt(0));
}

function sameKey(buffer, bytes) {
  if (!buffer) return false;
  const have = new Uint8Array(buffer);
  return have.length === bytes.length && have.every((b, i) => b === bytes[i]);
}

async function load(view) {
  if (cache.loading) return;
  cache.loading = true;
  try {
    cache.status = await api("/api/push");
    const registration = blocker() ? null : await navigator.serviceWorker.getRegistration();
    cache.subscribed = Boolean(await registration?.pushManager.getSubscription());
  } catch (error) {
    cache.error = error.message;
  }
  view.render();
}

/** From a click: Safari and others subscribe only in answer to a gesture. */
async function turnOn(view) {
  cache.busy = true;
  cache.error = null;
  view.render();
  try {
    const permission = await Notification.requestPermission();
    if (permission !== "granted") {
      throw new Error(permission === "denied" ? "Notifications are blocked for this site. Allow them in the browser's settings, then try again." : "Notifications were not allowed.");
    }
    const key = keyBytes(cache.status.vapid_key);
    const registration = await navigator.serviceWorker.ready;
    let subscription = await registration.pushManager.getSubscription();
    // One made with another daemon's key cannot carry this one's pushes.
    if (subscription && !sameKey(subscription.options?.applicationServerKey, key)) {
      await subscription.unsubscribe();
      subscription = null;
    }
    subscription ??= await registration.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: key });
    const { endpoint, keys } = subscription.toJSON();
    await api("/api/push", { method: "PUT", body: { endpoint, p256dh: keys.p256dh, auth: keys.auth, kind: "browser" } });
    await keepToken();
    cache.subscribed = true;
    cache.status = { ...cache.status, registered: true };
    toast("Notifications are on for this device");
  } catch (error) {
    cache.error = error.message;
  } finally {
    cache.busy = false;
    view.render();
  }
}

async function turnOff(view) {
  cache.busy = true;
  cache.error = null;
  view.render();
  try {
    const registration = await navigator.serviceWorker.getRegistration();
    await (await registration?.pushManager.getSubscription())?.unsubscribe();
    await api("/api/push", { method: "DELETE" });
    await (await caches.open(TOKEN_CACHE)).delete(TOKEN_URL);
    cache.subscribed = false;
    cache.status = { ...cache.status, registered: false };
  } catch (error) {
    cache.error = error.message;
  } finally {
    cache.busy = false;
    view.render();
  }
}

const BLOCKERS = {
  insecure: "Notifications need a secure address: open malac over https, through a tunnel such as Tailscale Serve or Cloudflare, or on this machine.",
  "home-screen": "On iPhone and iPad, add malac to the Home Screen first: tap Share, then Add to Home Screen. Open it from there and pair it; the Home Screen app does not share Safari's pairing.",
  unsupported: "This browser cannot receive push notifications.",
};

/** The Settings group: this device's notifications, and the daemon's push settings. */
export function pushGroup(view) {
  if (!cache.status && !cache.error) load(view);
  const why = blocker();
  const status = cache.status;
  const on = Boolean(status?.registered && cache.subscribed);
  let note = null;
  let action = null;
  if (why) {
    note = BLOCKERS[why];
  } else if (status && !status.device) {
    note = "Only paired devices get notifications. This page uses this computer's own token: open malac on the device you want notified and pair it.";
  } else if (status && !status.enabled) {
    note = "The daemon does not push: turn on Push approvals below.";
  } else if (status) {
    action = on
      ? h("button", { class: "button small", type: "button", disabled: cache.busy, onclick: () => turnOff(view) }, "Turn off")
      : h("button", { class: "button primary small", type: "button", disabled: cache.busy, onclick: () => turnOn(view) }, icon("bell", 14), "Turn on");
  }
  const children = [
    h(
      "div",
      { class: "remote-status" },
      h("span", { class: `badge ${on ? "on" : "warn"}` }, on ? "On" : "Off"),
      h(
        "p",
        {},
        on
          ? "A request that waits unanswered shows up here as a notification, with Allow and Deny where this browser offers buttons."
          : "Get a notification when an approval waits, also with no malac page open.",
      ),
    ),
    note ? h("p", { class: "field-note" }, icon("info", 14), note) : null,
    action ? h("div", { class: "row-actions" }, action) : null,
    cache.error ? h("p", { class: "field-error" }, cache.error) : null,
    view.field(["push", "enabled"], { type: "toggle", label: "Push approvals", help: "To every device that turned notifications on: phones with the app, and browsers here.", default: true }),
    view.field(["push", "wait_seconds"], { type: "int", label: "Wait before pushing", help: "Seconds a request waits unanswered before devices are woken, so answering at the computer keeps them quiet. 0 pushes at once.", placeholder: "15" }),
  ];
  return view.group("Notifications", children, { description: "Approvals pushed to this device and to your phones." });
}

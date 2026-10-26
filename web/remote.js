// Remote access in the browser: pairing this browser with a daemon on
// another machine, and the Settings section that shows whether other
// devices may connect, from where, the paired devices, tunnels,
// keep-awake and how to connect.

import { confirmDialog, copyButton, formatDuration, h, local, plural, setChildren, toast } from "./dom.js";
import { icon, pigLogo } from "./icons.js";
import { api, send, state } from "./store.js";

/** Where a paired browser keeps its device token across restarts. */
export const DEVICE_KEY = "malac.device";
const KIND_LABELS = { tunnel: "Tunnel", public: "Public", lan: "Network", local: "This machine or an SSH tunnel" };

/** The pairing code in the address (`#pair=CODE`), taken out of it. */
export function takePairCode() {
  const match = /(?:^#|&)pair=([^&]+)/.exec(location.hash);
  if (!match) return null;
  const rest = location.hash.replace(match[0], "").replace(/^#&/, "#");
  history.replaceState(null, "", `${location.pathname}${location.search}${rest === "#" ? "" : rest}`);
  return decodeURIComponent(match[1]);
}

function defaultDeviceName() {
  const ua = navigator.userAgent;
  const platform = /iPhone/.test(ua) ? "iPhone" : /iPad/.test(ua) ? "iPad" : /Android/.test(ua) ? "Android" : /Mac/.test(ua) ? "Mac" : /Windows/.test(ua) ? "Windows" : /Linux/.test(ua) ? "Linux" : "device";
  const browser = /Edg\//.test(ua) ? "Edge" : /Firefox\//.test(ua) ? "Firefox" : /Chrome\//.test(ua) ? "Chrome" : /Safari\//.test(ua) ? "Safari" : "Browser";
  return `${browser} on ${platform}`;
}

/**
 * A full-page form that trades a pairing code for this browser's device
 * token, then calls `onPaired(token)`.
 */
export function pairScreen(container, { code = "", reason = "", onPaired }) {
  const codeInput = h("input", {
    class: "input pair-code-input",
    value: code,
    placeholder: "ABCDE-FGHJK",
    autocomplete: "one-time-code",
    autocapitalize: "characters",
    spellcheck: "false",
    "aria-label": "Pairing code",
  });
  const nameInput = h("input", { class: "input", value: defaultDeviceName(), "aria-label": "Device name", spellcheck: "false" });
  const error = h("p", { class: "field-error", role: "alert", hidden: true });
  const button = h("button", { class: "button primary wide", type: "submit" }, "Pair this browser");
  const submit = async (event) => {
    event.preventDefault();
    error.hidden = true;
    button.disabled = true;
    try {
      const response = await fetch("/api/pair", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ code: codeInput.value, name: nameInput.value, kind: "browser" }),
      });
      const data = await response.json().catch(() => null);
      if (!response.ok) throw new Error(data?.error ?? (response.status === 403 ? "Remote access is off on that machine." : `${response.status} ${response.statusText}`));
      local.set(DEVICE_KEY, data.token);
      onPaired(data.token);
    } catch (problem) {
      error.textContent = problem.message;
      error.hidden = false;
      button.disabled = false;
    }
  };
  const form = h(
    "form",
    { class: "pair-form", onsubmit: submit },
    h("label", { class: "label" }, "Pairing code", codeInput),
    h("label", { class: "label" }, "Name for this browser", nameInput),
    error,
    button,
  );
  setChildren(
    container,
    h(
      "div",
      { class: "view-scroll" },
      h(
        "div",
        { class: "fatal pair-screen" },
        pigLogo(40),
        h("h1", { class: "page-title" }, "Pair this browser"),
        reason ? h("p", { class: "field-note" }, icon("info", 14), reason) : null,
        h("p", { class: "muted" }, "On the machine running malac, run ", h("code", {}, "malac remote pair"), " (or open Settings → Remote access there) and enter the code it shows. Scanning its QR code fills it in."),
        form,
      ),
    ),
  );
  (code ? nameInput : codeInput).focus();
}

// Settings → Remote access

const cache = {
  devices: { data: null, error: null, loading: false, at: 0, seen: 0 },
  pairing: null,
  pairingError: null,
  shownLink: 0,
  tunnelBusy: false,
};

function refreshDevices(view) {
  const d = cache.devices;
  const changed = d.seen !== state.remoteChanges;
  if (d.loading || (!changed && d.at && Date.now() - d.at < 5000)) return;
  d.loading = true;
  d.seen = state.remoteChanges;
  api("/api/devices")
    .then((data) => Object.assign(d, { data: data.devices, error: null }))
    .catch((problem) => Object.assign(d, { error: problem.message }))
    .finally(() => {
      Object.assign(d, { loading: false, at: Date.now() });
      if (view.section === "remote") view.render();
    });
}

function status(on, label) {
  return h("span", { class: `badge ${on ? "on" : "warn"}` }, label);
}

function urlRow(url) {
  return h(
    "div",
    { class: "remote-url" },
    h("span", { class: "muted remote-kind" }, KIND_LABELS[url.kind] ?? url.kind),
    h("a", { href: url.url, target: "_blank", rel: "noopener" }, url.url),
    copyButton(url.url, { icon: icon("copy", 14) }),
  );
}

function command(text) {
  return h("div", { class: "remote-command" }, h("code", {}, text), copyButton(text, { icon: icon("copy", 14) }));
}

/** The Remote access section of the settings page `view`. */
export function remoteSection(view) {
  const remote = state.remote ?? { enabled: false, urls: [], devices: 0 };
  refreshDevices(view);
  const port = (remote.listen ?? "").split(":").pop() || location.port || "7700";
  return [
    view.group(
      "Remote access",
      [
        h(
          "div",
          { class: "remote-status" },
          remote.enabled ? status(true, "Allowed") : status(false, "Off"),
          h(
            "p",
            {},
            remote.enabled
              ? `Paired devices may connect, directly or through a tunnel. Listening on ${remote.listen}.`
              : "Only this machine and SSH tunnels can connect. Other devices are refused before they reach the API.",
            remote.enabled && !view.data?.effective?.web?.remote ? h("span", { class: "muted" }, " This run was started with --remote; the setting below is for every start.") : null,
          ),
        ),
        remote.urls?.length ? h("div", { class: "remote-urls" }, remote.urls.map(urlRow)) : null,
        view.fileNote(),
        view.field(["web", "remote"], {
          type: "toggle",
          label: "Allow other devices",
          help: "Paired devices on other machines may connect. They need their own token from pairing; the master token never works from elsewhere.",
          default: false,
        }),
        view.field(["web", "host"], { label: "Bind address", help: "Unset: 127.0.0.1, or every interface with remote access. 127.0.0.1 keeps a remote daemon off the network, for tunnels only.", placeholder: remote.enabled ? "0.0.0.0" : "127.0.0.1" }),
        view.field(["web", "port"], { type: "int", label: "Port", help: "Any free port when it is taken; 0 picks one.", placeholder: "7700" }),
        view.field(["remote", "public_url"], { label: "Public address", help: "An address in front of this daemon, such as a reverse proxy with TLS. Offered in pairing links.", placeholder: "e.g. https://malac.example.com" }),
        view.field(["remote", "tunnel"], {
          type: "select",
          label: "Tunnel at start",
          help: "Started and stopped with the daemon.",
          unsetLabel: "None",
          options: [
            { value: "cloudflare", label: "Cloudflare quick tunnel" },
            { value: "ngrok", label: "ngrok" },
            { value: "tailscale", label: "Tailscale Serve (tailnet)" },
            { value: "tailscale-funnel", label: "Tailscale Funnel (public)" },
          ],
        }),
        h("p", { class: "field-note" }, icon("info", 14), "These apply the next time the daemon starts (", h("code", {}, "malac daemon restart"), ")."),
      ],
      { description: "Control this daemon from your laptop, phone or another browser." },
    ),
    pairGroup(view, remote),
    devicesGroup(view),
    tunnelGroup(view, remote),
    awakeGroup(view),
    howGroup(remote, port),
  ];
}

function pairGroup(view, remote) {
  const pairing = cache.pairing;
  const create = async () => {
    cache.pairingError = null;
    try {
      cache.pairing = await api("/api/pair/codes", { method: "POST", body: {} });
      cache.shownLink = 0;
    } catch (problem) {
      cache.pairingError = problem.message;
    }
    view.render();
  };
  const expired = pairing && Date.parse(pairing.expires) <= Date.now();
  const children = [];
  if (!pairing || expired) {
    children.push(
      h("p", { class: "muted" }, expired ? "The last code expired." : "A code is valid for ten minutes and pairs one device. Phones can scan its QR code."),
      h("div", { class: "row-actions" }, h("button", { class: "button primary small", type: "button", onclick: create }, icon("key", 14), "Show a pairing code")),
    );
  } else {
    const links = pairing.links ?? [];
    const link = links[Math.min(cache.shownLink, links.length - 1)];
    const left = formatDuration(Date.parse(pairing.expires) - Date.now());
    children.push(
      h(
        "div",
        { class: "pair-box" },
        link?.svg ? h("img", { class: "qr", alt: `QR code for ${link.url}`, src: `data:image/svg+xml;charset=utf-8,${encodeURIComponent(link.svg)}` }) : null,
        h(
          "div",
          { class: "pair-details" },
          h("p", { class: "pair-code" }, pairing.code),
          h("p", { class: "muted" }, `Valid for ${left}, for one device.`),
          links.length > 1
            ? h(
                "div",
                { class: "chips" },
                links.map((l, i) => h("button", { class: "chip", type: "button", "aria-pressed": String(i === cache.shownLink), onclick: () => ((cache.shownLink = i), view.render()) }, KIND_LABELS[l.kind] ?? l.kind)),
              )
            : null,
          link ? h("p", {}, "Scan it or open ", h("a", { href: link.url, target: "_blank", rel: "noopener" }, link.url), " on the device.") : null,
          link ? command(`malac remote add <name> ${link.base} --pair ${pairing.code}`) : null,
          h("div", { class: "row-actions" }, h("button", { class: "button small", type: "button", onclick: create }, icon("refresh", 14), "New code")),
        ),
      ),
    );
    if (!remote.enabled) children.push(h("p", { class: "field-note" }, icon("info", 14), "Remote access is off: only this machine and SSH tunnels can use this code."));
  }
  if (cache.pairingError) children.push(h("p", { class: "field-error" }, cache.pairingError));
  return view.group("Pair a device", children, { description: "Phones, browsers and other computers each pair once and get their own token, which you can revoke." });
}

function devicesGroup(view) {
  const d = cache.devices;
  const revoke = async (device) => {
    const ok = await confirmDialog({
      title: `Revoke ${device.name}?`,
      message: device.current ? "That is this browser: it disconnects at once and has to pair again." : "Its token stops working and its open connections close.",
      confirm: "Revoke",
      danger: true,
    });
    if (!ok) return;
    try {
      await api(`/api/devices/${encodeURIComponent(device.id)}`, { method: "DELETE" });
      d.at = 0;
      if (device.current) local.set(DEVICE_KEY, null);
      view.render();
    } catch (problem) {
      toast(problem.message, "error");
    }
  };
  let body;
  if (d.error) body = h("p", { class: "empty-note error" }, d.error);
  else if (!d.data) body = h("p", { class: "empty-note" }, h("span", { class: "spinner small" }), "Loading devices");
  else if (!d.data.length) body = h("p", { class: "empty-note" }, "No paired devices yet.");
  else
    body = h(
      "table",
      { class: "table devices" },
      h("thead", {}, h("tr", {}, h("th", {}, "Device"), h("th", {}, "Kind"), h("th", {}, "Paired"), h("th", {}, "Last seen"), h("th", {}, "From"), h("th", {}))),
      h(
        "tbody",
        {},
        d.data.map((device) =>
          h(
            "tr",
            {},
            h("td", {}, device.name, device.current ? h("span", { class: "badge on" }, "this browser") : null),
            h("td", { class: "muted" }, device.kind),
            h("td", { class: "muted tabular" }, new Date(device.created).toLocaleDateString()),
            h("td", { class: "muted tabular" }, device.last_seen ? `${formatDuration(Date.now() - Date.parse(device.last_seen))} ago` : "never"),
            h("td", { class: "muted tabular" }, device.address ?? ""),
            h("td", {}, h("button", { class: "button small danger", type: "button", onclick: () => revoke(device) }, "Revoke")),
          ),
        ),
      ),
    );
  return view.group("Paired devices", [body], { description: `${plural(d.data?.length ?? state.remote?.devices ?? 0, "device")}. Revoking one closes its connections at once.` });
}

function tunnelGroup(view, remote) {
  const tunnel = remote.tunnel;
  const running = tunnel && (tunnel.state === "running" || tunnel.state === "starting");
  const start = async (provider) => {
    cache.tunnelBusy = true;
    view.render();
    try {
      await api("/api/tunnel", { method: "POST", body: { provider } });
    } catch (problem) {
      toast(problem.message, "error", { sticky: true });
    }
    cache.tunnelBusy = false;
    view.render();
  };
  const stop = async () => {
    try {
      await api("/api/tunnel", { method: "DELETE" });
    } catch (problem) {
      toast(problem.message, "error");
    }
  };
  const providers = [
    ["cloudflare", "Cloudflare"],
    ["ngrok", "ngrok"],
    ["tailscale", "Tailscale"],
    ["tailscale-funnel", "Tailscale Funnel"],
  ];
  const children = [];
  if (tunnel) {
    children.push(
      h(
        "div",
        { class: "remote-status" },
        status(tunnel.state === "running", tunnel.state),
        h("p", {}, tunnel.provider, tunnel.url ? [" · ", h("a", { href: tunnel.url, target: "_blank", rel: "noopener" }, tunnel.url)] : null),
      ),
    );
    if (tunnel.error) children.push(h("pre", { class: "remote-error" }, tunnel.error));
    for (const note of tunnel.notes ?? []) children.push(h("p", { class: "field-note" }, icon("info", 14), note));
  }
  children.push(
    h(
      "div",
      { class: "row-actions" },
      running
        ? h("button", { class: "button small", type: "button", onclick: stop }, icon("stop", 14), "Stop the tunnel")
        : providers.map(([id, label]) => h("button", { class: "button small", type: "button", disabled: !remote.enabled || cache.tunnelBusy, onclick: () => start(id) }, label)),
    ),
  );
  if (!remote.enabled) children.push(h("p", { class: "field-note" }, icon("info", 14), "Tunnels need remote access: the daemon refuses what comes through one otherwise."));
  return view.group("Tunnel", children, { description: "A public address without opening ports. The tool must be installed; malac runs it and stops it with the daemon." });
}

function awakeGroup(view) {
  const a = state.awake ?? { mode: "busy", awake: false, reason: "" };
  const modes = [
    ["busy", "While working"],
    ["always", "Always"],
    ["off", "Off"],
  ];
  return view.group(
    "Keep awake",
    [
      h(
        "div",
        { class: "remote-status" },
        status(a.awake, a.awake ? "Awake" : "Not holding"),
        h("p", {}, a.error ?? a.reason, a.method ? h("span", { class: "muted" }, ` (${a.method})`) : null),
      ),
      h(
        "div",
        { class: "field" },
        h("div", { class: "field-label" }, h("span", { class: "label" }, "Now"), h("p", { class: "field-help" }, "Until the daemon restarts.")),
        h(
          "div",
          { class: "field-control" },
          h(
            "div",
            { class: "segmented", role: "group", "aria-label": "Keep awake" },
            modes.map(([value, label]) =>
              h("button", { type: "button", "aria-pressed": String(a.mode === value), onclick: () => send({ type: "keep_awake", mode: value }) }, h("span", { class: "segmented-label" }, label)),
            ),
          ),
        ),
      ),
      view.field(["daemon", "keep_awake"], {
        type: "select",
        label: "At start",
        help: "While working holds a power assertion while agents or jobs run and releases it a minute after.",
        options: modes.map(([value, label]) => ({ value, label })),
      }),
      h("p", { class: "field-note" }, icon("info", 14), "Closing a MacBook's lid still puts it to sleep unless it has power, an external display and a keyboard or mouse (clamshell mode). On Linux this uses systemd-inhibit."),
    ],
    { description: "Stops idle and system sleep while agents run, like Amphetamine." },
  );
}

function howGroup(remote, port) {
  const lan = remote.urls?.find((u) => u.kind === "lan");
  const ways = [
    {
      title: "Same network",
      text: remote.enabled ? (lan ? "Other devices on this network use the address below." : "Unset the bind address so the daemon listens on the network.") : "Turn on Allow other devices, then restart the daemon.",
      commands: lan ? [`malac remote add <name> ${lan.url} --pair <code>`] : [],
    },
    {
      title: "SSH tunnel",
      text: "Works even with remote access off, since it looks local. On your laptop:",
      commands: [`ssh -N -L ${port}:127.0.0.1:${port} <this host>`, `malac remote add <name> http://127.0.0.1:${port} --pair <code>`],
    },
    { title: "Cloudflare Tunnel", text: "A public https address, no account needed (cloudflared must be installed).", commands: ["malac tunnel cloudflare"] },
    { title: "ngrok", text: "Needs a free ngrok account and its authtoken.", commands: ["malac tunnel ngrok"] },
    { title: "Tailscale", text: "Private to your tailnet; Funnel makes it public.", commands: ["malac tunnel tailscale", "malac tunnel tailscale-funnel"] },
  ];
  return h(
    "section",
    { class: "settings-group" },
    h("header", { class: "group-head" }, h("div", {}, h("h2", { class: "group-title" }, "How to connect"), h("p", { class: "group-blurb" }, "Then pair: scan the QR code with a phone, open the link in a browser, or run the add command on a computer."))),
    h(
      "div",
      { class: "group-body remote-ways" },
      ways.map((way) => h("div", { class: "remote-way" }, h("h3", {}, way.title), h("p", { class: "muted" }, way.text), way.commands.map(command))),
    ),
  );
}

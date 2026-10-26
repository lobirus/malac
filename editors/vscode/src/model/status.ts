// The status bar summary: agents running, approvals waiting, connection and
// version notices.

import type { ConnectionStatus } from "../connection/client";
import { plural } from "./format";
import type { DaemonStore } from "./store";

export interface StatusSummary {
  text: string;
  tooltip: string[];
  severity: "normal" | "warning" | "error";
}

export function statusSummary(store: DaemonStore, connection: ConnectionStatus, clientVersion: string | undefined): StatusSummary {
  if (connection.state === "failed") {
    return { text: "$(error) malac", tooltip: ["malac cannot connect", connection.detail], severity: "error" };
  }
  if (connection.state === "stopped") {
    return { text: "$(debug-disconnect) malac", tooltip: ["The malac daemon stopped", connection.detail, "Click to open the chat and start it again"], severity: "normal" };
  }
  if (connection.state !== "online" || !store.loaded) {
    const text = connection.state === "offline" ? "$(debug-disconnect) malac" : "$(loading~spin) malac";
    return { text, tooltip: [connection.state === "offline" ? `Reconnecting to malac: ${connection.detail}` : "Connecting to malac"], severity: "normal" };
  }
  const running = store.runningAgents().length;
  const pending = store.pending.size;
  const parts: string[] = [];
  if (running) parts.push(`$(sync~spin) ${running}`);
  if (pending) parts.push(`$(bell-dot) ${pending}`);
  const mismatch = Boolean(store.daemon?.version_mismatch);
  if (mismatch) parts.push("$(warning)");
  const text = parts.length ? `malac ${parts.join(" ")}` : "$(check) malac";
  const tooltip = [
    running ? `${plural(running, "agent")} running` : "No agents running",
    pending ? `${plural(pending, "approval")} waiting` : null,
    mismatch
      ? `The daemon runs malac ${store.daemon?.version}${clientVersion ? `, this malac is ${clientVersion}` : ""}. Restart the daemon to update it.`
      : null,
    `Daemon ${store.daemon?.version ?? "?"} at ${connection.endpoint?.url ?? "?"}`,
    "Click to open the chat",
  ].filter((line): line is string => Boolean(line));
  return { text, tooltip, severity: pending ? "warning" : mismatch ? "warning" : "normal" };
}

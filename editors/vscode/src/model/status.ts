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

/** The pig, from malac's icon font (`contributes.icons`). */
export const PIG = "$(malac-pig)";

/**
 * Reads like Claude Code's "✻ Claude Code": the pig and "malac" when idle;
 * when agents work or wait, the pig and their counts.
 */
export function statusSummary(store: DaemonStore, connection: ConnectionStatus, clientVersion: string | undefined): StatusSummary {
  if (connection.state === "failed") {
    return { text: `${PIG} malac $(error)`, tooltip: ["malac cannot connect", connection.detail], severity: "error" };
  }
  if (connection.state === "stopped") {
    return { text: `${PIG} malac $(debug-disconnect)`, tooltip: ["The malac daemon is not running", connection.detail, "Click to open malac and start it"], severity: "normal" };
  }
  if (connection.state !== "online" || !store.loaded) {
    const text = connection.state === "offline" ? `${PIG} malac $(debug-disconnect)` : `${PIG} malac $(loading~spin)`;
    return { text, tooltip: [connection.state === "offline" ? `Reconnecting to malac: ${connection.detail}` : "Connecting to malac"], severity: "normal" };
  }
  const running = store.runningAgents().length;
  const pending = store.pending.size;
  const parts: string[] = [];
  if (running) parts.push(`$(sync~spin) ${running}`);
  if (pending) parts.push(`$(bell-dot) ${pending}`);
  const mismatch = Boolean(store.daemon?.version_mismatch);
  const text = `${PIG} ${parts.length ? parts.join(" ") : "malac"}${mismatch ? " $(warning)" : ""}`;
  const tooltip = [
    running ? `${plural(running, "agent")} running` : "No agents running",
    pending ? `${plural(pending, "approval")} waiting` : null,
    mismatch
      ? `The daemon runs malac ${store.daemon?.version}${clientVersion ? `, this malac is ${clientVersion}` : ""}. Restart the daemon to update it.`
      : null,
    `Daemon ${store.daemon?.version ?? "?"} at ${connection.endpoint?.url ?? "?"}`,
    "Click to open malac",
  ].filter((line): line is string => Boolean(line));
  return { text, tooltip, severity: pending ? "warning" : mismatch ? "warning" : "normal" };
}

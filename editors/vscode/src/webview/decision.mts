// What the user answered when a tool call waited for approval, and where:
// "Allowed · Pixel 9", "Allowed for the session · this computer", "Denied ·
// talk mode". Live from `permission_resolved` (`decision`, `by`), and for a
// saved transcript from its `decisions`, matched to tool cards by `call_id`.
// The terminal UI words it the same way (`decision_text`).

import type { Decision } from "../protocol.js";

export interface DecisionInfo {
  decision: Decision;
  /** Where it was answered; null when nobody did (a cancelled turn). */
  by: string | null;
}

const VERBS: Record<Decision, string> = {
  allow: "Allowed",
  allow_session: "Allowed for the session",
  deny: "Denied",
};

export function decisionText(decision: Decision, by?: string | null): string {
  const verb = VERBS[decision];
  const where = typeof by === "string" ? by.trim() : "";
  return where ? `${verb} · ${where}` : verb;
}

/** The answer an event or a saved record carries; null when it has none (older daemons). */
export function decisionOf(source: unknown): DecisionInfo | null {
  if (!source || typeof source !== "object") return null;
  const { decision, by } = source as { decision?: unknown; by?: unknown };
  if (typeof decision !== "string" || !(decision in VERBS)) return null;
  return { decision: decision as Decision, by: typeof by === "string" && by.trim() ? by.trim() : null };
}

/** A saved session's `decisions`, by the tool call they answered; a later answer to the same call wins. */
export function decisionsByCall(records: unknown): Map<string, DecisionInfo> {
  const map = new Map<string, DecisionInfo>();
  if (!Array.isArray(records)) return map;
  for (const record of records) {
    const info = decisionOf(record);
    const call = (record as { call_id?: unknown })?.call_id;
    if (info && typeof call === "string" && call) map.set(call, info);
  }
  return map;
}

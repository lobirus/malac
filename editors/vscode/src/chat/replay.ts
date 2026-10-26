// Joining a saved transcript with the live events buffered since connecting.
//
// The history (`GET /api/sessions/{id}`) holds every message written so far.
// Events that arrived after the request started are applied on top. Events
// from before it are mostly in the history already, except those that never
// reach the transcript file (deltas, tool progress, approvals) since the last
// persisted message: those are replayed too, so a turn that is under way shows
// its streaming text and running tools.

import type { Envelope, Message } from "../protocol";

const PERSISTED = new Set(["user_message", "assistant_message"]);
const TRANSIENT = new Set([
  "text_delta",
  "thinking_delta",
  "tool_call_streaming",
  "tool_started",
  "tool_finished",
  "permission_requested",
  "permission_resolved",
  "todos",
  "notice",
  "usage",
]);

export function replayEvents(events: { n: number; env: Envelope }[], cutoff: number, history: Message[]): Envelope[] {
  let start = 0;
  for (let i = events.length - 1; i >= 0; i -= 1) {
    if (events[i].n <= cutoff && PERSISTED.has(events[i].env.type)) {
      start = i + 1;
      break;
    }
  }
  const out: Envelope[] = [];
  // A turn under way started before the cutoff: tell the view when, so its
  // footer can show the duration and usage.
  let turnStart = -1;
  for (let i = 0; i < events.length && events[i].n <= cutoff; i += 1) {
    if (events[i].env.type === "user_message") turnStart = i;
    else if (events[i].env.type === "turn_finished") turnStart = -1;
  }
  if (turnStart >= 0) out.push({ ...events[turnStart].env, type: "turn_resumed", text: undefined });
  for (const { n, env } of events.slice(start)) {
    if (n > cutoff || TRANSIENT.has(env.type)) out.push(env);
  }
  return dropRecorded(out, history);
}

/**
 * Messages written while the history was being read can arrive twice: in the
 * history and as events (before or just after the transcript went out). The
 * filter drops persisted events at the front that match the history's last
 * messages; a dropped user message becomes `turn_resumed`, so the turn's
 * footer still knows when it started.
 */
export class RecordedFilter {
  private leading = true;
  private readonly until: number;

  constructor(
    private readonly tail: Message[],
    windowMs = 5000,
    now = Date.now(),
  ) {
    this.until = now + windowMs;
  }

  pass(env: Envelope, now = Date.now()): Envelope | null {
    if (!this.leading || now > this.until) return env;
    if (env.type === "turn_finished") this.leading = false;
    if (!PERSISTED.has(env.type)) return env;
    if (isRecorded(env, this.tail)) return env.type === "user_message" ? { ...env, type: "turn_resumed", text: undefined } : null;
    this.leading = false;
    return env;
  }
}

function dropRecorded(events: Envelope[], history: Message[]): Envelope[] {
  const filter = new RecordedFilter(history.slice(-4), Number.POSITIVE_INFINITY);
  return events.map((env) => filter.pass(env)).filter((env): env is Envelope => env !== null);
}

function isRecorded(env: Envelope, tail: Message[]): boolean {
  if (env.type === "assistant_message") {
    const content = JSON.stringify((env.message as Message | undefined)?.content ?? null);
    return tail.some((m) => m.role === "assistant" && JSON.stringify(m.content) === content);
  }
  const typed = String(env.text ?? "").replace(/\n?\[attached: [^\]]*\]\s*$/, "").trim();
  if (!typed) return false;
  return tail.some(
    (m) =>
      m.role === "user" &&
      m.content.some((b) => b.type === "text" && !b.text.startsWith("<attachment ") && b.text.trim() === typed),
  );
}

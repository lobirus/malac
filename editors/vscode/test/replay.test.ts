import assert from "node:assert/strict";
import { test } from "node:test";
import { RecordedFilter, replayEvents } from "../src/chat/replay";
import type { Envelope, Message } from "../src/protocol";

const env = (type: string, fields: Record<string, unknown> = {}): Envelope => ({ agent: "a", seq: 0, at: "2026-10-26T08:00:00Z", type, ...fields });
const numbered = (events: Envelope[]) => events.map((e, i) => ({ n: i + 1, env: e }));

test("events after the cutoff are applied on top of the history", () => {
  const events = numbered([env("user_message", { text: "hi" }), env("assistant_message", { message: { role: "assistant", content: [] } }), env("turn_finished"), env("user_message", { text: "next" })]);
  const out = replayEvents(events, 3, []);
  assert.deepEqual(out.map((e) => e.type), ["user_message"]);
});

test("a turn under way replays its streaming and tools, and when it started", () => {
  const events = numbered([
    env("user_message", { text: "fix it" }),
    env("thinking_delta", { text: "hmm" }),
    env("tool_started", { call_id: "c1" }),
    env("permission_requested", { request_id: "r1", call_id: "c1" }),
    env("text_delta", { text: "Wor" }),
  ]);
  const history: Message[] = [{ role: "user", content: [{ type: "text", text: "fix it" }] }];
  const out = replayEvents(events, 5, history);
  assert.deepEqual(out.map((e) => e.type), ["turn_resumed", "thinking_delta", "tool_started", "permission_requested", "text_delta"]);
});

test("messages both in the history and in the events show once", () => {
  const assistant = { role: "assistant" as const, content: [{ type: "text" as const, text: "done" }] };
  const history: Message[] = [{ role: "user", content: [{ type: "text", text: "go" }] }, assistant];
  const events = numbered([env("user_message", { text: "go\n[attached: a.txt]" }), env("assistant_message", { message: assistant }), env("turn_finished")]);
  const out = replayEvents(events, 0, history);
  assert.deepEqual(out.map((e) => e.type), ["turn_resumed", "turn_finished"]);
});

test("the live filter drops repeats only right after the transcript", () => {
  const history: Message[] = [{ role: "user", content: [{ type: "text", text: "<attachment path=\"x.png\"></attachment>" }, { type: "text", text: "look" }] }];
  const filter = new RecordedFilter(history, 5000, 1000);
  assert.equal(filter.pass(env("user_message", { text: "look\n[attached: x.png]" }), 1001)?.type, "turn_resumed");
  assert.equal(filter.pass(env("text_delta", { text: "ok" }), 1002)?.type, "text_delta");
  const fresh = filter.pass(env("user_message", { text: "something new" }), 1003);
  assert.equal(fresh?.type, "user_message");
  assert.equal(filter.pass(env("user_message", { text: "look" }), 1004)?.type, "user_message", "after a new message nothing is dropped");
  const late = new RecordedFilter(history, 5000, 1000);
  assert.equal(late.pass(env("user_message", { text: "look" }), 7000)?.type, "user_message", "the window closes");
});

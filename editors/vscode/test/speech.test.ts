import assert from "node:assert/strict";
import { test } from "node:test";
import type { SayMessage } from "../src/protocol";
import { Speaker, SpeakerHost } from "../src/voice/speech";

function setup() {
  const requests: Record<string, unknown>[] = [];
  const played: string[] = [];
  const shown: string[] = [];
  const failures: string[] = [];
  let nextListener = 7;
  let release: (() => void) | null = null;
  const host: SpeakerHost = {
    async request(command) {
      requests.push(command);
      return { type: "voice_listening", listener: command.on ? nextListener++ : null };
    },
    async download(id) {
      if (id === "missing") throw new Error("404 not found");
      return { data: Buffer.from(`audio ${id}`), type: "audio/mpeg" };
    },
    play({ id }, signal) {
      played.push(`start ${id}`);
      return new Promise<void>((resolve) => {
        release = () => {
          played.push(`end ${id}`);
          resolve();
        };
        signal.addEventListener("abort", () => {
          played.push(`stopped ${id}`);
          release = null;
          resolve();
        });
      });
    },
    show: (text) => void shown.push(text),
    failed: (error) => void failures.push(error.message),
    log: () => {},
  };
  const finishPlaying = async () => {
    for (let i = 0; i < 20 && !release; i += 1) await new Promise((r) => setTimeout(r, 5));
    const done = release;
    release = null;
    done?.();
    await new Promise((r) => setTimeout(r, 5));
  };
  return { speaker: new Speaker(host), requests, played, shown, failures, finishPlaying };
}

const say = (id: string, listener: number): SayMessage => ({ type: "say", id, text: `sentence ${id}`, reason: "agent", tab: null, agent: null, listener });

test("the connection listens when it should, again after reconnecting, and stops when switched off", async () => {
  const { speaker, requests } = setup();
  await speaker.connected(false);
  assert.equal(requests.length, 0, "nothing to say while speech is off");
  await speaker.want(true);
  assert.equal(speaker.listener, 7);
  await speaker.want(true);
  assert.equal(requests.length, 1, "already listening");
  speaker.disconnected();
  assert.equal(speaker.listener, null);
  await speaker.connected(true);
  assert.equal(speaker.listener, 8, "a new connection gets a new listener id");
  await speaker.want(false);
  assert.equal(speaker.listener, null);
  assert.deepEqual(requests.map((r) => r.on), [true, true, false]);
});

test("only sentences for this listener play, one after another", async () => {
  const { speaker, played, shown, failures, finishPlaying } = setup();
  assert.equal(speaker.hear(say("early", 7)), false, "not listening yet");
  await speaker.connected(true);
  assert.equal(speaker.hear(say("other", 99)), false, "for another device");
  assert.equal(speaker.hear(say("one", 7)), true);
  assert.equal(speaker.hear(say("missing", 7)), true);
  assert.equal(speaker.hear(say("two", 7)), true);
  await new Promise((r) => setTimeout(r, 30));
  assert.deepEqual(played, ["start one"], "the others wait for the first");
  await finishPlaying();
  assert.deepEqual(played, ["start one", "end one", "start two"]);
  await finishPlaying();
  await speaker.idle();
  assert.deepEqual(played, ["start one", "end one", "start two", "end two"]);
  assert.deepEqual(shown, ["sentence one", "sentence two"]);
  assert.deepEqual(failures, ["404 not found"], "a sentence that cannot be fetched is skipped");
});

test("cancelled sentences stop at once and queued ones never play", async () => {
  const { speaker, played } = setup();
  await speaker.connected(true);
  const listener = speaker.listener ?? -1;
  for (const id of ["a", "b", "c"]) speaker.hear(say(id, listener));
  for (let i = 0; i < 40 && !played.includes("start a"); i += 1) await new Promise((r) => setTimeout(r, 5));
  speaker.cancel(["a", "b"]);
  for (let i = 0; i < 40 && !played.includes("start c"); i += 1) await new Promise((r) => setTimeout(r, 5));
  assert.deepEqual(played, ["start a", "stopped a", "start c"]);
});

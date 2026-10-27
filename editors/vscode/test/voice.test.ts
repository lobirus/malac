import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { after, test } from "node:test";
import { Dictation, hasVoiceCommands, parseDictation, play, voiceArgs, VoiceSupport } from "../src/voice/cli";
import { DictationManager, DictationUpdate } from "../src/voice/dictation";

// A stand-in for the malac CLI: `--help` lists the voice commands; `voice
// dictate --json` waits for a line on stdin (or answers at once, per
// STUB_DICTATE), and logs its arguments to STUB_LOG.
const STUB = `#!/usr/bin/env node
const fs = require("fs");
const args = process.argv.slice(2);
if (process.env.STUB_LOG) fs.appendFileSync(process.env.STUB_LOG, JSON.stringify({ args, home: process.env.MALAC_HOME ?? null }) + "\\n");
if (args[0] === "--help") {
  console.log(process.env.STUB_OLD ? "Commands:\\n  web  Web UI\\n\\nArguments:\\n  [PROMPT]...  voice too" : "Commands:\\n  web    Web UI\\n  voice  Dictate, speak and talk\\n\\nArguments:\\n  [PROMPT]...");
  process.exit(0);
}
if (args[0] === "voice" && args[1] === "play") {
  if (!fs.existsSync(args[2])) { console.error("error: cannot open " + args[2]); process.exit(1); }
  process.exit(0);
}
if (args[0] === "voice" && args[1] === "dictate") {
  const mode = process.env.STUB_DICTATE ?? "wait";
  if (mode === "error") { console.log(JSON.stringify({ error: "Microphone access is denied. Allow it in System Settings > Privacy & Security > Microphone." })); process.exit(1); }
  if (mode === "crash") { console.error("error: no input device"); process.exit(3); }
  process.stdin.once("data", () => { console.log(JSON.stringify({ text: "hello from the stub" })); process.exit(0); });
  process.stdin.once("end", () => { console.log(JSON.stringify({ text: "ended by eof" })); process.exit(0); });
  process.on("SIGTERM", () => process.exit(143));
  setInterval(() => {}, 1000);
  return;
}
process.exit(2);
`;

const made: string[] = [];
after(() => made.forEach((dir) => fs.rmSync(dir, { recursive: true, force: true })));

function stub(env: Record<string, string> = {}): { command: string; log: string; dir: string } {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "malac-voice-"));
  made.push(dir);
  const command = path.join(dir, "malac");
  const log = path.join(dir, "calls.log");
  // A wrapper sets the environment, so each test has its own behaviour.
  fs.writeFileSync(path.join(dir, "stub.js"), STUB);
  const exports = Object.entries({ STUB_LOG: log, ...env })
    .map(([k, v]) => `export ${k}='${v}'`)
    .join("\n");
  fs.writeFileSync(command, `#!/bin/sh\n${exports}\nexec "${process.execPath}" "${path.join(dir, "stub.js")}" "$@"\n`, { mode: 0o755 });
  return { command, log, dir };
}

const calls = (log: string) => (fs.existsSync(log) ? fs.readFileSync(log, "utf8").trim().split("\n").map((l) => JSON.parse(l) as { args: string[]; home: string | null }) : []);

test("voice commands are found in malac --help, not in the prompt's description", async () => {
  assert.ok(hasVoiceCommands("Commands:\n  web    x\n  voice  Dictate\n\nArguments:\n  [PROMPT]"));
  assert.ok(!hasVoiceCommands("Commands:\n  web    x\n\nArguments:\n  voice notes"));
  const support = new VoiceSupport();
  const fresh = stub();
  assert.deepEqual(await support.check(fresh.command), { ok: true });
  assert.deepEqual(support.known(fresh.command), { ok: true });
  const old = stub({ STUB_OLD: "1" });
  const answer = await support.check(old.command);
  assert.ok(!answer.ok && /no voice commands/.test(answer.reason));
  const missing = await support.check(path.join(fresh.dir, "nope"));
  assert.ok(!missing.ok && missing.missing && /install malac/i.test(missing.reason));
  assert.deepEqual(calls(old.log).map((c) => c.args), [["--help"]], "an old malac only ever gets --help");
});

test("a saved connection is passed as -C after the voice command", () => {
  assert.deepEqual(voiceArgs({ command: "malac", home: undefined, connect: "studio" }, ["talk", "--tab", "t1"]), ["voice", "talk", "--tab", "t1", "-C", "studio"]);
  assert.deepEqual(voiceArgs({ command: "malac", home: undefined, connect: null }, ["play", "/tmp/a.mp3"]), ["voice", "play", "/tmp/a.mp3"]);
});

test("the dictation answer is the last JSON line", () => {
  assert.deepEqual(parseDictation('recording…\n{"text": "hi there"}\n'), { text: "hi there" });
  assert.deepEqual(parseDictation('{"error": "no model"}'), { error: "no model" });
  assert.equal(parseDictation("Ctrl-C"), null);
});

test("dictation stops on a newline and returns the text", async () => {
  const { command, log, dir } = stub();
  const dictation = new Dictation({ command, home: dir, connect: "studio" }, { maxSeconds: 30 });
  await new Promise((r) => setTimeout(r, 300));
  dictation.stop();
  assert.deepEqual(await dictation.result, { text: "hello from the stub" });
  assert.deepEqual(calls(log)[0], { args: ["voice", "dictate", "--json", "--max-seconds", "30", "-C", "studio"], home: dir });
});

test("cancelling kills the recording and drops it", async () => {
  const { command } = stub();
  const dictation = new Dictation({ command, home: undefined, connect: null });
  await new Promise((r) => setTimeout(r, 300));
  dictation.cancel();
  assert.deepEqual(await dictation.result, { cancelled: true });
});

test("errors from the CLI say what to do", async () => {
  const denied = new Dictation({ command: stub({ STUB_DICTATE: "error" }).command, home: undefined, connect: null });
  const result = await denied.result;
  assert.ok("error" in result && /Microphone access is denied/.test(result.error));
  const crashed = await new Dictation({ command: stub({ STUB_DICTATE: "crash" }).command, home: undefined, connect: null }).result;
  assert.deepEqual(crashed, { error: "No input device" });
  const missing = await new Dictation({ command: "/nonexistent/malac", home: undefined, connect: null }).result;
  assert.ok("error" in missing && /malac\.path/.test(missing.error));
});

test("play runs malac voice play and reports its error", async () => {
  const { command, log, dir } = stub();
  const file = path.join(dir, "say.mp3");
  fs.writeFileSync(file, "ID3");
  await play({ command, home: undefined, connect: null }, file);
  assert.deepEqual(calls(log).at(-1)!.args, ["voice", "play", file]);
  await assert.rejects(play({ command, home: undefined, connect: null }, path.join(dir, "gone.mp3")), /Cannot open/);
});

test("one recording per window: a second chat is told, the owner gets its text", async () => {
  const { command } = stub();
  const manager = new DictationManager(async () => new Dictation({ command, home: undefined, connect: null }));
  const updates: (DictationUpdate & { who: string })[] = [];
  const first = {};
  const second = {};
  manager.onUpdate.event((u) => updates.push({ ...u, who: u.owner === first ? "first" : "second" }));
  await Promise.race([manager.toggle(first), new Promise((r) => setTimeout(r, 200))]);
  assert.equal(manager.owner, first);
  void manager.begin(second);
  assert.equal(updates.at(-1)!.who, "second");
  assert.match(updates.at(-1)!.error ?? "", /already on in another malac chat/);
  await new Promise((r) => setTimeout(r, 200));
  manager.stop();
  for (let i = 0; i < 50 && updates.at(-1)!.state !== "done"; i += 1) await new Promise((r) => setTimeout(r, 20));
  assert.deepEqual(
    updates.filter((u) => u.who === "first").map((u) => [u.state, u.text]),
    [
      ["recording", undefined],
      ["transcribing", undefined],
      ["done", "hello from the stub"],
    ],
  );
  assert.equal(manager.owner, null);
});

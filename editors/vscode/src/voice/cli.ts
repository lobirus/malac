// The malac CLI's voice commands, which record and play audio on this
// machine for the extension: VS Code webviews may not use the microphone
// (their frames' permissions policy leaves it out), so dictation runs
// `malac voice dictate --json`, speech `malac voice play <file>` and talk
// mode `malac voice talk` in a terminal. A saved connection is passed as
// `-C <name>`; without it they use this machine's daemon. Free of the
// vscode module.

import { ChildProcessWithoutNullStreams, execFile, spawn } from "node:child_process";
import { MissingBinaryError } from "../connection/launcher";

/** Where the voice commands run and which daemon they use. */
export interface VoiceTarget {
  /** The malac binary. */
  command: string;
  /** `MALAC_HOME` for the child, when the extension uses a custom home. */
  home: string | undefined;
  /** A saved connection's name; null is this machine's daemon. */
  connect: string | null;
}

/** Recordings stop on their own after this long. */
export const MAX_DICTATION_SECONDS = 300;
const PLAY_TIMEOUT_MS = 5 * 60_000;
const HELP_TIMEOUT_MS = 5000;
const KILL_AFTER_MS = 3000;

export function voiceArgs(target: VoiceTarget, args: string[]): string[] {
  return ["voice", ...args, ...(target.connect ? ["-C", target.connect] : [])];
}

export function voiceEnv(target: VoiceTarget): NodeJS.ProcessEnv {
  return target.home ? { ...process.env, MALAC_HOME: target.home } : process.env;
}

/**
 * Whether this malac has the voice commands. Older ones take unknown words
 * as a prompt (and `--json` as their own flag), so nothing runs before
 * `malac --help` lists `voice`.
 */
export function hasVoiceCommands(help: string): boolean {
  return /^\s+voice\s/m.test(help.split(/^Arguments:/m)[0] ?? "");
}

export type Support = { ok: true } | { ok: false; reason: string; missing?: boolean };

/** Asks `malac --help` once per binary (again with `again`, after an update say). */
export class VoiceSupport {
  private checks = new Map<string, Promise<Support>>();
  private answers = new Map<string, Support>();

  check(command: string, { again = false } = {}): Promise<Support> {
    let check = again ? undefined : this.checks.get(command);
    if (!check) {
      check = readHelp(command).then(
        (help): Support =>
          hasVoiceCommands(help) ? { ok: true } : { ok: false, reason: `This malac (${command}) has no voice commands. Update malac (\`malac update\`) to use voice here.` },
        (error: Error): Support =>
          error instanceof MissingBinaryError
            ? { ok: false, missing: true, reason: "Voice needs the malac command on this computer. Install malac, or point the malac.path setting at it." }
            : { ok: false, reason: `Cannot run ${command}: ${error.message}` },
      );
      this.checks.set(command, check);
      void check.then((support) => this.answers.set(command, support));
    }
    return check;
  }

  /** The answer for `command` once a check finished, without waiting. */
  known(command: string): Support | undefined {
    return this.answers.get(command);
  }
}

function readHelp(command: string): Promise<string> {
  return new Promise((resolve, reject) => {
    execFile(command, ["--help"], { timeout: HELP_TIMEOUT_MS }, (error, stdout) => {
      if (error && (error as NodeJS.ErrnoException).code === "ENOENT") reject(new MissingBinaryError(command));
      else if (error && !stdout) reject(error);
      else resolve(String(stdout));
    });
  });
}

/** What to tell the user about a voice command that failed. */
export function failure(command: string, args: string[], code: number | null, stderr: string): Error {
  const lines = stderr
    .split("\n")
    .map((line) => line.replace(/^(error|✗|!)\s*:?\s*/i, "").trim())
    .filter(Boolean);
  const last = lines.at(-1);
  return new Error(last ? last.charAt(0).toUpperCase() + last.slice(1) : `${command} ${args.join(" ")} exited with code ${code}`);
}

/** Plays a WAV or MP3 file through `malac voice play`; resolves when it is done or `signal` stopped it. */
export function play(target: VoiceTarget, file: string, signal?: AbortSignal): Promise<void> {
  const args = voiceArgs(target, ["play", file]);
  return new Promise((resolve, reject) => {
    execFile(target.command, args, { timeout: PLAY_TIMEOUT_MS, env: voiceEnv(target), signal }, (error, _stdout, stderr) => {
      if (!error || signal?.aborted) resolve();
      else if ((error as NodeJS.ErrnoException).code === "ENOENT") reject(new MissingBinaryError(target.command));
      else reject(failure(target.command, args, typeof error.code === "number" ? error.code : null, String(stderr)));
    });
  });
}

export type DictationResult = { text: string } | { error: string } | { cancelled: true };

/** The last line of `malac voice dictate --json` that is its answer. */
export function parseDictation(stdout: string): { text: string } | { error: string } | null {
  for (const line of stdout.split("\n").reverse()) {
    if (!line.trim().startsWith("{")) continue;
    try {
      const value = JSON.parse(line) as { text?: unknown; error?: unknown };
      if (typeof value.error === "string") return { error: value.error };
      if (typeof value.text === "string") return { text: value.text };
    } catch {
      // Not the answer.
    }
  }
  return null;
}

type Spawn = (command: string, args: string[], options: { env: NodeJS.ProcessEnv; stdio: ["pipe", "pipe", "pipe"] }) => ChildProcessWithoutNullStreams;

/**
 * One recording: `malac voice dictate --json` records until a line on its
 * stdin (or EOF), then prints `{"text"}` or `{"error"}`. `stop` ends the
 * recording, `cancel` kills it and drops whatever it heard.
 */
export class Dictation {
  readonly started = Date.now();
  readonly result: Promise<DictationResult>;
  /** Set once the recording ended and the text is on its way. */
  stopping = false;
  private cancelled = false;
  private readonly child: ChildProcessWithoutNullStreams;

  constructor(
    target: VoiceTarget,
    { maxSeconds = MAX_DICTATION_SECONDS, log = () => {}, spawner = spawn as unknown as Spawn }: { maxSeconds?: number; log?: (line: string) => void; spawner?: Spawn } = {},
  ) {
    const args = voiceArgs(target, ["dictate", "--json", "--max-seconds", String(maxSeconds)]);
    log(`> ${target.command} ${args.join(" ")}`);
    this.child = spawner(target.command, args, { env: voiceEnv(target), stdio: ["pipe", "pipe", "pipe"] });
    let stdout = "";
    let stderr = "";
    this.child.stdout.on("data", (chunk: Buffer) => (stdout += chunk.toString()));
    this.child.stderr.on("data", (chunk: Buffer) => {
      stderr += chunk.toString();
      log(chunk.toString().trimEnd());
    });
    // A child that already exited makes writes to stdin fail; that is fine.
    this.child.stdin.on("error", () => {});
    this.result = new Promise((resolve) => {
      this.child.once("error", (error: NodeJS.ErrnoException) => {
        resolve(this.cancelled ? { cancelled: true } : { error: error.code === "ENOENT" ? new MissingBinaryError(target.command).message : error.message });
      });
      this.child.once("close", (code) => {
        if (this.cancelled) return resolve({ cancelled: true });
        const answer = parseDictation(stdout);
        if (answer) return resolve(answer);
        resolve({ error: failure(target.command, args, code, stderr).message });
      });
    });
  }

  /** Ends the recording; the text follows in `result`. */
  stop(): void {
    if (this.stopping || this.cancelled) return;
    this.stopping = true;
    this.child.stdin.write("\n");
    this.child.stdin.end();
  }

  /** Drops the recording. */
  cancel(): void {
    if (this.cancelled) return;
    this.cancelled = true;
    this.child.kill("SIGTERM");
    const timer = setTimeout(() => this.child.kill("SIGKILL"), KILL_AFTER_MS);
    timer.unref?.();
    void this.result.finally(() => clearTimeout(timer));
  }
}

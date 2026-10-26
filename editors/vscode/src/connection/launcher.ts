// Starts the daemon the way the extension always has: `malac web --port 0
// --json-ready --cwd <folder>`. That makes sure the daemon runs, adds or
// activates a tab for the folder, prints one JSON line with the address and
// stays attached as a client (which keeps the daemon from idling out).

import { ChildProcessByStdio, execFile, spawn } from "node:child_process";
import * as readline from "node:readline";
import type { Readable } from "node:stream";

const READY_TIMEOUT_MS = 30_000;

export interface LaunchConfig {
  command: string;
  extraArgs: string[];
  /** The tab directory to pass as `--cwd`, if any. */
  cwd: string | undefined;
  /** `MALAC_HOME` for the child, when the extension uses a custom home. */
  home: string | undefined;
}

export interface Log {
  appendLine(line: string): void;
  append(text: string): void;
}

export class MissingBinaryError extends Error {
  constructor(readonly command: string) {
    super(`Could not run "${command}". Install malac or point the malac.path setting at the binary.`);
    this.name = "MissingBinaryError";
  }
}

type ServerProcess = ChildProcessByStdio<null, Readable, Readable>;

/** One `malac web` process per window, started on demand. */
export class Launcher {
  private child: ServerProcess | undefined;
  private address: Promise<{ url: string; token: string }> | undefined;

  constructor(
    private readonly config: () => LaunchConfig,
    private readonly log: Log,
  ) {}

  start(): Promise<{ url: string; token: string }> {
    this.address ??= this.launch();
    return this.address;
  }

  /** Whether this window started the process that is still attached. */
  get attached(): boolean {
    return this.child !== undefined;
  }

  dispose(): void {
    this.child?.kill();
    this.forget();
  }

  private launch(): Promise<{ url: string; token: string }> {
    const { command, extraArgs, cwd, home } = this.config();
    const args = ["web", "--port", "0", "--json-ready", ...(cwd ? ["--cwd", cwd] : []), ...extraArgs];
    this.log.appendLine(`> ${command} ${args.join(" ")}`);
    const child = spawn(command, args, {
      cwd: cwd ?? process.cwd(),
      stdio: ["ignore", "pipe", "pipe"],
      env: home ? { ...process.env, MALAC_HOME: home } : process.env,
    });
    this.child = child;
    child.stderr.on("data", (chunk: Buffer) => this.log.append(chunk.toString()));
    child.on("exit", (code, signal) => {
      this.log.appendLine(`malac web exited with ${signal ?? `code ${code}`}`);
      if (this.child === child) this.forget();
    });
    const address = readAddress(child, command, this.log);
    address.catch(() => {
      if (this.child !== child) return;
      child.kill();
      this.forget();
    });
    return address;
  }

  private forget(): void {
    this.child = undefined;
    this.address = undefined;
  }
}

/** Resolves with the JSON line `malac web --json-ready` prints once it listens. */
function readAddress(child: ServerProcess, command: string, log: Log): Promise<{ url: string; token: string }> {
  return new Promise((resolve, reject) => {
    const fail = (error: Error) => {
      clearTimeout(timer);
      reject(error);
    };
    const timer = setTimeout(() => fail(new Error("malac did not report its address in time")), READY_TIMEOUT_MS);
    child.once("error", (error: NodeJS.ErrnoException) => {
      fail(error.code === "ENOENT" ? new MissingBinaryError(command) : error);
    });
    child.once("exit", (code) => fail(new Error(`malac exited with code ${code} before it was ready`)));
    let ready = false;
    readline.createInterface({ input: child.stdout }).on("line", (line) => {
      if (ready) {
        log.appendLine(line);
        return;
      }
      ready = true;
      clearTimeout(timer);
      try {
        resolve(parseAddress(line));
      } catch (error) {
        reject(error);
      }
    });
  });
}

export function parseAddress(line: string): { url: string; token: string } {
  const value = JSON.parse(line) as { url?: unknown; token?: unknown } | null;
  if (typeof value?.url !== "string" || typeof value.token !== "string") {
    throw new Error(`unexpected output from malac: ${line}`);
  }
  return { url: value.url, token: value.token };
}

/** Runs the malac binary once and returns its stdout (e.g. `--version`). */
export function runMalac(command: string, args: string[], home: string | undefined, timeoutMs = 60_000): Promise<string> {
  return new Promise((resolve, reject) => {
    execFile(
      command,
      args,
      { timeout: timeoutMs, env: home ? { ...process.env, MALAC_HOME: home } : process.env },
      (error, stdout, stderr) => {
        if (error) {
          const code = (error as NodeJS.ErrnoException).code;
          reject(code === "ENOENT" ? new MissingBinaryError(command) : new Error(String(stderr || stdout || error.message).trim()));
        } else {
          resolve(String(stdout));
        }
      },
    );
  });
}

/** `malac 1.0.1` → `1.0.1`. */
export function parseVersion(output: string): string | null {
  return /(\d+\.\d+\.\d+(?:[-+][\w.]+)?)/.exec(output)?.[1] ?? null;
}

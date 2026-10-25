import { ChildProcessByStdio, spawn } from "node:child_process";
import * as readline from "node:readline";
import type { Readable } from "node:stream";
import * as vscode from "vscode";

const READY_TIMEOUT_MS = 30_000;

export interface ServerAddress {
  url: string;
  token: string;
}

export class MissingBinaryError extends Error {
  constructor(readonly command: string) {
    super(`Could not run "${command}". Install malac or point the malac.path setting at the binary.`);
  }
}

type ServerProcess = ChildProcessByStdio<null, Readable, Readable>;

/** One `malac web` process per window, started on first use. */
export class MalacServer implements vscode.Disposable {
  private child: ServerProcess | undefined;
  private address: Promise<ServerAddress> | undefined;

  constructor(private readonly output: vscode.OutputChannel) {}

  start(cwd: string): Promise<ServerAddress> {
    this.address ??= this.launch(cwd);
    return this.address;
  }

  dispose(): void {
    this.child?.kill();
    this.forget();
  }

  private launch(cwd: string): Promise<ServerAddress> {
    const config = vscode.workspace.getConfiguration("malac");
    const command = config.get<string>("path", "malac");
    const args = ["web", "--port", "0", "--json-ready", "--cwd", cwd, ...config.get<string[]>("extraArgs", [])];
    this.output.appendLine(`> ${command} ${args.join(" ")}`);

    const child = spawn(command, args, { cwd, stdio: ["ignore", "pipe", "pipe"] });
    this.child = child;
    child.stderr.on("data", (chunk: Buffer) => this.output.append(chunk.toString()));
    child.on("exit", (code, signal) => {
      this.output.appendLine(`malac exited with ${signal ?? `code ${code}`}`);
      if (this.child === child) this.forget();
    });

    const address = readAddress(child, command, this.output);
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
function readAddress(child: ServerProcess, command: string, output: vscode.OutputChannel): Promise<ServerAddress> {
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
        output.appendLine(line);
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

function parseAddress(line: string): ServerAddress {
  const value = JSON.parse(line) as Partial<ServerAddress> | null;
  if (typeof value?.url !== "string" || typeof value.token !== "string") {
    throw new Error(`unexpected output from malac: ${line}`);
  }
  return { url: value.url, token: value.token };
}

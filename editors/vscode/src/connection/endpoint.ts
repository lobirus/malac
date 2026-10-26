// Where the daemon is. `local` (the default) reads `daemon.json` from the
// malac home and otherwise starts the daemon through `malac web`. Any other
// `malac.connection` names a saved connection to a daemon elsewhere.

import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

export interface Endpoint {
  /** Base URL, e.g. `http://127.0.0.1:7700`. */
  url: string;
  token: string;
  /** How it was found, for the log and tooltips. */
  source: "daemon.json" | "started" | "remote";
  pid?: number;
  version?: string;
}

export interface ResolveOptions {
  /** Start a daemon when none is running. */
  allowStart: boolean;
}

/** Finds (and possibly starts) the daemon to talk to. */
export interface EndpointProvider {
  readonly description: string;
  /** Whether this provider can start the daemon (only this machine's). */
  readonly canStart: boolean;
  resolve(options: ResolveOptions): Promise<Endpoint>;
}

export type ConnectionSetting = { kind: "local" } | { kind: "saved"; name: string };

/** Parses `malac.connection`: `local`, or the name of a saved connection. */
export function parseConnectionSetting(value: unknown): ConnectionSetting {
  const text = typeof value === "string" ? value.trim() : "";
  if (!text || text === "local") return { kind: "local" };
  return { kind: "saved", name: text };
}

/** `malac.home`, else `$MALAC_HOME`, else `~/.malac`. */
export function malacHome(setting: string | undefined, env: NodeJS.ProcessEnv = process.env): string {
  const chosen = setting?.trim() || env.MALAC_HOME?.trim();
  if (!chosen) return path.join(os.homedir(), ".malac");
  return chosen.startsWith("~/") ? path.join(os.homedir(), chosen.slice(2)) : chosen;
}

export interface DaemonFile {
  url: string;
  token: string;
  pid: number;
  version: string;
  urls?: string[];
  started?: string;
}

export function readDaemonFile(home: string): DaemonFile | null {
  try {
    const value = JSON.parse(fs.readFileSync(path.join(home, "daemon.json"), "utf8")) as Partial<DaemonFile>;
    if (typeof value.url !== "string" || typeof value.token !== "string") return null;
    return {
      url: value.url,
      token: value.token,
      pid: Number(value.pid) || 0,
      version: String(value.version ?? ""),
      urls: value.urls,
      started: value.started,
    };
  } catch {
    return null;
  }
}

export function pidAlive(pid: number): boolean {
  if (!pid) return false;
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return (error as NodeJS.ErrnoException).code === "EPERM";
  }
}

/** Whether `GET {url}/api/daemon` answers with the token. */
export async function probe(url: string, token: string, timeoutMs = 3000): Promise<boolean> {
  try {
    const response = await fetch(`${url}/api/daemon`, {
      headers: { Authorization: `Bearer ${token}` },
      signal: AbortSignal.timeout(timeoutMs),
    });
    return response.ok;
  } catch {
    return false;
  }
}

/** The daemon `daemon.json` names, when its process lives and it answers. */
export async function runningDaemon(home: string): Promise<DaemonFile | null> {
  const file = readDaemonFile(home);
  if (!file || !pidAlive(file.pid)) return null;
  return (await probe(file.url, file.token)) ? file : null;
}

export interface Starter {
  /** Starts the daemon (via `malac web`) and returns its address. */
  start(): Promise<{ url: string; token: string }>;
}

export class LocalEndpoints implements EndpointProvider {
  readonly canStart = true;

  constructor(
    private readonly home: () => string,
    private readonly starter: Starter,
  ) {}

  get description(): string {
    return `this machine (${this.home()})`;
  }

  async resolve({ allowStart }: ResolveOptions): Promise<Endpoint> {
    const file = await runningDaemon(this.home());
    if (file) return { url: file.url, token: file.token, source: "daemon.json", pid: file.pid, version: file.version };
    if (!allowStart) throw new Error("the malac daemon is not running");
    const started = await this.starter.start();
    const again = readDaemonFile(this.home());
    return { url: started.url, token: started.token, source: "started", pid: again?.pid, version: again?.version };
  }
}

export interface SavedConnection {
  name: string;
  url: string;
  token: string;
  device?: string | null;
  added?: string;
}

/** `connections.json` in the malac home: the daemons this machine paired with. */
export function readConnections(home: string): SavedConnection[] {
  try {
    const value = JSON.parse(fs.readFileSync(path.join(home, "connections.json"), "utf8")) as { connections?: unknown };
    return (Array.isArray(value.connections) ? value.connections : []).filter(
      (c): c is SavedConnection => typeof c?.name === "string" && typeof c?.url === "string" && typeof c?.token === "string",
    );
  } catch {
    return [];
  }
}

/**
 * A daemon on another machine, from a saved connection (made by pairing,
 * `malac remote add`, and used by `malac --connect <name>`). That daemon
 * must already run; the extension never starts it.
 */
export class SavedEndpoints implements EndpointProvider {
  readonly canStart = false;

  constructor(
    private readonly name: string,
    private readonly home: () => string,
  ) {}

  get description(): string {
    return `saved connection "${this.name}"`;
  }

  async resolve(): Promise<Endpoint> {
    const saved = readConnections(this.home());
    const connection = saved.find((c) => c.name === this.name);
    if (!connection) {
      const names = saved.map((c) => c.name);
      throw new Error(
        names.length
          ? `no saved connection "${this.name}"; there are: ${names.join(", ")}. Set malac.connection to one of them, or to "local".`
          : `no saved connection "${this.name}"; add one with \`malac remote add ${this.name} <url> --pair <code>\`, or set malac.connection to "local".`,
      );
    }
    return { url: connection.url.replace(/\/+$/, ""), token: connection.token, source: "remote" };
  }
}

// Where the daemon is. `local` (the default) reads `daemon.json` from the
// malac home and otherwise starts the daemon through `malac web`. The
// `malac.connection` setting is the seam for other daemons: saved remote
// connections will plug in here as further providers.

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

/**
 * The seam for daemons on other machines. Saved connections (from pairing,
 * `malac --connect <name>`) live in `~/.malac/connections.json` as
 * `{name, url, token, device, added}`; resolving one is reading that entry
 * into an Endpoint with `source: "remote"`. Not implemented in this version.
 */
export class SavedEndpoints implements EndpointProvider {
  constructor(private readonly name: string) {}

  get description(): string {
    return `saved connection "${this.name}"`;
  }

  async resolve(): Promise<Endpoint> {
    throw new Error(
      `malac.connection is "${this.name}", but saved remote connections are not supported by this version of the extension yet. Set it to "local".`,
    );
  }
}

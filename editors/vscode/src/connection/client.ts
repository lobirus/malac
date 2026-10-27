// One WebSocket connection to the daemon plus its REST API, reconnecting with
// backoff. The endpoint is resolved again before every attempt, so a daemon
// that restarted on another port is found through its new `daemon.json`.

import { Emitter } from "../model/emitter";
import type { ServerMessage } from "../protocol";
import type { Endpoint, EndpointProvider } from "./endpoint";
import { openSocket, redact, TextSocket } from "./socket";

export type ConnectionState =
  /** Looking for or starting the daemon, or opening the socket. */
  | "connecting"
  | "online"
  /** Lost; retrying with backoff. */
  | "offline"
  /** The daemon shut down on purpose; waiting for it to come back. */
  | "stopped"
  /** A configuration problem that retrying will not fix. */
  | "failed";

export interface ConnectionStatus {
  state: ConnectionState;
  detail: string;
  endpoint: Endpoint | null;
}

const BACKOFF_MIN_MS = 500;
const BACKOFF_MAX_MS = 10_000;
const STOPPED_POLL_MS = 3000;
const REQUEST_TIMEOUT_MS = 60_000;
/** Shared files are at most 50 MB; anything larger is not one. */
const DOWNLOAD_LIMIT = 64 * 1024 * 1024;
/** Failed attempts after which a lost daemon is started again. */
const RESTART_AFTER = 2;

export interface ClientOptions {
  provider: () => EndpointProvider;
  /** The malac version this client expects, for the version-skew notice. */
  version: () => string | undefined;
  log: (line: string) => void;
  forceFallbackSocket?: boolean;
}

interface Waiting {
  resolve(message: ServerMessage): void;
  reject(error: Error): void;
  timer: ReturnType<typeof setTimeout>;
}

export class DaemonClient {
  private socket: TextSocket | null = null;
  private endpoint: Endpoint | null = null;
  private status: ConnectionStatus = { state: "connecting", detail: "", endpoint: null };
  private waiting = new Map<string, Waiting>();
  private nextRef = 1;
  private backoff = BACKOFF_MIN_MS;
  private failures = 0;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private running = false;
  private connecting = false;
  private shutdownReason: string | null = null;
  private autoStart = false;
  /** Bumped on every (re)connect so stale socket callbacks are ignored. */
  private session = 0;

  readonly onMessage = new Emitter<ServerMessage>();
  readonly onStatus = new Emitter<ConnectionStatus>();

  constructor(private readonly options: ClientOptions) {}

  get connection(): ConnectionStatus {
    return this.status;
  }

  get online(): boolean {
    return this.status.state === "online";
  }

  /**
   * Connects and keeps reconnecting. With `autoStart`, a daemon that is not
   * running is started (through `malac web`); without it the client only
   * waits for one to appear until `ensureStarted` is called.
   */
  start({ autoStart }: { autoStart: boolean }): void {
    this.running = true;
    // A view that opened first may already have asked for the daemon.
    this.autoStart ||= autoStart;
    this.shutdownReason = null;
    this.failures = 0;
    if (this.socket || this.connecting) return;
    this.schedule(0, this.autoStart);
  }

  /** Starts the daemon if it is not running (the user opened malac). */
  ensureStarted(): void {
    if (this.autoStart && this.status.state !== "stopped") return;
    this.autoStart = true;
    if (this.status.state !== "online" && !this.connecting) this.reconnect({ allowStart: true });
  }

  /** Drops the connection and connects again now (e.g. after a setting changed). */
  reconnect({ allowStart = true } = {}): void {
    this.running = true;
    if (allowStart) {
      this.shutdownReason = null;
      this.autoStart = true;
    }
    this.backoff = BACKOFF_MIN_MS;
    this.failures = 0;
    this.closeSocket("reconnecting");
    this.schedule(0, allowStart);
  }

  dispose(): void {
    this.running = false;
    clearTimeout(this.timer);
    this.closeSocket("closed");
  }

  /** Sends a command and resolves with its reply; an `error` reply rejects. */
  request(command: Record<string, unknown>, timeoutMs = REQUEST_TIMEOUT_MS): Promise<ServerMessage> {
    return new Promise((resolve, reject) => {
      const socket = this.socket;
      if (!socket || this.status.state !== "online") {
        reject(new Error("Not connected to the malac daemon"));
        return;
      }
      const ref = `x${this.nextRef++}`;
      const timer = setTimeout(() => {
        this.waiting.delete(ref);
        reject(new Error(`the daemon did not answer ${String(command.type)}`));
      }, timeoutMs);
      this.waiting.set(ref, { resolve, reject, timer });
      socket.send(JSON.stringify({ ...command, client_ref: ref }));
    });
  }

  /** Sends a command without waiting. Returns false when offline. */
  send(command: Record<string, unknown>): boolean {
    if (!this.socket || this.status.state !== "online") return false;
    this.socket.send(JSON.stringify(command));
    return true;
  }

  async get<T>(path: string): Promise<T> {
    return this.http<T>("GET", path);
  }

  async post<T>(path: string, body: unknown): Promise<T> {
    return this.http<T>("POST", path, body);
  }

  async put<T>(path: string, body: unknown): Promise<T> {
    return this.http<T>("PUT", path, body);
  }

  /** A file from the REST API (a shared file, speech audio) with its type. */
  async download(path: string, maxBytes = DOWNLOAD_LIMIT): Promise<{ data: Buffer; type: string }> {
    const endpoint = this.endpoint;
    if (!endpoint) throw new Error("Not connected to the malac daemon");
    const response = await fetch(`${endpoint.url}${path}`, {
      headers: { Authorization: `Bearer ${endpoint.token}` },
      signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS),
    });
    if (!response.ok) {
      const text = await response.text().catch(() => "");
      let message = text || `${response.status} ${response.statusText}`;
      try {
        message = (JSON.parse(text) as { error?: string }).error ?? message;
      } catch {
        // Not JSON.
      }
      throw new HttpError(response.status, message);
    }
    if (Number(response.headers.get("content-length") ?? 0) > maxBytes) throw new Error(`the file is larger than ${Math.round(maxBytes / 1024 / 1024)} MB`);
    const data = Buffer.from(await response.arrayBuffer());
    if (data.length > maxBytes) throw new Error(`the file is larger than ${Math.round(maxBytes / 1024 / 1024)} MB`);
    return { data, type: (response.headers.get("content-type") ?? "application/octet-stream").split(";")[0].trim() };
  }

  async http<T>(method: string, path: string, body?: unknown): Promise<T> {
    const endpoint = this.endpoint;
    if (!endpoint) throw new Error("Not connected to the malac daemon");
    const headers: Record<string, string> = { Authorization: `Bearer ${endpoint.token}` };
    if (body !== undefined) headers["Content-Type"] = "application/json";
    const response = await fetch(`${endpoint.url}${path}`, {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS),
    });
    const text = await response.text();
    let data: unknown = null;
    try {
      data = text ? JSON.parse(text) : null;
    } catch {
      data = null;
    }
    if (!response.ok) {
      const message = (data as { error?: string } | null)?.error ?? (text || `${response.status} ${response.statusText}`);
      throw new HttpError(response.status, message);
    }
    return data as T;
  }

  /** The address of the daemon this client talks to, once known. */
  get address(): Endpoint | null {
    return this.endpoint;
  }

  private schedule(delay: number, allowStart: boolean): void {
    clearTimeout(this.timer);
    if (!this.running) return;
    this.timer = setTimeout(() => void this.connect(allowStart), delay);
  }

  private async connect(allowStart: boolean): Promise<void> {
    if (!this.running || this.connecting || this.socket) return;
    this.connecting = true;
    const session = ++this.session;
    if (this.status.state !== "stopped") this.setStatus("connecting", "");
    try {
      const endpoint = await this.options.provider().resolve({ allowStart });
      if (session !== this.session || !this.running) return;
      this.endpoint = endpoint;
      const version = this.options.version();
      const base = endpoint.url.replace(/^http/, "ws");
      const url = `${base}/api/ws?token=${encodeURIComponent(endpoint.token)}${version ? `&version=${encodeURIComponent(version)}` : ""}`;
      this.options.log(`connecting to ${redact(url)} (${endpoint.source})`);
      // Messages that arrive before the handshake has resolved (the snapshot can)
      // wait until this client is online, so replies to them can be sent.
      let early: string[] | null = [];
      const socket = await openSocket(
        url,
        {
          onMessage: (text) => {
            if (session !== this.session) return;
            if (early) early.push(text);
            else this.receive(text);
          },
          onClose: (reason) => {
            if (session === this.session) this.lost(reason);
          },
        },
        { forceFallback: this.options.forceFallbackSocket },
      );
      if (session !== this.session || !this.running) {
        socket.close();
        return;
      }
      this.socket = socket;
      this.backoff = BACKOFF_MIN_MS;
      this.failures = 0;
      this.shutdownReason = null;
      this.setStatus("online", "");
      const queued = early;
      early = null;
      for (const text of queued) this.receive(text);
    } catch (error) {
      if (session !== this.session) return;
      const message = error instanceof Error ? error.message : String(error);
      this.options.log(`connection failed: ${message}`);
      if (error instanceof Error && error.name === "MissingBinaryError") {
        this.setStatus("failed", message);
        return;
      }
      if (/no saved connection/.test(message)) {
        this.setStatus("failed", message);
        return;
      }
      this.retry(message);
    } finally {
      this.connecting = false;
    }
  }

  private lost(reason: string): void {
    this.socket = null;
    for (const [ref, waiting] of this.waiting) {
      clearTimeout(waiting.timer);
      waiting.reject(new Error("Disconnected from the malac daemon"));
      this.waiting.delete(ref);
    }
    this.options.log(`disconnected: ${reason}`);
    if (!this.running) return;
    this.retry(this.shutdownReason ? `the daemon stopped (${this.shutdownReason})` : reason);
  }

  private retry(detail: string): void {
    this.failures += 1;
    const canStart = this.options.provider().canStart;
    if (canStart && (this.shutdownReason || !this.autoStart)) {
      // Stopped on purpose, or not started yet: wait for a daemon to appear
      // without starting one.
      this.setStatus("stopped", this.shutdownReason ? detail : "not running");
      this.schedule(STOPPED_POLL_MS, false);
      return;
    }
    this.setStatus("offline", detail);
    const delay = this.backoff;
    this.backoff = Math.min(this.backoff * 2, BACKOFF_MAX_MS);
    this.schedule(delay, this.failures >= RESTART_AFTER);
  }

  private receive(text: string): void {
    let message: ServerMessage;
    try {
      message = JSON.parse(text) as ServerMessage;
    } catch {
      return;
    }
    if (message.type === "shutdown") this.shutdownReason = String(message.reason ?? "stopped");
    const ref = message.client_ref;
    const waiting = ref ? this.waiting.get(ref) : undefined;
    if (ref && waiting) {
      this.waiting.delete(ref);
      clearTimeout(waiting.timer);
      if (message.type === "error") waiting.reject(new Error(String(message.message ?? "error")));
      else waiting.resolve(message);
      // Replies that carry state are applied like broadcasts too.
      if (!["snapshot", "workspace_opened", "launched"].includes(message.type)) return;
    }
    this.onMessage.fire(message);
  }

  private closeSocket(reason: string): void {
    this.session += 1;
    const socket = this.socket;
    this.socket = null;
    socket?.close();
    if (socket) this.lost(reason);
  }

  private setStatus(state: ConnectionState, detail: string): void {
    if (this.status.state === state && this.status.detail === detail && this.status.endpoint === this.endpoint) return;
    this.status = { state, detail, endpoint: this.endpoint };
    this.onStatus.fire(this.status);
  }
}

export class HttpError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
  }
}

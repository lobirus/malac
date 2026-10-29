// Speech on this device: while "Speak on this device" is on and the daemon
// has speech, the connection listens (`voice_listen`) and plays the `say`
// broadcasts addressed to its listener id, one sentence after another.
// Free of the vscode module.

import type { SayMessage, ServerMessage } from "../protocol";

/** A sentence that waited longer than this is no longer worth hearing. */
const STALE_MS = 2 * 60_000;

export interface SpeakerHost {
  request(command: Record<string, unknown>): Promise<ServerMessage>;
  /** `GET /api/voice/say/{id}`. */
  download(id: string): Promise<{ data: Buffer; type: string }>;
  /** Plays the audio; resolves when it finished or `signal` stopped it. */
  play(audio: { id: string; data: Buffer; type: string }, signal: AbortSignal): Promise<void>;
  /** Shows the sentence while it plays. */
  show(text: string, playing: Promise<void>): void;
  /** A sentence could not be played. */
  failed(error: Error, say: SayMessage): void;
  log(line: string): void;
}

export class Speaker {
  /** This connection's listener id while it listens. */
  listener: number | null = null;
  private wanted = false;
  private syncing: Promise<void> = Promise.resolve();
  private queue: Promise<void> = Promise.resolve();
  /** Bumped on every connection, so replies for an old one are dropped. */
  private connection = 0;
  /** Sentences the daemon called off; queued ones are skipped. */
  private cancelled = new Set<string>();
  private playing: { id: string; stop: AbortController } | null = null;

  constructor(private readonly host: SpeakerHost) {}

  /** A new connection (its snapshot arrived): it listens again if it should. */
  connected(wanted: boolean): Promise<void> {
    this.connection += 1;
    this.listener = null;
    return this.want(wanted);
  }

  disconnected(): void {
    this.connection += 1;
    this.listener = null;
  }

  /** Whether this device should play speech: the switch is on, the daemon has speech and audio can play here. */
  want(wanted: boolean): Promise<void> {
    this.wanted = wanted;
    this.syncing = this.syncing.then(() => this.reconcile());
    return this.syncing;
  }

  private async reconcile(): Promise<void> {
    const on = this.wanted;
    if (on === (this.listener !== null)) return;
    const connection = this.connection;
    try {
      const reply = await this.host.request({ type: "voice_listen", on });
      if (connection !== this.connection) return;
      this.listener = on && typeof reply.listener === "number" ? reply.listener : null;
      this.host.log(on ? `speech plays here (listener ${this.listener})` : "speech no longer plays here");
    } catch (error) {
      this.host.log(`voice_listen failed: ${error instanceof Error ? error.message : String(error)}`);
    }
  }

  /** Queues a `say` broadcast when it is for this connection; false otherwise. */
  hear(say: SayMessage): boolean {
    if (this.listener === null || say.listener !== this.listener) return false;
    const queued = Date.now();
    // Fetch now, play in turn.
    const audio = this.host.download(say.id);
    audio.catch(() => undefined);
    this.queue = this.queue.then(async () => {
      if (Date.now() - queued > STALE_MS) {
        this.host.log(`skipped a stale sentence: ${say.text}`);
        return;
      }
      try {
        const { data, type } = await audio;
        if (this.cancelled.has(say.id)) return;
        const stop = new AbortController();
        this.playing = { id: say.id, stop };
        const playing = this.host.play({ id: say.id, data, type }, stop.signal);
        this.host.show(say.text, playing);
        await playing;
      } catch (error) {
        this.host.failed(error instanceof Error ? error : new Error(String(error)), say);
      } finally {
        this.playing = null;
        this.cancelled.delete(say.id);
      }
    });
    return true;
  }

  /** `say_cancelled`: the user answered an approval or prompted; these sentences stop now. */
  cancel(ids: string[]): void {
    for (const id of ids) this.cancelled.add(id);
    if (this.playing && ids.includes(this.playing.id)) this.playing.stop.abort();
  }

  /** Resolves once everything queued has played (for tests). */
  idle(): Promise<void> {
    return this.queue;
  }
}

// Dictation for the window: one recording at a time (there is one
// microphone), owned by the chat that started it, whose composer gets the
// text. Free of the vscode module.

import { Emitter } from "../model/emitter";
import type { Dictation } from "./cli";

export interface DictationUpdate {
  /** The chat the recording belongs to. */
  owner: object;
  state: "recording" | "transcribing" | "done";
  /** When the recording started (ms since the epoch), for the elapsed time. */
  started?: number;
  text?: string;
  error?: string;
}

export class DictationManager {
  private session: { owner: object; dictation: Dictation } | null = null;
  private starting: object | null = null;
  /** Cancelled before the recording started. */
  private abandoned = false;
  readonly onUpdate = new Emitter<DictationUpdate>();

  /** `start` makes a recording, or throws with what stands in the way. */
  constructor(private readonly start: () => Promise<Dictation>) {}

  /** The chat recording or about to, if any. */
  get owner(): object | null {
    return this.session?.owner ?? this.starting;
  }

  /** Starts a recording for `owner`, or stops the one running (whoever started it). */
  async toggle(owner: object): Promise<void> {
    if (this.session || this.starting) {
      if (this.session && !this.session.dictation.stopping) this.stop();
      return;
    }
    await this.begin(owner);
  }

  async begin(owner: object): Promise<void> {
    if (this.session || this.starting) {
      if (this.owner !== owner) this.onUpdate.fire({ owner, state: "done", error: "Dictation is already on in another malac chat." });
      return;
    }
    this.starting = owner;
    this.abandoned = false;
    let dictation: Dictation;
    try {
      dictation = await this.start();
    } catch (error) {
      this.starting = null;
      this.onUpdate.fire({ owner, state: "done", error: error instanceof Error ? error.message : String(error) });
      return;
    }
    this.starting = null;
    if (this.abandoned) {
      this.abandoned = false;
      dictation.cancel();
      this.onUpdate.fire({ owner, state: "done" });
      return;
    }
    const session = { owner, dictation };
    this.session = session;
    this.onUpdate.fire({ owner, state: "recording", started: dictation.started });
    const result = await dictation.result;
    if (this.session === session) this.session = null;
    if ("cancelled" in result) this.onUpdate.fire({ owner, state: "done" });
    else if ("error" in result) this.onUpdate.fire({ owner, state: "done", error: result.error });
    else this.onUpdate.fire({ owner, state: "done", text: result.text.trim() });
  }

  /** Ends the recording; its text goes to the chat that started it. */
  stop(): void {
    const session = this.session;
    if (!session || session.dictation.stopping) return;
    session.dictation.stop();
    this.onUpdate.fire({ owner: session.owner, state: "transcribing" });
  }

  /** Drops the recording of `owner` (any when omitted). */
  cancel(owner?: object): void {
    if (this.starting && (!owner || this.starting === owner)) this.abandoned = true;
    const session = this.session;
    if (!session || (owner && session.owner !== owner)) return;
    session.dictation.cancel();
  }

  dispose(): void {
    this.session?.dictation.cancel();
    this.onUpdate.dispose();
  }
}

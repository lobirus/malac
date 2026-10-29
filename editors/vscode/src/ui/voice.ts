// Voice in VS Code: dictation into a chat's composer, speech on this device
// and talk mode in a terminal, all through the malac CLI on the machine the
// extension runs on (webviews may not use the microphone), plus the Voice
// Settings quick picks, which change the daemon's `[voice]` table.

import * as fs from "node:fs";
import * as fsp from "node:fs/promises";
import * as path from "node:path";
import * as vscode from "vscode";
import type { ChatVoice } from "../chat/controller";
import type { VoiceOffer } from "../chat/messages";
import type { DaemonClient } from "../connection/client";
import { isRemoteEndpoint } from "../model/attachments";
import { Disposable, Emitter } from "../model/emitter";
import type { DaemonStore } from "../model/store";
import { featureReady, type SayMessage, type VoiceFeature, type VoiceFeatureName, type VoiceSettings, type VoiceStatus } from "../protocol";
import { Dictation, play as playFile, VoiceSupport, VoiceTarget, voiceArgs } from "../voice/cli";
import { DictationManager } from "../voice/dictation";
import { Speaker } from "../voice/speech";

/** The spoken sentence stays in the status bar at least this long. */
const SAID_MIN_MS = 4000;

export interface VoiceOptions {
  /** The malac binary. */
  command: () => string;
  /** `MALAC_HOME` for the CLI, when the extension uses a custom home. */
  home: () => string | undefined;
  /** The saved connection the extension uses (`malac.connection`); null for this machine. */
  connection: () => string | null;
  /** The chat a command works on: its tab and agent. */
  chat: () => { tab: string | null; agent: string | null };
  log: (line: string) => void;
}

const FEATURE_TITLES: Record<VoiceFeatureName, string> = { dictation: "Dictation", speech: "Speech", talk: "Talk mode" };

type Item = vscode.QuickPickItem & { run?: () => Promise<void> };

export class VoiceFeatures implements ChatVoice, vscode.Disposable {
  readonly dictation: DictationManager;
  private readonly speaker: Speaker;
  private readonly support = new VoiceSupport();
  private readonly changed = new Emitter<void>();
  private talkTerminal: { terminal: vscode.Terminal; key: string } | undefined;
  private warnedPlay = false;
  private disposables: vscode.Disposable[] = [];

  constructor(
    private readonly context: vscode.ExtensionContext,
    private readonly store: DaemonStore,
    private readonly client: DaemonClient,
    private readonly options: VoiceOptions,
  ) {
    this.dictation = new DictationManager(() => this.startDictation());
    this.speaker = new Speaker({
      request: (command) => client.request(command),
      download: (id) => client.download(`/api/voice/say/${encodeURIComponent(id)}`),
      play: (audio, signal) => this.play(audio, signal),
      show: (text, playing) => this.said(text, playing),
      failed: (error, say) => this.playFailed(error, say),
      log: (line) => options.log(line),
    });
    this.disposables.push(
      store.onChange.event((change) => {
        // Every snapshot (one per connection) carries the voice status too.
        if (change.voice) void this.speaker.want(this.wantSpeech());
        if (change.say) this.speaker.hear(change.say);
        if (change.sayCancelled) this.speaker.cancel(change.sayCancelled);
      }),
      client.onStatus.event((status) => {
        // A new connection has no listener yet; it listens again with what the last one knew.
        if (status.state === "online") void this.speaker.connected(this.wantSpeech());
        else this.speaker.disconnected();
      }),
      vscode.workspace.onDidChangeConfiguration((event) => {
        if (event.affectsConfiguration("malac.speakOnThisDevice")) void this.speaker.want(this.wantSpeech());
        if (event.affectsConfiguration("malac.path")) void this.recheck();
      }),
      vscode.window.onDidCloseTerminal((terminal) => {
        if (terminal === this.talkTerminal?.terminal) this.talkTerminal = undefined;
      }),
    );
    void this.recheck();
  }

  dispose(): void {
    this.dictation.dispose();
    this.changed.dispose();
    for (const d of this.disposables) d.dispose();
  }

  onChange(listener: () => void): Disposable {
    return this.changed.event(listener);
  }

  /** Asks the malac binary again whether it has the voice commands (after an update, say). */
  async recheck(): Promise<void> {
    await this.support.check(this.options.command(), { again: true });
    void this.speaker.want(this.wantSpeech());
    this.changed.fire();
  }

  offer(): VoiceOffer {
    const voice = this.store.voice;
    return { dictation: featureReady(voice?.dictation), talk: featureReady(voice?.talk), blocked: this.blocked() };
  }

  /** Why the voice commands cannot run for this window, as far as known now. */
  private blocked(): string | null {
    if (vscode.env.remoteName && this.context.extension.extensionKind === vscode.ExtensionKind.Workspace) {
      return `Voice is not available in this remote window: the extension runs on the ${vscode.env.remoteName} side, away from your microphone and speakers. Use a local window, the web UI or the terminal on your computer.`;
    }
    const support = this.support.known(this.options.command());
    return support && !support.ok ? support.reason : null;
  }

  /** The same, once the binary has answered. */
  private async ready(): Promise<string | null> {
    await this.support.check(this.options.command());
    return this.blocked();
  }

  private target(): VoiceTarget {
    return { command: this.options.command(), home: this.options.home(), connect: this.options.connection() };
  }

  // Dictation

  private async startDictation(): Promise<Dictation> {
    const reason = await this.ready();
    if (reason) throw new Error(reason);
    if (!featureReady(this.store.voice?.dictation)) {
      throw new Error("Dictation is off, or no model can transcribe speech. Turn it on and pick a model in malac: Voice Settings.");
    }
    return new Dictation(this.target(), { log: (line) => this.options.log(`[dictate] ${line}`) });
  }

  // Speech

  private wantSpeech(): boolean {
    const on = vscode.workspace.getConfiguration("malac").get<boolean>("speakOnThisDevice", true);
    const support = this.support.known(this.options.command());
    return on && featureReady(this.store.voice?.speech) && Boolean(support?.ok) && this.blocked() === null;
  }

  private async play(audio: { id: string; data: Buffer; type: string }, signal: AbortSignal): Promise<void> {
    const dir = path.join(this.context.globalStorageUri.fsPath, "voice");
    await fsp.mkdir(dir, { recursive: true });
    const file = path.join(dir, `${audio.id.replace(/[^\w-]+/g, "_")}.${/wav/.test(audio.type) ? "wav" : "mp3"}`);
    await fsp.writeFile(file, audio.data);
    try {
      await playFile(this.target(), file, signal);
      this.warnedPlay = false;
    } finally {
      await fsp.rm(file, { force: true });
    }
  }

  private said(text: string, playing: Promise<void>): void {
    const shown = Promise.all([playing.catch(() => undefined), new Promise((resolve) => setTimeout(resolve, SAID_MIN_MS))]);
    const line = text.length > 120 ? `${text.slice(0, 119)}…` : text;
    vscode.window.setStatusBarMessage(`$(unmute) ${line}`, shown);
  }

  private playFailed(error: Error, say: SayMessage): void {
    this.options.log(`could not play "${say.text}": ${error.message}`);
    if (this.warnedPlay) return;
    this.warnedPlay = true;
    void vscode.window.showWarningMessage(`malac could not speak "${say.text}": ${error.message}`, "Voice Settings", "Stop speaking here").then((choice) => {
      if (choice === "Voice Settings") void vscode.commands.executeCommand("malac.voiceSettings");
      if (choice === "Stop speaking here") void vscode.workspace.getConfiguration("malac").update("speakOnThisDevice", false, vscode.ConfigurationTarget.Global);
    });
  }

  // Talk mode

  /** Talk mode in a terminal, through the agent the chat shows (or the tab's active one). */
  async talk(tab?: string | null, agent?: string | null): Promise<void> {
    const reason = await this.ready();
    if (reason) throw new Error(reason);
    if (!featureReady(this.store.voice?.talk)) throw new Error("Talk mode is off, or no Realtime model is available. Turn it on in malac: Voice Settings.");
    const chat = this.options.chat();
    const tabId = tab ?? chat.tab;
    const agentId = tab ? (agent ?? null) : chat.agent;
    if (!tabId) throw new Error("Open a malac tab for this folder first.");
    const target = this.target();
    const args = voiceArgs(target, ["talk", "--tab", tabId, ...(agentId ? ["--agent", agentId] : [])]);
    const key = args.join(" ");
    if (this.talkTerminal?.key === key && this.talkTerminal.terminal.exitStatus === undefined) {
      this.talkTerminal.terminal.show();
      return;
    }
    // One conversation at a time: there is one microphone.
    this.talkTerminal?.terminal.dispose();
    const cwd = this.store.tab(tabId)?.tab.cwd;
    const terminal = vscode.window.createTerminal({
      name: "malac talk",
      shellPath: target.command,
      shellArgs: args,
      cwd: cwd && !isRemoteEndpoint(this.client.address) && fs.existsSync(cwd) ? cwd : undefined,
      env: target.home ? { MALAC_HOME: target.home } : undefined,
      iconPath: new vscode.ThemeIcon("mic"),
    });
    this.talkTerminal = { terminal, key };
    terminal.show();
  }

  // Settings

  /** "malac: Voice Settings": the daemon's voice settings and this device's speech switch, until closed. */
  async settings(): Promise<void> {
    for (;;) {
      const voice = this.store.voice;
      if (!voice) throw new Error(`The daemon (malac ${this.store.daemon?.version ?? "?"}) has no voice settings. Update malac and restart the daemon.`);
      const picked = await vscode.window.showQuickPick(this.items(voice), { title: "malac: Voice", placeHolder: "Pick a setting to change it", matchOnDescription: true });
      if (!picked?.run) return;
      try {
        await picked.run();
      } catch (error) {
        void vscode.window.showErrorMessage(`malac: ${error instanceof Error ? error.message : String(error)}`);
      }
    }
  }

  private items(voice: VoiceStatus): Item[] {
    const items: Item[] = [];
    const blocked = this.blocked();
    if (blocked) items.push({ label: "$(warning) Voice does not work in this window", detail: blocked });
    const speakHere = vscode.workspace.getConfiguration("malac").get<boolean>("speakOnThisDevice", true);
    for (const name of ["dictation", "speech", "talk"] as const) {
      const feature = voice[name];
      items.push({ label: FEATURE_TITLES[name], kind: vscode.QuickPickItemKind.Separator });
      items.push({
        label: `${feature.enabled ? "$(pass-filled)" : "$(circle-large-outline)"} ${FEATURE_TITLES[name]}`,
        description: feature.enabled ? (feature.model ? "On" : "On, but no model can do it") : "Off",
        detail: name === "speech" ? "Reads approvals, questions and notable results aloud" : name === "talk" ? "A spoken conversation that hands the work to malac" : "The microphone button in the chat",
        run: () => this.put({ [name]: !feature.enabled }),
      });
      items.push({
        label: "    Model",
        description: modelLabel(feature),
        run: () => this.pickModel(name, feature),
      });
      if (name === "dictation") {
        items.push({ label: "    Language", description: voice.language || "Detected from the speech", run: () => this.pickLanguage(voice.language ?? "") });
      } else {
        items.push({ label: "    Voice", description: feature.voice || "The model's default", run: () => this.pickVoice(name, feature, voice.voices) });
      }
      if (name === "speech") {
        items.push({
          label: `    ${speakHere ? "$(pass-filled)" : "$(circle-large-outline)"} Speak on this device`,
          description: speakHere ? "On" : "Off",
          detail: "Of the devices with this on, speech plays on the one you used last",
          run: () => Promise.resolve(vscode.workspace.getConfiguration("malac").update("speakOnThisDevice", !speakHere, vscode.ConfigurationTarget.Global)),
        });
      }
    }
    items.push({ label: "", kind: vscode.QuickPickItemKind.Separator });
    items.push({
      label: "$(refresh) Read the providers' model lists again",
      description: voice.checked ? `last read ${new Date(voice.checked).toLocaleString()}` : undefined,
      run: async () => this.store.setVoice(await this.client.get<VoiceStatus>("/api/voice?refresh=true")),
    });
    return items;
  }

  private async put(settings: VoiceSettings): Promise<void> {
    this.store.setVoice(await this.client.put<VoiceStatus>("/api/voice", settings));
  }

  private async pickModel(name: VoiceFeatureName, feature: VoiceFeature): Promise<void> {
    if (!feature.models.length) {
      const needs = name === "dictation" ? "a transcription model (whisper-1, gpt-4o-mini-transcribe)" : name === "speech" ? "a speech model (tts-1, gpt-4o-mini-tts)" : "a Realtime model (gpt-realtime)";
      void vscode.window.showInformationMessage(`No provider offers ${needs}. Add an OpenAI-style provider that has one, then read the model lists again.`);
      return;
    }
    const automatic = { label: "Automatic", description: feature.models[0] ? `the first that can: ${feature.models[0]}` : undefined, value: null as string | null, picked: !feature.configured };
    const options = feature.models.map((model) => ({ label: model, description: model === "system" ? "this computer's own voice" : undefined, value: model as string | null, picked: model === feature.configured }));
    const picked = await vscode.window.showQuickPick([automatic, ...options], { title: `${FEATURE_TITLES[name]} model` });
    if (picked) await this.put({ [`${name}_model`]: picked.value });
  }

  private async pickVoice(name: "speech" | "talk", feature: VoiceFeature, voices: string[]): Promise<void> {
    const automatic = { label: "The model's default", value: null as string | null };
    const options = [...new Set([...voices, ...(feature.voice ? [feature.voice] : [])])].map((v) => ({ label: v, description: v === feature.voice ? "in use" : undefined, value: v as string | null }));
    const picked = await vscode.window.showQuickPick([automatic, ...options], { title: `${FEATURE_TITLES[name]} voice` });
    if (picked) await this.put({ [`${name}_voice`]: picked.value });
  }

  private async pickLanguage(current: string): Promise<void> {
    const value = await vscode.window.showInputBox({
      title: "Dictation language",
      prompt: "An ISO 639-1 code such as en, de or hu; empty lets the model work it out",
      value: current,
      validateInput: (text) => (!text.trim() || /^[a-z]{2,3}(-[A-Za-z]{2,4})?$/.test(text.trim()) ? undefined : "Use a language code such as en or de"),
    });
    if (value !== undefined) await this.put({ language: value.trim() || null });
  }
}

function modelLabel(feature: VoiceFeature): string {
  if (feature.configured) return feature.configured === "system" ? "system (this computer's voice)" : feature.configured;
  if (feature.model) return `Automatic: ${feature.model}`;
  return "None available";
}

// Messages between the extension host and the chat webview. Types only; the
// webview imports them too.

import type { ConnectionState } from "../connection/client";
import type { ContextChip } from "../model/context";
import type { ModelGroup } from "../model/models";
import type { DisplayStatus } from "../model/store";
import type { DecisionRecord, Envelope, ImageInput, Message, Mode, Usage } from "../protocol";

export interface AgentSummary {
  id: string;
  name: string;
  label: string;
  parent: string | null;
  depth: number;
  status: DisplayStatus;
  live: boolean;
  model: string;
  activity: string;
}

export interface PendingView {
  id: string;
  callId: string;
  agent: string;
  agentLabel: string;
  tool: string;
  summary: string;
  input: unknown;
  /** One line for the request, e.g. "Enable plugin x, which runs hooks". */
  title: string;
  /** Shown behind "Show details": a plugin's contents, a long command, the input. */
  details: string | null;
  /** Set for enabling a plugin that runs code. */
  plugin: string | null;
}

export interface SlashCompletion {
  name: string;
  description: string;
  hint: string;
  source: string;
}

export interface Banner {
  id: string;
  level: "info" | "warning" | "error";
  text: string;
  actions: { label: string; command: string; args?: unknown[]; primary?: boolean }[];
}

/** The voice buttons the chat shows: each only while the daemon has the feature ready. */
export interface VoiceOffer {
  /** The microphone button in the composer. */
  dictation: boolean;
  /** The talk button in the header. */
  talk: boolean;
  /** Why voice cannot work in this window (shown when a button is used); null when it can. */
  blocked: string | null;
}

/** A file to open in an editor or save: a shared one by id, else the data itself. */
export interface MediaFile {
  id?: string;
  name: string;
  media_type: string;
  /** Base64, for images that are not shared files (tool results, attachments). */
  data?: string;
}

export interface ChatState {
  connection: { state: ConnectionState; detail: string };
  banners: Banner[];
  tab: { id: string; name: string; cwd: string; mode: Mode; workspace: string } | null;
  tabs: { id: string; name: string; cwd: string; workspace: string; status: DisplayStatus }[];
  /** Agents of the tab: top-level ones, then subagents with a `parent`. */
  agents: AgentSummary[];
  /** The agent the chat shows; null means the next prompt starts a new one. */
  agent:
    | (AgentSummary & {
        usage: Usage;
        contextTokens: number;
        readOnly: boolean;
        running: boolean;
      })
    | null;
  models: string[];
  /** `models` (plus the one in use) by provider, for the model menu's headings. */
  modelGroups: ModelGroup[];
  defaultModel: string | null;
  /** The model a new agent would use. */
  newModel: string | null;
  pending: PendingView[];
  context: ContextChip | null;
  /** Home directory, for showing paths as ~/… */
  home: string | null;
  /** Where to drop files: VS Code needs Shift held to drop into a webview. */
  dropHint: string;
  /** Slash commands: built-ins, then the tab's custom commands. */
  commands: SlashCompletion[];
  /** The daemon's address when it runs on another machine: files are uploaded to it. */
  remote: string | null;
  voice: VoiceOffer;
}

export type ToWebview =
  | { type: "state"; state: ChatState }
  | {
      type: "transcript";
      agent: string | null;
      history: Message[];
      events: Envelope[];
      error?: string;
      /** The approvals asked for and their answers, shown on the tool cards. */
      decisions?: DecisionRecord[];
    }
  | { type: "events"; agent: string; events: Envelope[] }
  | { type: "files"; id: number; files: { path: string; relative: string }[] }
  | { type: "attach"; paths: { path: string; label: string }[]; images?: ImageInput[] }
  | { type: "focus"; text?: string }
  | { type: "sent"; ok: boolean; error?: string; notes?: string[] }
  | { type: "toast"; level: "info" | "warning" | "error"; message: string }
  /** A shared file's contents (for images shown inline), or why it cannot be shown. */
  | { type: "shared"; id: string; media_type?: string; data?: string; error?: string }
  /** The chat's recording: started, being transcribed, or done (with the text or an error). */
  | { type: "dictation"; state: "recording" | "transcribing" | "done"; started?: number; text?: string; error?: string };

export type FromWebview =
  | { type: "ready" }
  | {
      type: "submit";
      text: string;
      attachments: { path: string }[];
      images: ImageInput[];
    }
  | { type: "stop" }
  | { type: "selectAgent"; agent: string | null }
  | { type: "selectTab"; tab: string }
  | { type: "setModel"; model: string }
  | { type: "setMode"; mode: Mode }
  | { type: "toggleContext"; enabled: boolean }
  | { type: "searchFiles"; id: number; query: string }
  | { type: "upload"; files: { name: string; type: string; data: string }[] }
  | { type: "approve"; request: string; decision: "allow" | "allow_session" | "deny" }
  | { type: "openFile"; path: string; line?: number }
  | { type: "openAgent"; agent: string }
  | { type: "command"; command: string; args?: unknown[] }
  | { type: "loadImages"; agent: string }
  | { type: "loadShared"; id: string }
  | { type: "media"; action: "open" | "save"; file: MediaFile }
  | { type: "dictation"; action: "toggle" | "stop" | "cancel" };

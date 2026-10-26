// Messages between the extension host and the chat webview. Types only; the
// webview imports them too.

import type { ConnectionState } from "../connection/client";
import type { ContextChip } from "../model/context";
import type { DisplayStatus } from "../model/store";
import type { Envelope, ImageInput, Message, Mode, Usage } from "../protocol";

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
}

export type ToWebview =
  | { type: "state"; state: ChatState }
  | {
      type: "transcript";
      agent: string | null;
      history: Message[];
      events: Envelope[];
      error?: string;
    }
  | { type: "events"; agent: string; events: Envelope[] }
  | { type: "files"; id: number; files: { path: string; relative: string }[] }
  | { type: "attach"; paths: { path: string; label: string }[]; images?: ImageInput[] }
  | { type: "focus"; text?: string }
  | { type: "sent"; ok: boolean; error?: string; notes?: string[] }
  | { type: "toast"; level: "info" | "warning" | "error"; message: string };

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
  | { type: "loadImages"; agent: string };

// The daemon's wire types, as described in docs/protocol.md. Only the fields
// the extension reads are typed; everything else passes through untouched.

export type Mode = "ask" | "edits" | "autopilot";
export type AgentStatus = "idle" | "running" | "completed" | "failed" | "cancelled";
export type Decision = "allow" | "allow_session" | "deny";

export interface Usage {
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_write_tokens: number;
  cost: number;
}

export interface DaemonStatus {
  version: string;
  pid: number;
  started: string;
  version_mismatch: boolean;
  clients: number;
  attached: boolean;
  remote: boolean;
  user_home?: string | null;
}

export interface WorkspaceSummary {
  id: string;
  name: string;
  tabs: number;
  active_tab: string | null;
  running: number;
  created: string;
  updated: string;
}

export interface AgentView {
  id: string;
  label: string;
  parent: string | null;
  depth: number;
  read_only?: boolean;
  cwd: string;
  created: string;
  model: string;
  status: AgentStatus;
  activity: string;
  usage: Usage;
  report: string | null;
  live: boolean;
  title?: string;
}

/** A custom slash command: user, project, plugin (`plugin:name`) or skill. */
export interface CommandInfo {
  name: string;
  description: string;
  argument_hint?: string | null;
  /** `user`, `project`, `plugin <id>` or `skill`. */
  source: string;
}

/** Where a tab sits in its repository (`Layout`, flattened) and its custom commands. */
export interface ProjectSummary {
  root: string;
  subdir?: string | null;
  siblings?: string[];
  /** Set when the tab is one part of a larger repository. */
  suggest_root?: string | null;
  commands?: CommandInfo[];
}

export interface TabView {
  id: string;
  name: string;
  cwd: string;
  agents: string[];
  active_agent: string | null;
  mode: Mode;
  options: { model?: string; mode?: Mode; profile?: string; no_control?: boolean };
  loaded: boolean;
  snapshots: AgentView[];
  /** Once the tab's runtime is loaded. */
  project?: ProjectSummary | null;
}

export interface WorkspaceView {
  id: string;
  name: string;
  active_tab: string | null;
  created: string;
  updated: string;
  tabs: TabView[];
}

export interface PendingApproval {
  id: string;
  call_id: string;
  agent: string;
  agent_label: string;
  tool: string;
  summary: string;
  input: unknown;
  tab: string | null;
}

export interface JobView {
  id: string;
  command: string;
  cwd: string;
  pid: number | null;
  log: string;
  started: string;
  finished: string | null;
  exit_code: number | null;
  status: "running" | "exited" | "killed" | "unknown";
  agent: string | null;
  tab: string | null;
}

/** One voice feature. Ready when `enabled` and `model` is set. */
export interface VoiceFeature {
  enabled: boolean;
  /** The model in use: `configured`, else the first of `models`; null when nothing can do it. */
  model: string | null;
  configured?: string | null;
  /** For speech and talk mode. */
  voice?: string | null;
  /** `provider/model` names that can do it; speech may list `system`. */
  models: string[];
}

/** What dictation, speech and talk mode offer (snapshot `voice`, `voice` broadcasts, `/api/voice`). */
export interface VoiceStatus {
  dictation: VoiceFeature;
  speech: VoiceFeature;
  talk: VoiceFeature;
  language?: string | null;
  voices: string[];
  checked?: string | null;
}

export type VoiceFeatureName = "dictation" | "speech" | "talk";

export function featureReady(feature: VoiceFeature | null | undefined): boolean {
  return Boolean(feature?.enabled && feature.model);
}

/** A sentence to read aloud, for the connection whose listener id it names. */
export interface SayMessage {
  type: "say";
  id: string;
  text: string;
  reason: "approval" | "agent" | "finished" | "failed" | string;
  tab: string | null;
  agent: string | null;
  listener: number;
}

/** `PUT /api/voice`: any of these; null or "" removes a setting. */
export interface VoiceSettings {
  dictation?: boolean | null;
  dictation_model?: string | null;
  language?: string | null;
  speech?: boolean | null;
  speech_model?: string | null;
  speech_voice?: string | null;
  talk?: boolean | null;
  talk_model?: string | null;
  talk_voice?: string | null;
}

export interface Snapshot {
  type: "snapshot";
  client_ref?: string;
  daemon: DaemonStatus;
  workspaces: WorkspaceSummary[];
  workspace: WorkspaceView | null;
  pending: PendingApproval[];
  jobs: JobView[];
  update?: unknown;
  control?: unknown;
  models: string[];
  default_model: string | null;
  /** Older daemons have none. */
  voice?: VoiceStatus | null;
}

export type Block =
  | { type: "text"; text: string }
  | { type: "image"; media_type: string; data: string }
  | { type: "thinking"; text: string; signature?: string }
  | { type: "tool_use"; id: string; name: string; input: unknown }
  | { type: "tool_result"; tool_use_id: string; content: Block[]; is_error?: boolean };

export interface Message {
  role: "user" | "assistant";
  content: Block[];
}

/** An agent event: `agent`, `seq`, `at` and the event's own fields. */
export interface Envelope {
  agent: string;
  seq: number;
  at: string;
  type: string;
  [field: string]: unknown;
}

export interface SessionSummary {
  id: string;
  label: string;
  parent: string | null;
  cwd: string;
  model: string;
  created: string;
  title: string;
  updated: string;
}

export interface AttachmentReport {
  path: string;
  kind: "text" | "image" | "directory" | "binary";
  size: number;
  note: string | null;
}

/**
 * A prompt attachment: a path the daemon reads, or (for a daemon on another
 * machine) the file itself, with `path` only its name.
 */
export interface AttachmentInput {
  path: string;
  /** Base64; the daemon saves it on its own machine (20 MB at most). */
  data?: string;
}

export interface ImageInput {
  media_type: string;
  data: string;
  name?: string;
}

/** Any message from the daemon. */
export interface ServerMessage {
  type: string;
  client_ref?: string;
  [field: string]: unknown;
}

export interface UsageBucket extends Usage {
  sessions: number;
}

/** `GET /api/usage`. */
export interface UsageReport {
  days: number;
  since: string;
  today: UsageBucket;
  week: UsageBucket;
  total: UsageBucket;
  by_model: (UsageBucket & { model: string })[];
  by_day: (UsageBucket & { date: string })[];
}

export function emptyUsage(): Usage {
  return { input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_write_tokens: 0, cost: 0 };
}

export function totalTokens(usage: Partial<Usage> | null | undefined): number {
  if (!usage) return 0;
  return (usage.input_tokens ?? 0) + (usage.output_tokens ?? 0) + (usage.cache_read_tokens ?? 0) + (usage.cache_write_tokens ?? 0);
}

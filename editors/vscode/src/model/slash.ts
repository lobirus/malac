// Slash commands in the chat composer: the built-ins the chat runs itself,
// and the tab's custom commands (`tab.project.commands`), which go to the
// agent as typed and are expanded by its runtime.

import type { CommandInfo, Mode, ProjectSummary } from "../protocol";

export interface SlashCommand {
  name: string;
  description: string;
  hint: string;
  /** `malac` for built-ins, else where the command comes from. */
  source: string;
  /** Run by the chat instead of being sent to the agent. */
  builtin: boolean;
}

export const BUILTIN_COMMANDS: SlashCommand[] = [
  { name: "model", hint: "provider/model", description: "Switch the model", source: "malac", builtin: true },
  { name: "mode", hint: "ask|edits|autopilot", description: "Change the permission mode", source: "malac", builtin: true },
  { name: "compact", hint: "", description: "Summarize the conversation to free context", source: "malac", builtin: true },
  { name: "clear", hint: "", description: "Start a new agent", source: "malac", builtin: true },
  { name: "cost", hint: "", description: "Tokens and cost of this agent", source: "malac", builtin: true },
  // Run by the agent's runtime, but always there.
  { name: "context", hint: "", description: "Instructions, docs, index, skills, commands and plugins in use", source: "malac", builtin: false },
];

/** Built-ins first, then the tab's commands; a custom command never shadows a built-in. */
export function slashCommands(project: ProjectSummary | null | undefined): SlashCommand[] {
  const taken = new Set(BUILTIN_COMMANDS.map((c) => c.name));
  const custom = (project?.commands ?? [])
    .filter((c: CommandInfo) => c.name && !taken.has(c.name))
    .map((c: CommandInfo) => ({ name: c.name, description: c.description ?? "", hint: c.argument_hint ?? "", source: c.source || "project", builtin: false }));
  return [...BUILTIN_COMMANDS, ...custom];
}

/** `/name args`, as the daemon parses it: a name without `/` (so `/tmp/x` is a path). */
export function parseSlash(text: string): { name: string; args: string } | null {
  const match = /^\s*\/([A-Za-z0-9_:.-]+)(?:\s+([\s\S]*))?$/.exec(text);
  return match ? { name: match[1], args: (match[2] ?? "").trim() } : null;
}

export type BuiltinAction =
  | { kind: "model"; model: string }
  | { kind: "mode"; mode: Mode }
  | { kind: "compact" }
  | { kind: "clear" }
  | { kind: "cost" }
  | { kind: "error"; message: string };

const MODES: Mode[] = ["ask", "edits", "autopilot"];

/** What a typed built-in asks for, or null when the text is not one (it goes to the agent). */
export function builtinAction(text: string, models: string[]): BuiltinAction | null {
  const slash = parseSlash(text);
  const command = slash && BUILTIN_COMMANDS.find((c) => c.name === slash.name && c.builtin);
  if (!slash || !command) return null;
  switch (command.name) {
    case "model": {
      if (!slash.args) return { kind: "error", message: `Usage: /model provider/model (${models.join(", ")})` };
      const exact = models.find((m) => m === slash.args);
      const byName = models.filter((m) => m.split("/").slice(1).join("/") === slash.args);
      const model = exact ?? (byName.length === 1 ? byName[0] : null) ?? (slash.args.includes("/") ? slash.args : null);
      return model ? { kind: "model", model } : { kind: "error", message: `Unknown model ${slash.args}. Try one of: ${models.join(", ")}` };
    }
    case "mode": {
      const mode = MODES.find((m) => m === slash.args.toLowerCase());
      return mode ? { kind: "mode", mode } : { kind: "error", message: "Usage: /mode ask|edits|autopilot" };
    }
    case "compact":
      return { kind: "compact" };
    case "clear":
      return { kind: "clear" };
    default:
      return { kind: "cost" };
  }
}

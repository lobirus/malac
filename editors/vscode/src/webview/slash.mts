// The composer's "/" completion: which commands match what is typed.

export interface Completion {
  name: string;
  description: string;
  hint: string;
  source: string;
}

/** The command name being typed: the prompt starts with `/` and the caret is still in the name. */
export function slashQuery(beforeCaret: string): string | null {
  const match = /^\s*\/([A-Za-z0-9_:.-]*)$/.exec(beforeCaret);
  return match ? match[1] : null;
}

/**
 * Commands whose name starts with the query, or whose part after `plugin:`
 * does (as the daemon resolves `/name`), then those that contain it.
 * Built-ins keep their place before custom commands.
 */
export function matchCommands(commands: Completion[], query: string, limit = 30): Completion[] {
  const q = query.toLowerCase();
  const rank = (c: Completion): number | null => {
    const name = c.name.toLowerCase();
    const short = name.slice(name.lastIndexOf(":") + 1);
    if (!q || name.startsWith(q)) return 0;
    if (short.startsWith(q)) return 1;
    if (name.includes(q)) return 2;
    return null;
  };
  return commands
    .map((c, index) => ({ c, index, rank: rank(c) }))
    .filter((r): r is { c: Completion; index: number; rank: number } => r.rank !== null)
    .sort((a, b) => a.rank - b.rank || a.index - b.index)
    .slice(0, limit)
    .map((r) => r.c);
}

/** A short label for where a command comes from: `plugin x` → `x`. */
export function sourceLabel(source: string): string {
  if (source === "malac") return "";
  return source.startsWith("plugin ") ? source.slice("plugin ".length) : source;
}

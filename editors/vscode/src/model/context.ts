// The current-file context chip: it follows the active editor, shows the
// selection as `name:12-30`, can be turned off, and decides what a prompt
// carries: the file as an attachment, or the selected lines inline.

export interface Selection {
  /** Zero-based, as in vscode.Selection. */
  startLine: number;
  startCharacter: number;
  endLine: number;
  endCharacter: number;
}

/** What the extension knows about the active editor. */
export interface EditorSnapshot {
  /** Absolute path; editors without one (untitled, output) get no chip. */
  path: string | null;
  /** Shown in the chip's tooltip, e.g. relative to the workspace folder. */
  displayPath: string;
  languageId: string;
  selection: Selection | null;
  /** The selected text, when the selection is not empty. */
  selectedText: string;
  /** Unsaved changes: the file on disk differs from the editor. */
  dirty: boolean;
  /** The editor's full text, read only when needed. */
  text: () => string;
}

export interface LineRange {
  first: number;
  last: number;
}

export interface ContextChip {
  path: string;
  label: string;
  title: string;
  range: LineRange | null;
  enabled: boolean;
}

/** Inline content beyond this is cut. Matches the daemon's attachment limit. */
export const INLINE_LIMIT = 256 * 1024;

/** One-based and inclusive; a selection ending at column 0 leaves that line out. */
export function lineRange(selection: Selection | null): LineRange | null {
  if (!selection) return null;
  const empty = selection.startLine === selection.endLine && selection.startCharacter === selection.endCharacter;
  if (empty) return null;
  const first = selection.startLine + 1;
  const endsAtLineStart = selection.endCharacter === 0 && selection.endLine > selection.startLine;
  const last = endsAtLineStart ? selection.endLine : selection.endLine + 1;
  return { first, last: Math.max(first, last) };
}

export function rangeText(range: LineRange): string {
  return range.first === range.last ? `${range.first}` : `${range.first}-${range.last}`;
}

export function chipLabel(path: string, range: LineRange | null): string {
  const name = baseName(path);
  return range ? `${name}:${rangeText(range)}` : name;
}

/** The chip for the active editor, or null when it has no file. */
export function chipFor(editor: EditorSnapshot | null, enabled: boolean): ContextChip | null {
  if (!editor?.path) return null;
  const range = lineRange(editor.selection);
  const label = chipLabel(editor.path, range);
  const what = range ? `Lines ${rangeText(range)} of ${editor.displayPath}` : editor.displayPath;
  const title = enabled
    ? `${what} goes with your next message. Click to leave it out.`
    : `${what} is left out. Click to include it.`;
  return { path: editor.path, label, title, range, enabled };
}

export interface ContextPayload {
  attachments: { path: string }[];
  /** Appended to the prompt text: the selected lines, or the unsaved buffer. */
  inline: string;
}

/**
 * What an enabled chip adds to a prompt. A selection travels inline with its
 * path and line numbers; a whole saved file as an attachment, which the
 * daemon reads; a file with unsaved changes inline, so the agent sees what
 * the editor shows.
 */
export function contextPayload(chip: ContextChip | null, editor: EditorSnapshot | null): ContextPayload {
  const none = { attachments: [], inline: "" };
  if (!chip?.enabled || !editor?.path || editor.path !== chip.path) return none;
  if (chip.range) {
    const code = editor.selectedText.replace(/\r?\n$/, "");
    return { attachments: [], inline: fenced(`${editor.displayPath}, ${linesText(chip.range)}:`, code, editor.languageId) };
  }
  if (editor.dirty) {
    return { attachments: [], inline: fenced(`${editor.displayPath} (unsaved changes):`, editor.text(), editor.languageId) };
  }
  return { attachments: [{ path: editor.path }], inline: "" };
}

function linesText(range: LineRange): string {
  return range.first === range.last ? `line ${range.first}` : `lines ${range.first}-${range.last}`;
}

/** A heading line and a code fence long enough for the code inside it. */
export function fenced(heading: string, code: string, languageId: string): string {
  let body = code;
  let note = "";
  if (body.length > INLINE_LIMIT) {
    body = body.slice(0, INLINE_LIMIT);
    note = `\n(cut at ${INLINE_LIMIT / 1024} KB)`;
  }
  const fence = "`".repeat(Math.max(3, longestBacktickRun(body) + 1));
  return `${heading}\n\n${fence}${languageId}\n${body}\n${fence}${note}`;
}

/** The text to send: what was typed, then the context. */
export function withContext(text: string, inline: string): string {
  if (!inline) return text;
  return text ? `${text}\n\n${inline}` : inline;
}

function longestBacktickRun(text: string): number {
  return Math.max(0, ...(text.match(/`+/g) ?? []).map((run) => run.length));
}

function baseName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).at(-1) ?? path;
}

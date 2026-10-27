// Where malac's chat lives: a view in the right-hand panel (the secondary
// side bar, VS Code 1.106 and later; Cursor reports its VS Code base), or the
// activity-bar view on older versions, plus any number of editor tabs.

/** The first VS Code with extension containers in the secondary side bar. */
export const SECONDARY_SIDEBAR_SINCE = "1.106.0";

/** The side-bar chat view on each kind of VS Code. */
export const CHAT_VIEW = { secondary: "malac.chatSecondary", activityBar: "malac.chat" } as const;

/** `1.106.0-insider` → [1, 106, 0]; anything unreadable → null. */
export function parseVersion(version: string): [number, number, number] | null {
  const match = /^(\d+)\.(\d+)(?:\.(\d+))?/.exec(version.trim());
  return match ? [Number(match[1]), Number(match[2]), Number(match[3] ?? 0)] : null;
}

export function versionAtLeast(version: string, minimum: string): boolean {
  const have = parseVersion(version);
  const want = parseVersion(minimum);
  if (!have || !want) return false;
  for (let i = 0; i < 3; i += 1) if (have[i] !== want[i]) return have[i] > want[i];
  return true;
}

/** Whether `vscode.version` can put malac's chat in the right-hand panel. Unknown versions get the activity bar. */
export function supportsSecondarySidebar(vscodeVersion: string): boolean {
  return versionAtLeast(vscodeVersion, SECONDARY_SIDEBAR_SINCE);
}

export function chatViewId(vscodeVersion: string): string {
  return supportsSecondarySidebar(vscodeVersion) ? CHAT_VIEW.secondary : CHAT_VIEW.activityBar;
}

export interface SurfaceInfo {
  id: string;
  kind: "side" | "tab";
  open: boolean;
  visible: boolean;
}

/**
 * The chat that "malac: Open" brings back: the editor tab used last while
 * it is still open, else the side-bar chat.
 */
export function lastUsedSurface(surfaces: SurfaceInfo[], lastActive: string | null): string {
  const side = surfaces.find((s) => s.kind === "side")?.id ?? CHAT_VIEW.secondary;
  const last = surfaces.find((s) => s.id === lastActive && s.open);
  return last?.id ?? side;
}

/** An editor tab's title: its agent's name, short enough for a tab. */
export function tabTitle(agent: { name: string } | null | undefined, max = 32): string {
  const name = agent?.name?.replace(/\s+/g, " ").trim();
  if (!name) return "malac";
  return name.length > max ? `${name.slice(0, max - 1)}…` : name;
}

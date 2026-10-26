// Which tab the editor's folder should use. A folder inside a larger git
// repository is better served by a tab at the repository root, so the agent
// can work on the full stack (the monorepo recommendation).

import * as fs from "node:fs";
import * as path from "node:path";

export type FolderChoice = "folder" | "root" | "dismissed";

export type FolderPlan =
  /** A matching tab exists; show it. */
  | { kind: "select"; tab: string }
  /** No tab yet: ask whether to open one (and where). */
  | { kind: "offer"; folder: string; root: string | null }
  /** Open a tab without asking (a remembered or configured choice). */
  | { kind: "create"; cwd: string }
  /** The folder has a tab, but the repository root would be better. */
  | { kind: "suggestRoot"; tab: string; root: string }
  | { kind: "none" };

export type FolderSetting = "ask" | "folder" | "repositoryRoot" | "never";

/** The nearest directory at or above `start` holding `.git` (a directory or a worktree file). */
export function findGitRoot(start: string, exists: (p: string) => boolean = fs.existsSync): string | null {
  let dir = path.resolve(start);
  for (let i = 0; i < 64; i += 1) {
    if (exists(path.join(dir, ".git"))) return dir;
    const parent = path.dirname(dir);
    if (parent === dir) return null;
    dir = parent;
  }
  return null;
}

/** The repository root when it is above `folder`, else null. */
export function largerRoot(folder: string, root: string | null): string | null {
  if (!root) return null;
  const a = trim(path.resolve(folder));
  const b = trim(path.resolve(root));
  return a !== b && a.startsWith(`${b}${path.sep}`) ? b : null;
}

export function planFolder({
  folder,
  root,
  tabs,
  remembered,
  setting = "ask",
}: {
  folder: string;
  /** The repository root, when it is above the folder. */
  root: string | null;
  tabs: { id: string; cwd: string }[];
  remembered?: FolderChoice;
  setting?: FolderSetting;
}): FolderPlan {
  const find = (dir: string) => tabs.find((t) => trim(t.cwd) === trim(dir));
  const rootTab = root ? find(root) : undefined;
  const folderTab = find(folder);
  const preferRoot = Boolean(root) && (remembered === "root" || setting === "repositoryRoot");
  const preferFolder = remembered === "folder" || setting === "folder";
  if (preferFolder && !folderTab) return setting === "never" ? { kind: "none" } : { kind: "create", cwd: folder };
  if (rootTab && (preferRoot || !folderTab)) return { kind: "select", tab: rootTab.id };
  if (folderTab) {
    if (root && !preferFolder && !preferRoot && remembered !== "dismissed" && setting === "ask") {
      return { kind: "suggestRoot", tab: folderTab.id, root };
    }
    return { kind: "select", tab: folderTab.id };
  }
  if (setting === "never" || remembered === "dismissed") return { kind: "none" };
  if (preferRoot && root) return { kind: "create", cwd: root };
  if (preferFolder) return { kind: "create", cwd: folder };
  return { kind: "offer", folder, root };
}

function trim(dir: string): string {
  return dir.replace(/[\\/]+$/, "") || dir;
}

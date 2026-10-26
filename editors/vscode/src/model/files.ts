// Ranking workspace files for the composer's "@" picker.

export interface FileHit {
  path: string;
  /** Relative to the workspace folder, for display. */
  relative: string;
}

/**
 * Scores `relative` against `query` (lower is better), or returns null when
 * the query's characters do not all appear in order. Matches in the file
 * name beat matches in the directories; contiguous and early matches win.
 */
export function scorePath(relative: string, query: string): number | null {
  const q = query.toLowerCase().replace(/\s+/g, "");
  if (!q) return relative.split("/").length * 10 + relative.length / 100;
  const text = relative.toLowerCase();
  const name = text.slice(text.lastIndexOf("/") + 1);
  const exactName = name === q ? -1000 : name.startsWith(q) ? -500 : name.includes(q) ? -250 : 0;
  if (exactName) return exactName + name.length + text.length / 100;
  if (text.includes(q)) return 100 + text.indexOf(q) + text.length / 100;
  // Subsequence match with a penalty for gaps.
  let position = 0;
  let gaps = 0;
  let last = -1;
  for (const char of q) {
    const found = text.indexOf(char, position);
    if (found < 0) return null;
    if (last >= 0) gaps += found - last - 1;
    last = found;
    position = found + 1;
  }
  return 1000 + gaps + text.length / 100;
}

export function rankFiles(files: FileHit[], query: string, limit = 30, recent: string[] = []): FileHit[] {
  const recentRank = new Map(recent.map((path, index) => [path, index]));
  const scored: { hit: FileHit; score: number }[] = [];
  for (const hit of files) {
    const score = scorePath(hit.relative, query);
    if (score === null) continue;
    const boost = recentRank.has(hit.path) ? 50 - Math.min(40, recentRank.get(hit.path)!) : 0;
    scored.push({ hit, score: score - boost });
  }
  scored.sort((a, b) => a.score - b.score || a.hit.relative.localeCompare(b.hit.relative));
  return scored.slice(0, limit).map((s) => s.hit);
}

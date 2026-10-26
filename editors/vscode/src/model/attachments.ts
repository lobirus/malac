// Prompt attachments for the daemon. A daemon on this machine reads files by
// path. A daemon on another machine (a saved connection, or any non-loopback
// address) cannot, so the extension sends the files themselves: `path` is
// then only the file name and `data` the contents in base64.

import * as fs from "node:fs/promises";
import * as path from "node:path";
import type { AttachmentInput } from "../protocol";

/** The daemon's cap for an uploaded attachment. */
export const REMOTE_ATTACHMENT_LIMIT = 20 * 1024 * 1024;

const LOOPBACK = new Set(["127.0.0.1", "localhost", "[::1]", "::1"]);

export function isRemoteEndpoint(endpoint: { url: string; source?: string } | null | undefined): boolean {
  if (!endpoint) return false;
  if (endpoint.source === "remote") return true;
  try {
    const host = new URL(endpoint.url).hostname.toLowerCase();
    return !(LOOPBACK.has(host) || host.endsWith(".localhost") || /^127\./.test(host));
  } catch {
    return false;
  }
}

export interface FileAccess {
  stat(p: string): Promise<{ size: number; isDirectory(): boolean }>;
  read(p: string): Promise<Buffer>;
}

const nodeFiles: FileAccess = { stat: (p) => fs.stat(p), read: (p) => fs.readFile(p) };

/** A name not used yet: `index.ts`, then `index (2).ts`. */
function uniqueName(name: string, used: Set<string>): string {
  let candidate = name;
  const ext = path.extname(name);
  const stem = name.slice(0, name.length - ext.length);
  for (let n = 2; used.has(candidate); n += 1) candidate = `${stem} (${n})${ext}`;
  used.add(candidate);
  return candidate;
}

/**
 * The attachments to send for `paths`. Local daemons get the paths. Remote
 * ones get each file's contents; a folder or a file over the cap fails the
 * whole prompt, as an unreadable attachment does on the daemon.
 */
export async function attachmentInputs(paths: string[], remote: boolean, files: FileAccess = nodeFiles): Promise<AttachmentInput[]> {
  const unique = [...new Set(paths)];
  if (!remote) return unique.map((p) => ({ path: p }));
  const used = new Set<string>();
  const out: AttachmentInput[] = [];
  for (const file of unique) {
    const name = path.basename(file) || "file";
    let stat: { size: number; isDirectory(): boolean };
    try {
      stat = await files.stat(file);
    } catch {
      throw new Error(`Cannot read ${name}.`);
    }
    if (stat.isDirectory()) throw new Error(`${name} is a folder; only files can be sent to a daemon on another machine.`);
    if (stat.size > REMOTE_ATTACHMENT_LIMIT) {
      throw new Error(`${name} is larger than ${REMOTE_ATTACHMENT_LIMIT / 1024 / 1024} MB, the limit for a daemon on another machine.`);
    }
    out.push({ path: uniqueName(name, used), data: (await files.read(file)).toString("base64") });
  }
  return out;
}

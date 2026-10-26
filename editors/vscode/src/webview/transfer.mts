// Turning drops and pastes into prompt attachments. Pure, so it is tested in
// Node: the webview passes in what it read from the DataTransfer.
//
// - Files from the Explorer (and from Finder, when the drag carries file
//   URLs) arrive as URI lists and become `attachments: [{path}]`.
// - Images without a path (screenshots, pasted images) become `images`.
// - Other files without a path are uploaded to the extension, which saves
//   them and attaches the saved copy.

export const IMAGE_TYPES = new Set(["image/png", "image/jpeg", "image/gif", "image/webp"]);
export const MAX_IMAGE_BYTES = 10 * 1024 * 1024;
export const MAX_UPLOAD_BYTES = 10 * 1024 * 1024;

export interface TransferFile {
  name: string;
  type: string;
  size: number;
}

export interface TransferInput {
  /** `application/vnd.code.uri-list`, `text/uri-list`, … in that order. */
  uriLists: string[];
  files: TransferFile[];
}

export interface Converted<F extends TransferFile = TransferFile> {
  paths: string[];
  images: F[];
  uploads: F[];
  rejected: { name: string; reason: string }[];
}

/** File paths in a `text/uri-list` (comments and other schemes are skipped). */
export function parseUriList(text: string): string[] {
  const paths: string[] = [];
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line || line.startsWith("#")) continue;
    const path = uriToPath(line);
    if (path && !paths.includes(path)) paths.push(path);
  }
  return paths;
}

/** The local path of a `file:` (or `vscode-remote:`) URI, or null. */
export function uriToPath(uri: string): string | null {
  const match = /^(file|vscode-remote):\/\/([^/]*)(\/.*)?$/i.exec(uri.trim());
  if (!match) return null;
  let path: string;
  try {
    path = decodeURIComponent(match[3] ?? "/");
  } catch {
    return null;
  }
  // file:///c:/x → c:/x on Windows.
  if (/^\/[a-zA-Z]:[\\/]/.test(path)) path = path.slice(1);
  const host = match[2];
  if (match[1].toLowerCase() === "file" && host && host !== "localhost") return `//${host}${path}`;
  return path;
}

export function convertTransfer<F extends TransferFile>(input: { uriLists: string[]; files: F[] }): Converted<F> {
  const result: Converted<F> = { paths: [], images: [], uploads: [], rejected: [] };
  for (const list of input.uriLists) {
    for (const path of parseUriList(list)) if (!result.paths.includes(path)) result.paths.push(path);
  }
  const named = new Set(result.paths.map((p) => baseName(p)));
  for (const file of input.files) {
    // The same item, already attached by its path.
    if (named.has(file.name)) continue;
    if (IMAGE_TYPES.has(file.type)) {
      if (file.size > MAX_IMAGE_BYTES) result.rejected.push({ name: file.name, reason: `larger than ${MAX_IMAGE_BYTES / 1024 / 1024} MB` });
      else result.images.push(file);
    } else if (file.type.startsWith("image/")) {
      result.rejected.push({ name: file.name, reason: "only PNG, JPEG, GIF and WebP images can be attached" });
    } else if (file.size > MAX_UPLOAD_BYTES) {
      result.rejected.push({ name: file.name, reason: `larger than ${MAX_UPLOAD_BYTES / 1024 / 1024} MB` });
    } else {
      result.uploads.push(file);
    }
  }
  return result;
}

/** A name for a pasted image without one, e.g. `Pasted image 14.03.12.png`. */
export function pastedImageName(type: string, now: Date, existing = ""): string {
  if (existing && existing !== "image.png") return existing;
  const ext = type === "image/jpeg" ? "jpg" : (type.split("/")[1] ?? "png");
  const pad = (n: number) => String(n).padStart(2, "0");
  return `Pasted image ${pad(now.getHours())}.${pad(now.getMinutes())}.${pad(now.getSeconds())}.${ext}`;
}

/** Base64 of an ArrayBuffer, in chunks so large images do not overflow the stack. */
export function toBase64(buffer: ArrayBuffer): string {
  const bytes = new Uint8Array(buffer);
  let binary = "";
  for (let i = 0; i < bytes.length; i += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  }
  return btoa(binary);
}

/** Whether a drag carries anything the composer accepts. */
export function acceptsDrag(types: readonly string[]): boolean {
  return types.some((t) => t === "Files" || t === "text/uri-list" || t === "application/vnd.code.uri-list" || t === "resourceurls" || t === "codefiles");
}

/** VS Code's own drag formats hold JSON arrays of URIs or paths; normalize them to URI lists. */
export function codeDragToUriList(type: string, data: string): string {
  if (!data) return "";
  if (type === "resourceurls" || type === "codefiles") {
    try {
      const items = JSON.parse(data) as unknown[];
      return items
        .map((item) => (typeof item === "string" ? (item.includes("://") ? item : `file://${encodeURI(item)}`) : ""))
        .filter(Boolean)
        .join("\n");
    } catch {
      return "";
    }
  }
  return data;
}

function baseName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).at(-1) ?? path;
}

import assert from "node:assert/strict";
import { test } from "node:test";
import { acceptsDrag, codeDragToUriList, convertTransfer, MAX_IMAGE_BYTES, parseUriList, pastedImageName, toBase64, uriToPath } from "../src/webview/transfer.mjs";

const file = (name: string, type: string, size = 100) => ({ name, type, size });

test("file URIs become paths", () => {
  assert.equal(uriToPath("file:///Users/me/My%20Project/a.ts"), "/Users/me/My Project/a.ts");
  assert.equal(uriToPath("file:///c%3A/src/main.rs"), "c:/src/main.rs");
  assert.equal(uriToPath("file:///C:/src/main.rs"), "C:/src/main.rs");
  assert.equal(uriToPath("file://localhost/etc/hosts"), "/etc/hosts");
  assert.equal(uriToPath("file://server/share/x"), "//server/share/x", "UNC paths keep their host");
  assert.equal(uriToPath("vscode-remote://ssh-remote%2Bbox/home/me/a.py"), "/home/me/a.py");
  assert.equal(uriToPath("https://example.com/a"), null);
  assert.equal(uriToPath("untitled:Untitled-1"), null);
  assert.equal(uriToPath("file:///bad%E0%A4%A"), null);
});

test("URI lists skip comments, blank lines and duplicates", () => {
  assert.deepEqual(parseUriList("# dragged from VS Code\r\nfile:///a/b.ts\r\n\r\nfile:///a/b.ts\nfile:///a/dir\nhttps://x"), ["/a/b.ts", "/a/dir"]);
});

test("Explorer drops become attachments by path", () => {
  const result = convertTransfer({ uriLists: ["file:///p/src/app.ts\nfile:///p/docs"], files: [] });
  assert.deepEqual(result.paths, ["/p/src/app.ts", "/p/docs"]);
  assert.deepEqual([result.images, result.uploads, result.rejected], [[], [], []]);
});

test("a Finder drop that carries file URLs is not attached twice", () => {
  const result = convertTransfer({ uriLists: ["file:///Users/me/Desktop/shot.png"], files: [file("shot.png", "image/png")] });
  assert.deepEqual(result.paths, ["/Users/me/Desktop/shot.png"]);
  assert.deepEqual(result.images, []);
});

test("images without a path become images; other files are uploaded", () => {
  const result = convertTransfer({
    uriLists: [],
    files: [file("Screenshot.png", "image/png"), file("photo.jpg", "image/jpeg"), file("notes.txt", "text/plain"), file("data.bin", "")],
  });
  assert.deepEqual(result.images.map((f) => f.name), ["Screenshot.png", "photo.jpg"]);
  assert.deepEqual(result.uploads.map((f) => f.name), ["notes.txt", "data.bin"]);
});

test("unsupported and oversized files are rejected with a reason", () => {
  const result = convertTransfer({ uriLists: [], files: [file("vector.svg", "image/svg+xml"), file("huge.png", "image/png", MAX_IMAGE_BYTES + 1), file("big.zip", "application/zip", 50 * 1024 * 1024)] });
  assert.deepEqual(result.rejected.map((r) => r.name), ["vector.svg", "huge.png", "big.zip"]);
  assert.match(result.rejected[0].reason, /PNG, JPEG, GIF and WebP/);
  assert.deepEqual([result.images, result.uploads], [[], []]);
});

test("pasted images get a readable name", () => {
  const now = new Date(2026, 9, 26, 14, 3, 7);
  assert.equal(pastedImageName("image/png", now), "Pasted image 14.03.07.png");
  assert.equal(pastedImageName("image/jpeg", now, "image.png"), "Pasted image 14.03.07.jpg", "the browser's generic name is replaced");
  assert.equal(pastedImageName("image/png", now, "diagram.png"), "diagram.png");
});

test("VS Code's own drag formats are read as URI lists", () => {
  assert.equal(codeDragToUriList("resourceurls", JSON.stringify(["file:///a/b.ts"])), "file:///a/b.ts");
  assert.equal(codeDragToUriList("codefiles", JSON.stringify(["/a/my file.ts"])), "file:///a/my%20file.ts");
  assert.equal(codeDragToUriList("codefiles", "not json"), "");
  assert.ok(acceptsDrag(["Files"]));
  assert.ok(acceptsDrag(["application/vnd.code.uri-list", "text/plain"]));
  assert.ok(!acceptsDrag(["text/plain"]), "dragging text selections is left alone");
});

test("base64 encoding handles large buffers", () => {
  const bytes = new Uint8Array(100_000).map((_, i) => i % 256);
  assert.equal(toBase64(bytes.buffer), Buffer.from(bytes).toString("base64"));
});

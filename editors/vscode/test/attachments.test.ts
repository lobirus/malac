import assert from "node:assert/strict";
import { test } from "node:test";
import { attachmentInputs, FileAccess, isRemoteEndpoint, REMOTE_ATTACHMENT_LIMIT } from "../src/model/attachments";

test("a daemon is remote when it is a saved connection or not on loopback", () => {
  assert.equal(isRemoteEndpoint({ url: "http://127.0.0.1:7700", source: "daemon.json" }), false);
  assert.equal(isRemoteEndpoint({ url: "http://localhost:7700" }), false);
  assert.equal(isRemoteEndpoint({ url: "http://[::1]:7700" }), false);
  assert.equal(isRemoteEndpoint({ url: "http://127.0.0.1:7790", source: "remote" }), true, "a saved connection, even through a local tunnel");
  assert.equal(isRemoteEndpoint({ url: "https://x.trycloudflare.com" }), true);
  assert.equal(isRemoteEndpoint({ url: "http://192.168.1.20:7700", source: "daemon.json" }), true);
  assert.equal(isRemoteEndpoint(null), false);
});

function files(table: Record<string, string | "dir" | number>): FileAccess {
  return {
    async stat(p) {
      if (!(p in table)) throw new Error("ENOENT");
      const value = table[p];
      return { size: typeof value === "number" ? value : value === "dir" ? 0 : Buffer.byteLength(value), isDirectory: () => value === "dir" };
    },
    async read(p) {
      return Buffer.from(String(table[p]));
    },
  };
}

test("a local daemon gets paths", async () => {
  assert.deepEqual(await attachmentInputs(["/a/x.ts", "/a/x.ts", "/a/dir"], false, files({})), [{ path: "/a/x.ts" }, { path: "/a/dir" }]);
});

test("a remote daemon gets the files, named by their file name", async () => {
  const fs = files({ "/p/src/index.ts": "one", "/p/web/index.ts": "two", "/p/README.md": "# hi" });
  assert.deepEqual(await attachmentInputs(["/p/src/index.ts", "/p/web/index.ts", "/p/README.md"], true, fs), [
    { path: "index.ts", data: Buffer.from("one").toString("base64") },
    { path: "index (2).ts", data: Buffer.from("two").toString("base64") },
    { path: "README.md", data: Buffer.from("# hi").toString("base64") },
  ]);
});

test("folders, missing files and files over 20 MB fail the prompt", async () => {
  await assert.rejects(attachmentInputs(["/p/src"], true, files({ "/p/src": "dir" })), /src is a folder/);
  await assert.rejects(attachmentInputs(["/p/big.bin"], true, files({ "/p/big.bin": REMOTE_ATTACHMENT_LIMIT + 1 })), /larger than 20 MB/);
  await assert.rejects(attachmentInputs(["/p/gone.txt"], true, files({})), /Cannot read gone\.txt/);
});

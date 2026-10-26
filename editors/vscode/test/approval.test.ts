import assert from "node:assert/strict";
import { test } from "node:test";
import { describeApproval } from "../src/model/approval";

const contents = "deploy 1.2.0: ship things\n  MCP servers (start programs): ship (node server.js)\n  hooks (run commands): PostToolUse: ./hooks/lint.sh";

test("enabling a plugin shows what it would run", () => {
  const text = describeApproval("malac_configure", "enable plugin deploy@acme, which runs hooks, MCP servers", { plugin: "deploy@acme", contents });
  assert.equal(text.title, "Enable plugin deploy@acme, which runs hooks, MCP servers");
  assert.equal(text.details, contents);
  assert.equal(text.plugin, "deploy@acme");
  assert.equal(describeApproval("malac_configure", "", { plugin: "x" }).title, "Enable plugin x");
  assert.equal(describeApproval("malac_configure", "", { plugin: "x" }).details, null);
});

test("other requests: long commands and inputs go behind the details", () => {
  assert.equal(describeApproval("bash", "ls", { command: "ls" }).details, null, "a short command is in the title already");
  assert.equal(describeApproval("bash", "build", { command: "cargo build\ncargo test" }).details, "cargo build\ncargo test");
  const edit = describeApproval("edit", "src/a.ts", { path: "src/a.ts", old_string: "a", new_string: "b" });
  assert.equal(edit.title, "edit: src/a.ts");
  assert.match(edit.details!, /"old_string": "a"/);
  assert.equal(edit.plugin, null);
  assert.equal(describeApproval("malac_configure", "set model", { action: "set", key: "model" }).plugin, null, "only plugin requests are plugin requests");
  assert.equal(describeApproval("x", "", {}).details, null);
});

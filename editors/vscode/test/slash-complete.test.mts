import assert from "node:assert/strict";
import { test } from "node:test";
import { matchCommands, slashQuery, sourceLabel } from "../src/webview/slash.mjs";

const commands = [
  { name: "model", description: "", hint: "provider/model", source: "malac" },
  { name: "mode", description: "", hint: "", source: "malac" },
  { name: "compact", description: "", hint: "", source: "malac" },
  { name: "review", description: "Review", hint: "[pr]", source: "project" },
  { name: "deploy:ship", description: "Ship", hint: "", source: "plugin deploy" },
  { name: "remodel", description: "", hint: "", source: "user" },
];

test("the query is the command name typed at the start of the prompt", () => {
  assert.equal(slashQuery("/"), "");
  assert.equal(slashQuery("  /mo"), "mo");
  assert.equal(slashQuery("/deploy:sh"), "deploy:sh");
  assert.equal(slashQuery("/model "), null, "past the name, arguments are typed");
  assert.equal(slashQuery("fix /mo"), null);
  assert.equal(slashQuery("/tmp/x"), null);
});

test("prefix matches first, then the part after plugin:, then substrings; order kept", () => {
  assert.deepEqual(matchCommands(commands, "mo").map((c) => c.name), ["model", "mode", "remodel"]);
  assert.deepEqual(matchCommands(commands, "sh").map((c) => c.name), ["deploy:ship"]);
  assert.deepEqual(matchCommands(commands, "").map((c) => c.name), commands.map((c) => c.name));
  assert.deepEqual(matchCommands(commands, "zz"), []);
  assert.equal(matchCommands(commands, "", 2).length, 2);
});

test("sources read short", () => {
  assert.equal(sourceLabel("malac"), "");
  assert.equal(sourceLabel("plugin deploy"), "deploy");
  assert.equal(sourceLabel("project"), "project");
});

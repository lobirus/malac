import assert from "node:assert/strict";
import { test } from "node:test";
import { BUILTIN_COMMANDS, builtinAction, parseSlash, slashCommands } from "../src/model/slash";

const project = {
  root: "/repo",
  commands: [
    { name: "review", description: "Review the diff", argument_hint: "[pr]", source: "project" },
    { name: "deploy:ship", description: "Ship it", source: "plugin deploy" },
    { name: "compact", description: "A custom compact", source: "user" },
  ],
};

test("built-ins come first, then the tab's commands; built-ins are not shadowed", () => {
  const commands = slashCommands(project);
  assert.deepEqual(commands.slice(0, BUILTIN_COMMANDS.length).map((c) => c.name), ["model", "mode", "compact", "clear", "cost", "context"]);
  assert.deepEqual(commands.slice(BUILTIN_COMMANDS.length).map((c) => [c.name, c.hint, c.source, c.builtin]), [
    ["review", "[pr]", "project", false],
    ["deploy:ship", "", "plugin deploy", false],
  ]);
  assert.deepEqual(slashCommands(null).map((c) => c.name), BUILTIN_COMMANDS.map((c) => c.name), "a tab not loaded yet has the built-ins");
});

test("slash parsing matches the daemon's: a path is not a command", () => {
  assert.deepEqual(parseSlash("/review 42 please"), { name: "review", args: "42 please" });
  assert.deepEqual(parseSlash("  /deploy:ship"), { name: "deploy:ship", args: "" });
  assert.equal(parseSlash("/tmp/x is full"), null);
  assert.equal(parseSlash("look at /review"), null);
  assert.equal(parseSlash("/"), null);
});

test("built-ins become actions; everything else goes to the agent", () => {
  const models = ["lora/default", "lora/smart", "other/smart"];
  assert.deepEqual(builtinAction("/model lora/smart", models), { kind: "model", model: "lora/smart" });
  assert.deepEqual(builtinAction("/model default", models), { kind: "model", model: "lora/default" }, "a unique short name");
  assert.equal(builtinAction("/model smart", models)?.kind, "error", "an ambiguous short name");
  assert.deepEqual(builtinAction("/model new/one", models), { kind: "model", model: "new/one" }, "any provider/model is passed on");
  assert.equal(builtinAction("/model", models)?.kind, "error");
  assert.deepEqual(builtinAction("/mode Autopilot", models), { kind: "mode", mode: "autopilot" });
  assert.equal(builtinAction("/mode yolo", models)?.kind, "error");
  assert.deepEqual(builtinAction("/compact", models), { kind: "compact" });
  assert.deepEqual(builtinAction("/clear", models), { kind: "clear" });
  assert.deepEqual(builtinAction("/cost", models), { kind: "cost" });
  assert.equal(builtinAction("/context", models), null, "the runtime answers /context");
  assert.equal(builtinAction("/review 42", models), null, "custom commands are expanded by the daemon");
  assert.equal(builtinAction("fix /model", models), null);
});

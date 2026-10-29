import assert from "node:assert/strict";
import { test } from "node:test";
import { groupModels, splitModel } from "../src/model/models";
import { Change, DaemonStore } from "../src/model/store";
import type { ServerMessage } from "../src/protocol";

test("models group under their provider, the part before the first slash", () => {
  assert.deepEqual(splitModel("openrouter/anthropic/claude-sonnet"), ["openrouter", "anthropic/claude-sonnet"]);
  assert.deepEqual(splitModel("system"), ["", "system"]);
  const groups = groupModels(["lora/fast", "openrouter/anthropic/claude-sonnet", "lora/smart", "lora/fast", "system", "openrouter/x"]);
  assert.deepEqual(
    groups.map((g) => [g.provider, g.models.map((m) => m.name)]),
    [
      ["lora", ["fast", "smart"]],
      ["openrouter", ["anthropic/claude-sonnet", "x"]],
      ["", ["system"]],
    ],
    "providers in the order they first appear, each model once",
  );
  assert.equal(groups[1].models[0].spec, "openrouter/anthropic/claude-sonnet");
  assert.deepEqual(groupModels([]), []);
});

test("a models broadcast replaces the model list and the default at once", () => {
  const store = new DaemonStore();
  const changes: Change[] = [];
  store.onChange.event((c) => changes.push(c));
  store.apply({ type: "snapshot", daemon: { version: "1.2.1", pid: 1, started: "", version_mismatch: false, clients: 1, attached: false, remote: false }, workspaces: [], workspace: null, pending: [], jobs: [], models: ["lora/fast"], default_model: "lora/fast" } as ServerMessage);
  assert.ok(changes.at(-1)!.models, "a snapshot brings models too");
  store.apply({ type: "models", models: ["lora/fast", "openrouter/x"], default_model: "openrouter/x" });
  assert.deepEqual(store.models, ["lora/fast", "openrouter/x"]);
  assert.equal(store.defaultModel, "openrouter/x");
  assert.deepEqual(changes.at(-1), { models: true });
  store.apply({ type: "models", models: [], default_model: null });
  assert.deepEqual([store.models, store.defaultModel], [[], null], "the last provider removed");
});

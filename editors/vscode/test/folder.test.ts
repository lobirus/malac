import assert from "node:assert/strict";
import * as path from "node:path";
import { test } from "node:test";
import { findGitRoot, largerRoot, planFolder } from "../src/model/folder";

const tabs = (...dirs: string[]) => dirs.map((cwd, i) => ({ id: `t${i}`, cwd }));

test("the git root is the nearest directory with .git", () => {
  const exists = (p: string) => p === path.join("/repo", ".git");
  assert.equal(findGitRoot("/repo/apps/web", exists), "/repo");
  assert.equal(findGitRoot("/elsewhere/x", exists), null);
  assert.equal(largerRoot("/repo/apps/web", "/repo"), "/repo");
  assert.equal(largerRoot("/repo", "/repo"), null, "the folder is the root");
  assert.equal(largerRoot("/repo/", "/repo"), null);
  assert.equal(largerRoot("/repository", "/repo"), null, "a prefix is not a parent");
});

test("an existing tab for the folder is shown", () => {
  assert.deepEqual(planFolder({ folder: "/repo", root: null, tabs: tabs("/other", "/repo/") }), { kind: "select", tab: "t1" });
});

test("without a tab, offer one, with the repository root when the folder is inside one", () => {
  assert.deepEqual(planFolder({ folder: "/repo/apps/web", root: "/repo", tabs: [] }), { kind: "offer", folder: "/repo/apps/web", root: "/repo" });
  assert.deepEqual(planFolder({ folder: "/solo", root: null, tabs: [] }), { kind: "offer", folder: "/solo", root: null });
});

test("a folder tab inside a larger repository suggests the root (the monorepo case)", () => {
  assert.deepEqual(planFolder({ folder: "/repo/apps/web", root: "/repo", tabs: tabs("/repo/apps/web") }), { kind: "suggestRoot", tab: "t0", root: "/repo" });
  // Once answered, it is not asked again.
  assert.deepEqual(planFolder({ folder: "/repo/apps/web", root: "/repo", tabs: tabs("/repo/apps/web"), remembered: "folder" }), { kind: "select", tab: "t0" });
});

test("a tab at the repository root is used for a folder below it", () => {
  assert.deepEqual(planFolder({ folder: "/repo/apps/web", root: "/repo", tabs: tabs("/repo") }), { kind: "select", tab: "t0" });
  assert.deepEqual(planFolder({ folder: "/repo/apps/web", root: "/repo", tabs: tabs("/repo/apps/web", "/repo"), remembered: "root" }), { kind: "select", tab: "t1" });
});

test("remembered choices and settings open tabs without asking", () => {
  assert.deepEqual(planFolder({ folder: "/repo/apps/web", root: "/repo", tabs: [], remembered: "root" }), { kind: "create", cwd: "/repo" });
  assert.deepEqual(planFolder({ folder: "/repo/apps/web", root: "/repo", tabs: [], setting: "repositoryRoot" }), { kind: "create", cwd: "/repo" });
  assert.deepEqual(planFolder({ folder: "/repo/apps/web", root: "/repo", tabs: tabs("/repo"), setting: "folder" }), { kind: "create", cwd: "/repo/apps/web" });
  assert.deepEqual(planFolder({ folder: "/solo", root: null, tabs: [], remembered: "dismissed" }), { kind: "none" });
  assert.deepEqual(planFolder({ folder: "/solo", root: null, tabs: [], setting: "never" }), { kind: "none" });
});

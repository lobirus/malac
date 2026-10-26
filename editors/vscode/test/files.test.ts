import assert from "node:assert/strict";
import { test } from "node:test";
import { rankFiles, scorePath } from "../src/model/files";

const files = ["src/cart.ts", "src/checkout/cart-view.tsx", "docs/cart.md", "README.md", "web/index.html", "src/very/deep/thing/carts.rs"].map((relative) => ({ path: `/p/${relative}`, relative }));

test("file names beat directories, exact beats prefix beats substring", () => {
  assert.deepEqual(rankFiles(files, "cart.ts").map((f) => f.relative)[0], "src/cart.ts");
  assert.deepEqual(rankFiles(files, "cart").map((f) => f.relative).slice(0, 3), ["src/cart.ts", "docs/cart.md", "src/very/deep/thing/carts.rs"]);
  assert.deepEqual(rankFiles(files, "readme").map((f) => f.relative), ["README.md"]);
});

test("characters in order match across the path; others do not", () => {
  assert.ok(scorePath("src/checkout/cart-view.tsx", "chkcv") !== null);
  assert.equal(scorePath("README.md", "xyz"), null);
  assert.deepEqual(rankFiles(files, "zzz"), []);
});

test("an empty query lists shallow files first and honours the limit", () => {
  assert.deepEqual(rankFiles(files, "", 2).map((f) => f.relative), ["README.md", "src/cart.ts"]);
});

test("recently open files are boosted", () => {
  assert.equal(rankFiles(files, "", 1, ["/p/web/index.html"])[0].relative, "web/index.html");
});

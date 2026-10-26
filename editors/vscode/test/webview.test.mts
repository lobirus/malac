import assert from "node:assert/strict";
import { test } from "node:test";
import { inline, renderMarkdown } from "../src/webview/markdown.mjs";
import { diffLines, splitContext } from "../src/webview/transcript.mjs";

test("markdown escapes model output", () => {
  const html = renderMarkdown('<img src=x onerror="alert(1)"> **bold** `<b>` [x](javascript:alert(1)) [ok](https://malac.io)');
  assert.ok(!html.includes("<img"));
  assert.match(html, /&lt;img src=x onerror=&quot;alert\(1\)&quot;&gt;/);
  assert.match(html, /<strong>bold<\/strong>/);
  assert.match(html, /<code>&lt;b&gt;<\/code>/);
  assert.ok(!html.includes('href="javascript'), "only http(s) and mailto links");
  assert.match(html, /<a href="https:\/\/malac\.io"/);
});

test("markdown blocks: code, lists, tasks, tables, quotes", () => {
  const html = renderMarkdown("# Title\n\n- one\n- [x] two\n\n1. first\n2. second\n\n```ts\nconst a = 1 < 2;\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n> quoted");
  assert.match(html, /<h3>Title<\/h3>/);
  assert.match(html, /<ul><li>one<\/li><li class="task-item"><span class="task done"/);
  assert.match(html, /<ol><li>first<\/li><li>second<\/li><\/ol>/);
  assert.match(html, /data-lang="ts".*<code>const a = 1 &lt; 2;<\/code>/s);
  assert.match(html, /<th>a<\/th><th>b<\/th>.*<td>1<\/td>/s);
  assert.match(html, /<blockquote><p>quoted<\/p><\/blockquote>/);
  assert.equal(inline("see https://example.com/x."), 'see <a href="https://example.com/x">https://example.com/x</a>.');
});

test("editor context in a prompt is split from what was typed", () => {
  const { typed, context } = splitContext("Explain this\n\nsrc/cart.ts, lines 7-13:\n\n```ts\nexport function total() {}\n```");
  assert.equal(typed, "Explain this");
  assert.deepEqual(context, [{ heading: "src/cart.ts, lines 7-13:", label: "cart.ts:7-13", code: "export function total() {}" }]);
  assert.deepEqual(splitContext("plain text\n```js\nx\n```").context, [], "a fence the user typed stays");
  assert.equal(splitContext("a\n\nREADME.md (unsaved changes):\n\n````md\n# x\n```\n````").context[0].label, "README.md");
});

test("edit diffs keep context and mark changes", () => {
  const ops = diffLines("a\nb\nc", "a\nB\nc\nd");
  assert.deepEqual(ops.map((o) => `${o.op}${o.text}`), [" a", "-b", "+B", " c", "+d"]);
  assert.deepEqual(diffLines("", "x").map((o) => o.op), ["+"]);
});

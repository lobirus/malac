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

test("send_to_user results become a caption and files; the tags never show", async () => {
  const { parseShared, kindLabel, isInline } = await import("../src/webview/shared.mjs");
  const output = 'Sent to the user: Screenshot 2026-10-27 14.02.11.jpg, report.pdf.\n<caption>The settings page after the fix</caption>\n<shared id="0193f0c2" name="Screenshot 2026-10-27 14.02.11.jpg" type="image/jpeg" size="84211"/>\n<shared id="0193f0c3" name="report.pdf" type="application/pdf" size="1048576"/>';
  const { caption, files, text } = parseShared(output);
  assert.equal(caption, "The settings page after the fix");
  assert.deepEqual(files, [
    { id: "0193f0c2", name: "Screenshot 2026-10-27 14.02.11.jpg", type: "image/jpeg", size: 84211 },
    { id: "0193f0c3", name: "report.pdf", type: "application/pdf", size: 1048576 },
  ]);
  assert.equal(text, "Sent to the user: Screenshot 2026-10-27 14.02.11.jpg, report.pdf.");
  assert.deepEqual(parseShared("Sent.\n<shared broken").files, []);
  assert.equal(parseShared('<shared id="x" name="a.txt"/>').files[0].type, "application/octet-stream");
  assert.equal(kindLabel(files[1]), "PDF");
  assert.equal(kindLabel({ name: "notes.md", type: "application/octet-stream" }), "MD");
  assert.equal(kindLabel(files[0]), "JPG image");
  assert.ok(isInline(files[0]) && !isInline(files[1]));
});

test("the viewer fits, keeps the point under the pointer and keeps edges in view", async () => {
  const { clampOffset, fitScale, zoomOffset } = await import("../src/webview/viewer.mjs");
  assert.equal(fitScale(2000, 1000, 500, 500), 0.25);
  assert.equal(fitScale(100, 50, 500, 500), 1, "small images are not blown up");
  // Zooming 1x → 2x around x=100 keeps the image point under it there.
  const offset = zoomOffset(0, 100, 1, 2);
  assert.equal(offset, -100);
  assert.equal((100 - offset) / 2, 100);
  assert.equal(clampOffset(50, 400, 500), 50, "smaller than the stage: centred");
  assert.equal(clampOffset(20, 1000, 500), 0, "the left edge stays at the left");
  assert.equal(clampOffset(-900, 1000, 500), -500, "the right edge stays at the right");
});

test("dictated text goes in at the cursor with spaces where words would touch", async () => {
  const { insertDictation } = await import("../src/webview/composer.mjs");
  assert.deepEqual(insertDictation("", 0, 0, "fix the tests"), { value: "fix the tests", caret: 13 });
  assert.deepEqual(insertDictation("Please now", 6, 6, "fix it"), { value: "Please fix it now", caret: 13 });
  assert.deepEqual(insertDictation("Say hi", 6, 6, "there"), { value: "Say hi there", caret: 12 });
  assert.deepEqual(insertDictation("a XX b", 2, 4, "word"), { value: "a word b", caret: 6 }, "a selection is replaced");
});

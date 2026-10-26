import assert from "node:assert/strict";
import { test } from "node:test";
import { chipFor, chipLabel, contextPayload, EditorSnapshot, INLINE_LIMIT, lineRange, withContext } from "../src/model/context";

function editor(overrides: Partial<EditorSnapshot> = {}): EditorSnapshot {
  return {
    path: "/src/app/README.md",
    displayPath: "README.md",
    languageId: "markdown",
    selection: null,
    selectedText: "",
    dirty: false,
    text: () => "# app\n",
    ...overrides,
  };
}

const select = (startLine: number, startCharacter: number, endLine: number, endCharacter: number) => ({ startLine, startCharacter, endLine, endCharacter });

test("line ranges are one-based and leave out a line the selection only touches", () => {
  assert.equal(lineRange(null), null);
  assert.equal(lineRange(select(4, 2, 4, 2)), null, "an empty selection is no range");
  assert.deepEqual(lineRange(select(11, 0, 29, 7)), { first: 12, last: 30 });
  assert.deepEqual(lineRange(select(11, 0, 30, 0)), { first: 12, last: 30 }, "ending at column 0 stops at the line before");
  assert.deepEqual(lineRange(select(3, 1, 3, 9)), { first: 4, last: 4 });
});

test("the chip shows the file name, with the selected lines", () => {
  assert.equal(chipLabel("/src/app/README.md", null), "README.md");
  assert.equal(chipLabel("/src/app/README.md", { first: 12, last: 30 }), "README.md:12-30");
  assert.equal(chipLabel("C:\\src\\main.rs", { first: 3, last: 3 }), "main.rs:3");
  const chip = chipFor(editor({ selection: select(11, 0, 29, 4) }), true)!;
  assert.equal(chip.label, "README.md:12-30");
  assert.equal(chip.enabled, true);
  assert.match(chip.title, /Lines 12-30 of README\.md goes with your next message/);
});

test("the chip follows the editor and can be turned off", () => {
  assert.equal(chipFor(null, true), null, "no editor, no chip");
  assert.equal(chipFor(editor({ path: null }), true), null, "untitled and output editors get no chip");
  const off = chipFor(editor(), false)!;
  assert.equal(off.enabled, false);
  assert.match(off.title, /left out/);
  assert.deepEqual(contextPayload(off, editor()), { attachments: [], inline: "" }, "a disabled chip adds nothing");
});

test("a saved file goes as an attachment, a selection inline", () => {
  const whole = editor();
  assert.deepEqual(contextPayload(chipFor(whole, true), whole), { attachments: [{ path: "/src/app/README.md" }], inline: "" });

  const selected = editor({ selection: select(1, 0, 3, 0), selectedText: "line two\nline three\n", languageId: "markdown" });
  const payload = contextPayload(chipFor(selected, true), selected);
  assert.deepEqual(payload.attachments, []);
  assert.equal(payload.inline, "README.md, lines 2-3:\n\n```markdown\nline two\nline three\n```");
});

test("unsaved changes travel inline, so the agent sees the editor's text", () => {
  const dirty = editor({ dirty: true, text: () => "# changed\n" });
  const payload = contextPayload(chipFor(dirty, true), dirty);
  assert.deepEqual(payload.attachments, []);
  assert.match(payload.inline, /^README\.md \(unsaved changes\):\n\n```markdown\n# changed\n/);
});

test("the fence outgrows backticks in the code and long text is cut", () => {
  const code = "```js\nx\n```";
  const selected = editor({ selection: select(0, 0, 2, 3), selectedText: code });
  assert.match(contextPayload(chipFor(selected, true), selected).inline, /\n````markdown\n```js\nx\n```\n````$/);
  const huge = editor({ dirty: true, text: () => "a".repeat(INLINE_LIMIT + 10) });
  const inline = contextPayload(chipFor(huge, true), huge).inline;
  assert.match(inline, /\(cut at 256 KB\)$/);
  assert.ok(inline.length < INLINE_LIMIT + 200);
});

test("a chip for another file than the editor's adds nothing", () => {
  const chip = chipFor(editor(), true);
  assert.deepEqual(contextPayload(chip, editor({ path: "/src/app/other.md" })), { attachments: [], inline: "" });
});

test("context follows the typed text", () => {
  assert.equal(withContext("explain", ""), "explain");
  assert.equal(withContext("explain", "a.md, line 1:\n\n```\nx\n```"), "explain\n\na.md, line 1:\n\n```\nx\n```");
  assert.equal(withContext("", "ctx"), "ctx");
});

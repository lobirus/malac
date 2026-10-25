// Markdown and syntax highlighting. Source text is always escaped first and
// only known tags are added, so model output can never inject markup.

import { copyText, escapeHtml, h } from "./dom.js";

const FENCE = /^ {0,3}(`{3,}|~{3,})\s*([^`\s]*)/;
const HEADING = /^ {0,3}(#{1,6})\s+(.*?)(?:\s+#+)?\s*$/;
const RULE = /^ {0,3}([-*_])(?:\s*\1){2,}\s*$/;
const QUOTE = /^ {0,3}> ?/;
const LIST_ITEM = /^( *)([-*+]|\d{1,9}[.)])\s+(.*)$/;
const TASK = /^\[([ xX])\]\s+/;
const TABLE_DIVIDER = /^\s*\|?\s*:?-+:?\s*(?:\|\s*:?-+:?\s*)*\|?\s*$/;
const CODE_SPAN = /(?<!`)(`+)(?!`)([\s\S]*?[^`])\1(?!`)/g;
const LINK = /!?\[([^\]]+)\]\(((?:[^()\s\u0000]|\([^()\s\u0000]*\))+)(?:\s+"[^"]*")?\)/g;
const BARE_URL = /(^|[\s(])(https?:\/\/[^\s<>"'`\u0000]+)/g;
const BLOCK_PARSERS = [fencedCode, heading, rule, blockquote, table, list, paragraph];

export function renderMarkdown(source) {
  const lines = String(source).replace(/\r\n?/g, "\n").replace(/\u0000/g, "").split("\n");
  return renderBlocks(lines);
}

/** A rendered markdown element whose code blocks have working copy buttons. */
export function markdownNode(source, className = "") {
  const node = h("div", { class: `markdown ${className}`.trim() });
  node.innerHTML = renderMarkdown(source);
  node.addEventListener("click", onCodeCopy);
  return node;
}

/** Re-renders `node` in place (used while text streams in). */
export function updateMarkdown(node, source) {
  node.innerHTML = renderMarkdown(source);
}

async function onCodeCopy(event) {
  const button = event.target.closest?.(".code-copy");
  if (!button) return;
  const code = button.closest(".code-block")?.querySelector("code");
  if (!code) return;
  const ok = await copyText(code.textContent);
  button.classList.toggle("copied", ok);
  button.textContent = ok ? "Copied" : "Copy failed";
  setTimeout(() => {
    button.classList.remove("copied");
    button.textContent = "Copy";
  }, 1400);
}

function renderBlocks(lines) {
  const out = [];
  let i = 0;
  while (i < lines.length) {
    if (isBlank(lines[i])) {
      i += 1;
      continue;
    }
    for (const parse of BLOCK_PARSERS) {
      const block = parse(lines, i);
      if (block) {
        out.push(block.html);
        i = block.next;
        break;
      }
    }
  }
  return out.join("");
}

function isBlank(line) {
  return line.trim() === "";
}

function indentOf(line) {
  return line.length - line.trimStart().length;
}

function startsBlock(line) {
  return FENCE.test(line) || HEADING.test(line) || RULE.test(line) || QUOTE.test(line) || LIST_ITEM.test(line);
}

function isTableStart(lines, i) {
  return lines[i].includes("|") && (lines[i + 1] ?? "").includes("|") && TABLE_DIVIDER.test(lines[i + 1]);
}

function fencedCode(lines, start) {
  const open = FENCE.exec(lines[start]);
  if (!open) return null;
  const close = new RegExp(`^ {0,3}${open[1][0] === "`" ? "`" : "~"}{${open[1].length},}\\s*$`);
  const body = [];
  let i = start + 1;
  while (i < lines.length && !close.test(lines[i])) body.push(lines[i++]);
  const lang = open[2].replace(/[^\w+#.-]/g, "").toLowerCase();
  return { html: codeBlockHtml(body.join("\n"), lang), next: i + 1 };
}

export function codeBlockHtml(code, lang) {
  const attr = lang ? ` class="language-${lang}"` : "";
  const label = lang ? `<span class="code-lang">${escapeHtml(lang)}</span>` : `<span class="code-lang"></span>`;
  return (
    `<div class="code-block"><div class="code-head">${label}` +
    `<button class="code-copy" type="button">Copy</button></div>` +
    `<pre><code${attr}>${highlight(code, lang)}</code></pre></div>`
  );
}

function heading(lines, i) {
  const match = HEADING.exec(lines[i]);
  if (!match) return null;
  const level = match[1].length;
  return { html: `<h${level}>${inline(match[2])}</h${level}>`, next: i + 1 };
}

function rule(lines, i) {
  return RULE.test(lines[i]) ? { html: "<hr>", next: i + 1 } : null;
}

function blockquote(lines, start) {
  if (!QUOTE.test(lines[start])) return null;
  const body = [];
  let i = start;
  while (i < lines.length && QUOTE.test(lines[i])) body.push(lines[i++].replace(QUOTE, ""));
  return { html: `<blockquote>${renderBlocks(body)}</blockquote>`, next: i };
}

function table(lines, start) {
  if (!isTableStart(lines, start)) return null;
  const head = splitRow(lines[start]);
  const aligns = splitRow(lines[start + 1]).map(alignment);
  const rows = [];
  let i = start + 2;
  while (i < lines.length && lines[i].includes("|") && !isBlank(lines[i])) rows.push(splitRow(lines[i++]));
  const cell = (tag, text, column) => {
    const style = aligns[column] ? ` style="text-align:${aligns[column]}"` : "";
    return `<${tag}${style}>${inline(text ?? "")}</${tag}>`;
  };
  const headHtml = head.map((text, c) => cell("th", text, c)).join("");
  const bodyHtml = rows.map((row) => `<tr>${head.map((_, c) => cell("td", row[c], c)).join("")}</tr>`).join("");
  return {
    html: `<div class="table"><table><thead><tr>${headHtml}</tr></thead><tbody>${bodyHtml}</tbody></table></div>`,
    next: i,
  };
}

function splitRow(line) {
  let row = line.trim();
  if (row.startsWith("|")) row = row.slice(1);
  if (row.endsWith("|") && !row.endsWith("\\|")) row = row.slice(0, -1);
  return row.split(/(?<!\\)\|/).map((cell) => cell.trim().replace(/\\\|/g, "|"));
}

function alignment(divider) {
  const left = divider.startsWith(":");
  const right = divider.endsWith(":");
  if (left && right) return "center";
  if (right) return "right";
  return left ? "left" : null;
}

function list(lines, start) {
  const first = LIST_ITEM.exec(lines[start]);
  if (!first) return null;
  const indent = first[1].length;
  const ordered = /\d/.test(first[2]);
  const sameList = (match) => match && match[1].length === indent && /\d/.test(match[2]) === ordered;
  const items = [];
  let loose = false;
  let i = start;
  while (i < lines.length) {
    const line = lines[i];
    const match = LIST_ITEM.exec(line);
    if (sameList(match)) {
      items.push({ lines: [match[3]], contentIndent: line.length - match[3].length });
      i += 1;
      continue;
    }
    if (match && indentOf(line) <= indent) break;
    const item = items[items.length - 1];
    if (isBlank(line)) {
      const next = lines[i + 1];
      if (next === undefined || (indentOf(next) <= indent && !sameList(LIST_ITEM.exec(next)))) break;
      item.lines.push("");
      loose = true;
    } else if (indentOf(line) > indent) {
      item.lines.push(line.slice(Math.min(indentOf(line), item.contentIndent)));
    } else if (startsBlock(line)) {
      break;
    } else {
      item.lines.push(line.trim());
    }
    i += 1;
  }
  const tag = ordered ? "ol" : "ul";
  const number = parseInt(first[2], 10);
  const startAttr = ordered && number !== 1 ? ` start="${number}"` : "";
  const body = items.map((item) => listItem(item, loose)).join("");
  return { html: `<${tag}${startAttr}>${body}</${tag}>`, next: i };
}

function listItem(item, loose) {
  const task = TASK.exec(item.lines[0]);
  if (task) item.lines[0] = item.lines[0].slice(task[0].length);
  const html = renderBlocks(item.lines);
  const body = loose ? html : html.replace(/^<p>([\s\S]*?)<\/p>/, "$1");
  if (!task) return `<li>${body}</li>`;
  const done = task[1] !== " ";
  return `<li class="task${done ? " done" : ""}"><span class="task-box" aria-hidden="true"></span>${body}</li>`;
}

function paragraph(lines, start) {
  const body = [lines[start].trim()];
  let i = start + 1;
  while (i < lines.length && !isBlank(lines[i]) && !startsBlock(lines[i]) && !isTableStart(lines, i)) {
    body.push(lines[i++].trim());
  }
  return { html: `<p>${inline(body.join("\n")).replace(/\n/g, "<br>")}</p>`, next: i };
}

// Code spans and links are swapped for placeholders before the rest of the
// text is escaped, so their contents are never parsed as emphasis.
function inline(text) {
  const stash = [];
  const keep = (html) => `\u0000${stash.push(html) - 1}\u0000`;
  let out = text.replace(CODE_SPAN, (_, _ticks, code) => keep(`<code>${escapeHtml(code.trim())}</code>`));
  out = out.replace(LINK, (_, label, href) => (isSafeHref(href) ? keep(anchor(href, emphasis(escapeHtml(label)))) : label));
  out = out.replace(BARE_URL, (_, before, raw) => {
    const url = raw.replace(/[.,:;!?)\]]+$/, "");
    return before + keep(anchor(url, escapeHtml(url))) + raw.slice(url.length);
  });
  const restore = (html) => html.replace(/\u0000(\d+)\u0000/g, (_, n) => restore(stash[Number(n)]));
  return restore(emphasis(escapeHtml(out)));
}

function anchor(href, html) {
  return `<a href="${escapeHtml(href)}" target="_blank" rel="noopener noreferrer">${html}</a>`;
}

function isSafeHref(href) {
  return /^(https?:\/\/|mailto:)/i.test(href);
}

function emphasis(html) {
  return html
    .replace(/\*\*(?=\S)([\s\S]*?\S)\*\*/g, "<strong>$1</strong>")
    .replace(/(^|[^\w])__(?=\S)([\s\S]*?\S)__(?!\w)/g, "$1<strong>$2</strong>")
    .replace(/(^|[^*\w])\*(?=[^\s*])([^*]*?[^\s*])\*(?!\*)/g, "$1<em>$2</em>")
    .replace(/(^|[^\w])_(?=[^\s_])([^_]*?[^\s_])_(?!\w)/g, "$1<em>$2</em>")
    .replace(/~~(?=\S)([\s\S]*?\S)~~/g, "<del>$1</del>");
}

// Syntax highlighting: a small tokenizer per language family, good enough to
// make code readable without shipping a parser.

const words = (list) => new Set(list.split(" "));

const FAMILIES = {
  js: {
    line: "//",
    block: true,
    strings: "\"'`",
    keywords: words(
      "async await break case catch class const continue debugger default delete do else export extends finally for from function get if import in instanceof let new of return set static super switch this throw try typeof var void while yield",
    ),
    literals: words("true false null undefined NaN Infinity"),
  },
  rust: {
    line: "//",
    block: true,
    strings: '"',
    keywords: words(
      "as async await break const continue crate dyn else enum extern fn for if impl in let loop match mod move mut pub ref return self Self static struct super trait type unsafe use where while",
    ),
    literals: words("true false None Some Ok Err"),
  },
  python: {
    line: "#",
    strings: "\"'",
    keywords: words(
      "and as assert async await break class continue def del elif else except finally for from global if import in is lambda nonlocal not or pass raise return try while with yield self",
    ),
    literals: words("True False None"),
  },
  go: {
    line: "//",
    block: true,
    strings: "\"'`",
    keywords: words(
      "break case chan const continue default defer else fallthrough for func go goto if import interface map package range return select struct switch type var",
    ),
    literals: words("true false nil iota"),
  },
  c: {
    line: "//",
    block: true,
    strings: "\"'",
    keywords: words(
      "auto break case catch char class const continue default delete do double else enum extern final float for fun func goto guard if implements import include int interface let long namespace new override package private protected public return short signed sizeof static struct switch template this throw throws try typedef typename union unsigned using val var virtual void volatile while",
    ),
    literals: words("true false null nullptr nil"),
  },
  sh: {
    line: "#",
    strings: "\"'",
    keywords: words("if then else elif fi for while until do done case esac in function return local export cd echo exit set unset source alias sudo"),
    literals: words("true false"),
  },
  sql: {
    line: "--",
    block: true,
    strings: "'\"",
    keywords: words(
      "select from where insert into values update set delete create table drop alter join left right inner outer on group by order having limit as and or not is in primary key index distinct union all case when then else end returning with",
    ),
    literals: words("null true false"),
    caseless: true,
  },
  toml: { line: "#", strings: "\"'", keywords: new Set(), literals: words("true false"), keys: /^(\s*)(\[\[?[^\]]*\]\]?|[\w.\-"]+)(?=\s*=|\s*$)/ },
  yaml: { line: "#", strings: "\"'", keywords: new Set(), literals: words("true false null yes no"), keys: /^(\s*-?\s*)([\w.\-"]+)(?=\s*:)/ },
  json: { strings: '"', keywords: new Set(), literals: words("true false null") },
};

const ALIASES = {
  js: "js", javascript: "js", mjs: "js", cjs: "js", jsx: "js", ts: "js", tsx: "js", typescript: "js",
  rs: "rust", rust: "rust",
  py: "python", python: "python",
  go: "go", golang: "go",
  c: "c", h: "c", cpp: "c", "c++": "c", cc: "c", hpp: "c", cs: "c", csharp: "c", java: "c", kotlin: "c", kt: "c", swift: "c", php: "c", scala: "c", dart: "c",
  sh: "sh", bash: "sh", shell: "sh", zsh: "sh", console: "sh", fish: "sh", ps1: "sh",
  sql: "sql", toml: "toml", ini: "toml", cfg: "toml", yaml: "yaml", yml: "yaml", json: "json", jsonc: "json",
};

/** Maps a file name to a language for highlighting. */
export function languageFor(path) {
  const name = String(path ?? "").toLowerCase();
  if (/(^|\/)(dockerfile|makefile)$/.test(name)) return "sh";
  const ext = name.split(".").pop();
  if (ext === "md" || ext === "markdown") return "";
  return ALIASES[ext] ? ext : "";
}

export function highlight(code, lang) {
  if (lang === "diff" || lang === "patch") return highlightDiff(code);
  const family = FAMILIES[ALIASES[lang]];
  if (!family || code.length > 200_000) return escapeHtml(code);
  return code
    .split("\n")
    .reduce(
      (acc, line) => {
        const { html, open } = highlightLine(line, family, acc.open);
        acc.out.push(html);
        acc.open = open;
        return acc;
      },
      { out: [], open: null },
    )
    .out.join("\n");
}

function highlightDiff(code) {
  return code
    .split("\n")
    .map((line) => {
      const cls = line.startsWith("+") ? "add" : line.startsWith("-") ? "del" : line.startsWith("@@") ? "hunk" : "";
      return cls ? `<span class="tok-${cls}">${escapeHtml(line)}</span>` : escapeHtml(line);
    })
    .join("\n");
}

const span = (cls, text) => `<span class="tok-${cls}">${escapeHtml(text)}</span>`;

// `open` carries an unterminated block comment or string into the next line.
function highlightLine(line, family, open) {
  let out = "";
  let i = 0;
  if (open) {
    const end = line.indexOf(open.close);
    if (end < 0) return { html: span(open.cls, line), open };
    out += span(open.cls, line.slice(0, end + open.close.length));
    i = end + open.close.length;
  } else if (family.keys) {
    const key = family.keys.exec(line);
    if (key) {
      out += escapeHtml(key[1]) + span("key", key[2]);
      i = key[0].length;
    }
  }
  while (i < line.length) {
    const rest = line.slice(i);
    const ch = line[i];
    if (family.line && rest.startsWith(family.line) && (family.line !== "#" || i === 0 || /\s/.test(line[i - 1]))) {
      return { html: out + span("comment", rest), open: null };
    }
    if (family.block && rest.startsWith("/*")) {
      const end = rest.indexOf("*/", 2);
      if (end < 0) return { html: out + span("comment", rest), open: { close: "*/", cls: "comment" } };
      out += span("comment", rest.slice(0, end + 2));
      i += end + 2;
      continue;
    }
    if (family.strings.includes(ch) && !(ch === "'" && family === FAMILIES.rust && /^'[a-z_]\w*(?!')/.test(rest))) {
      const triple = ch.repeat(3);
      if ((ch === '"' || ch === "'") && family === FAMILIES.python && rest.startsWith(triple)) {
        const end = rest.indexOf(triple, 3);
        if (end < 0) return { html: out + span("string", rest), open: { close: triple, cls: "string" } };
        out += span("string", rest.slice(0, end + 3));
        i += end + 3;
        continue;
      }
      let j = 1;
      while (j < rest.length && rest[j] !== ch) j += rest[j] === "\\" ? 2 : 1;
      if (j >= rest.length && ch === "`") return { html: out + span("string", rest), open: { close: "`", cls: "string" } };
      out += span("string", rest.slice(0, j + 1));
      i += j + 1;
      continue;
    }
    const number = /^(?:0x[\da-f_]+|\d[\d_]*(?:\.\d[\d_]*)?(?:e[+-]?\d+)?)[a-z\d]*/i.exec(rest);
    if (number && (i === 0 || !/[\w$]/.test(line[i - 1]))) {
      out += span("number", number[0]);
      i += number[0].length;
      continue;
    }
    const word = /^[A-Za-z_$][\w$]*!?/.exec(rest);
    if (word) {
      const text = word[0];
      const key = family.caseless ? text.toLowerCase() : text;
      if (family.keywords.has(key)) out += span("keyword", text);
      else if (family.literals.has(key)) out += span("literal", text);
      else if (text.endsWith("!") || line[i + text.length] === "(") out += span("fn", text);
      else if (/^[A-Z][a-z]/.test(text) && family !== FAMILIES.sh && family !== FAMILIES.sql) out += span("type", text);
      else out += escapeHtml(text);
      i += text.length;
      continue;
    }
    out += escapeHtml(ch);
    i += 1;
  }
  return { html: out, open: null };
}

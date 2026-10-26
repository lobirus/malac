// A small Markdown renderer for agent messages. The source is escaped first
// and only known tags are produced, so model output cannot inject markup.

const FENCE = /^ {0,3}(`{3,}|~{3,})\s*([^`\s]*)/;
const HEADING = /^ {0,3}(#{1,6})\s+(.*?)(?:\s+#+)?\s*$/;
const RULE = /^ {0,3}([-*_])(?:\s*\1){2,}\s*$/;
const QUOTE = /^ {0,3}> ?/;
const LIST_ITEM = /^( *)([-*+]|\d{1,9}[.)])\s+(.*)$/;
const TASK = /^\[([ xX])\]\s+/;
const TABLE_DIVIDER = /^\s*\|?\s*:?-+:?\s*(?:\|\s*:?-+:?\s*)*\|?\s*$/;

export function escapeHtml(text: string): string {
  return text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;").replace(/'/g, "&#39;");
}

export function renderMarkdown(source: string): string {
  const lines = String(source).replace(/\r\n?/g, "\n").replace(/\u0000/g, "").split("\n");
  return blocks(lines);
}

function blocks(lines: string[]): string {
  const out: string[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    if (!line.trim()) {
      i += 1;
      continue;
    }
    const fence = FENCE.exec(line);
    if (fence) {
      const marker = fence[1];
      const body: string[] = [];
      i += 1;
      while (i < lines.length && !lines[i].trimStart().startsWith(marker)) body.push(lines[i++]);
      i += 1;
      out.push(codeBlock(body.join("\n"), fence[2]));
      continue;
    }
    const heading = HEADING.exec(line);
    if (heading) {
      const level = Math.min(6, heading[1].length + 2);
      out.push(`<h${level}>${inline(heading[2])}</h${level}>`);
      i += 1;
      continue;
    }
    if (RULE.test(line)) {
      out.push("<hr>");
      i += 1;
      continue;
    }
    if (QUOTE.test(line)) {
      const body: string[] = [];
      while (i < lines.length && QUOTE.test(lines[i])) body.push(lines[i++].replace(QUOTE, ""));
      out.push(`<blockquote>${blocks(body)}</blockquote>`);
      continue;
    }
    if (line.includes("|") && i + 1 < lines.length && TABLE_DIVIDER.test(lines[i + 1]) && lines[i + 1].includes("-")) {
      const rows: string[] = [line];
      i += 2;
      while (i < lines.length && lines[i].includes("|") && lines[i].trim()) rows.push(lines[i++]);
      out.push(table(rows));
      continue;
    }
    if (LIST_ITEM.test(line)) {
      const items: string[] = [];
      const first = LIST_ITEM.exec(line)!;
      const ordered = /\d/.test(first[2]);
      const indent = first[1].length;
      // A top-level item of the other kind (bullets after numbers) starts a new list.
      const sameKind = (text: string) => {
        const match = LIST_ITEM.exec(text);
        return Boolean(match) && (match![1].length > indent + 1 || /\d/.test(match![2]) === ordered);
      };
      while (
        i < lines.length &&
        ((LIST_ITEM.test(lines[i]) && sameKind(lines[i])) ||
          (lines[i].startsWith("  ") && !LIST_ITEM.test(lines[i]) && items.length) ||
          (!lines[i].trim() && sameKind(lines[i + 1] ?? "")))
      ) {
        items.push(lines[i++]);
      }
      out.push(list(items));
      continue;
    }
    const paragraph: string[] = [];
    while (i < lines.length && lines[i].trim() && !FENCE.test(lines[i]) && !HEADING.test(lines[i]) && !QUOTE.test(lines[i]) && !(LIST_ITEM.test(lines[i]) && paragraph.length === 0)) {
      paragraph.push(lines[i++]);
    }
    if (!paragraph.length) paragraph.push(lines[i++]);
    out.push(`<p>${paragraph.map((l) => inline(l.trim())).join("<br>")}</p>`);
  }
  return out.join("\n");
}

function codeBlock(code: string, lang: string): string {
  const language = lang ? ` data-lang="${escapeHtml(lang)}"` : "";
  return `<div class="code-block"${language}><button class="code-copy" type="button" aria-label="Copy code" title="Copy">Copy</button><pre><code>${escapeHtml(code)}</code></pre></div>`;
}

function list(lines: string[]): string {
  const first = LIST_ITEM.exec(lines.find((l) => LIST_ITEM.test(l)) ?? "");
  const ordered = Boolean(first && /\d/.test(first[2]));
  const base = first?.[1].length ?? 0;
  const items: string[][] = [];
  for (const line of lines) {
    const match = LIST_ITEM.exec(line);
    if (match && match[1].length <= base + 1) items.push([match[3]]);
    else if (items.length) items.at(-1)!.push(line.slice(Math.min(base + 2, line.length - line.trimStart().length)));
  }
  const tag = ordered ? "ol" : "ul";
  const start = ordered && first && parseInt(first[2], 10) !== 1 ? ` start="${parseInt(first[2], 10)}"` : "";
  const body = items
    .map(([head, ...rest]) => {
      const task = TASK.exec(head);
      const text = task ? head.slice(task[0].length) : head;
      const box = task ? `<span class="task ${task[1] === " " ? "" : "done"}" aria-hidden="true"></span>` : "";
      const nested = rest.some((l) => l.trim()) ? blocks(rest) : "";
      return `<li${task ? ' class="task-item"' : ""}>${box}${inline(text)}${nested}</li>`;
    })
    .join("");
  return `<${tag}${start}>${body}</${tag}>`;
}

function table(rows: string[]): string {
  const cells = (row: string) =>
    row
      .trim()
      .replace(/^\||\|$/g, "")
      .split("|")
      .map((cell) => cell.trim());
  const [head, ...body] = rows.map(cells);
  return `<div class="table-wrap"><table><thead><tr>${head.map((c) => `<th>${inline(c)}</th>`).join("")}</tr></thead><tbody>${body
    .map((r) => `<tr>${r.map((c) => `<td>${inline(c)}</td>`).join("")}</tr>`)
    .join("")}</tbody></table></div>`;
}

/** Inline markup on one line of text. Code spans are cut out first. */
export function inline(text: string): string {
  const codes: string[] = [];
  let source = text.replace(/(`+)([^`]|[^`][\s\S]*?[^`])\1(?!`)/g, (_, _ticks: string, code: string) => {
    codes.push(`<code>${escapeHtml(code.trim() ? code.replace(/^ (.*) $/, "$1") : code)}</code>`);
    return `\u0000${codes.length - 1}\u0000`;
  });
  const links: string[] = [];
  source = source.replace(/\[([^\]]+)\]\(((?:[^()\s]|\([^()\s]*\))+)(?:\s+"[^"]*")?\)/g, (match, label: string, url: string) => {
    const href = safeUrl(url);
    if (!href) return match;
    links.push(`<a href="${escapeHtml(href)}" title="${escapeHtml(href)}">${emphasis(escapeHtml(label))}</a>`);
    return `\u0001${links.length - 1}\u0001`;
  });
  let html = emphasis(escapeHtml(source));
  html = html.replace(/(^|[\s(])(https?:\/\/[^\s<>"'`]+[^\s<>"'`.,;:!?)])/g, (_, lead: string, url: string) => `${lead}<a href="${url}">${url}</a>`);
  html = html.replace(/\u0001(\d+)\u0001/g, (_, n: string) => links[Number(n)]);
  return html.replace(/\u0000(\d+)\u0000/g, (_, n: string) => codes[Number(n)]);
}

function emphasis(html: string): string {
  return html
    .replace(/\*\*(?=\S)([\s\S]*?\S)\*\*/g, "<strong>$1</strong>")
    .replace(/(^|[^\w])__(?=\S)([\s\S]*?\S)__(?!\w)/g, "$1<strong>$2</strong>")
    .replace(/(^|[^*\w])\*(?=[^\s*])([^*]*?[^\s*])\*(?!\*)/g, "$1<em>$2</em>")
    .replace(/(^|[^\w])_(?=[^\s_])([^_]*?[^\s_])_(?!\w)/g, "$1<em>$2</em>")
    .replace(/~~(?=\S)([\s\S]*?\S)~~/g, "<del>$1</del>");
}

function safeUrl(url: string): string | null {
  const value = url.trim();
  return /^(https?:|mailto:)/i.test(value) ? value : null;
}

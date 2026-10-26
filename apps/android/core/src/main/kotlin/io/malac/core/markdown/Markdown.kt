package io.malac.core.markdown

/**
 * The Markdown that models write, parsed into blocks and styled spans for
 * native rendering. The rules follow `web/markdown.js`, so the app and the
 * web UI agree on what a message looks like.
 */
object Markdown {
    fun parse(source: String): List<MarkdownBlock> =
        BlockParser(source.replace(Regex("\r\n?"), "\n").replace("\u0000", "").split('\n')).blocks()

    fun inline(text: String): List<Span> = InlineParser.parse(text)
}

sealed interface MarkdownBlock {
    data class Heading(val level: Int, val content: List<Span>) : MarkdownBlock

    /** Line breaks inside a paragraph are kept, as in chat. */
    data class Paragraph(val content: List<Span>) : MarkdownBlock

    data class Code(val language: String, val code: String) : MarkdownBlock

    data class Quote(val blocks: List<MarkdownBlock>) : MarkdownBlock

    data class ListBlock(
        val ordered: Boolean,
        val start: Int,
        val items: List<ListItem>,
        /** Items separated by blank lines get paragraph spacing. */
        val loose: Boolean,
    ) : MarkdownBlock

    data class Table(
        val header: List<List<Span>>,
        val alignments: List<Alignment>,
        val rows: List<List<List<Span>>>,
    ) : MarkdownBlock

    data object Rule : MarkdownBlock
}

/** [task] is null for a plain item, else whether its box is checked. */
data class ListItem(val blocks: List<MarkdownBlock>, val task: Boolean? = null)

enum class Alignment { Start, Center, End, Default }

data class Span(
    val text: String,
    val bold: Boolean = false,
    val italic: Boolean = false,
    val code: Boolean = false,
    val strike: Boolean = false,
    val link: String? = null,
)

private val FENCE = Regex("""^ {0,3}(`{3,}|~{3,})\s*([^`\s]*)""")
private val HEADING = Regex("""^ {0,3}(#{1,6})\s+(.*?)(?:\s+#+)?\s*$""")
private val RULE = Regex("""^ {0,3}([-*_])(?:\s*\1){2,}\s*$""")
private val QUOTE = Regex("""^ {0,3}> ?""")
private val LIST_ITEM = Regex("""^( *)([-*+]|\d{1,9}[.)])\s+(.*)$""")
private val TASK = Regex("""^\[([ xX])]\s+""")
private val TABLE_DIVIDER = Regex("""^\s*\|?\s*:?-+:?\s*(?:\|\s*:?-+:?\s*)*\|?\s*$""")

private class BlockParser(private val lines: List<String>) {
    fun blocks(): List<MarkdownBlock> {
        val out = mutableListOf<MarkdownBlock>()
        var i = 0
        while (i < lines.size) {
            if (lines[i].isBlank()) {
                i++
                continue
            }
            val (block, next) = fencedCode(i) ?: heading(i) ?: rule(i) ?: quote(i) ?: table(i) ?: list(i) ?: paragraph(i)
            out += block
            i = next
        }
        return out
    }

    private fun fencedCode(start: Int): Pair<MarkdownBlock, Int>? {
        val open = FENCE.find(lines[start]) ?: return null
        val fence = open.groupValues[1]
        val close = Regex("""^ {0,3}${Regex.escape(fence.substring(0, 1))}{${fence.length},}\s*$""")
        val body = mutableListOf<String>()
        var i = start + 1
        while (i < lines.size && !close.matches(lines[i])) body += lines[i++]
        val language = open.groupValues[2].replace(Regex("""[^\w+#.-]"""), "").lowercase()
        return MarkdownBlock.Code(language, body.joinToString("\n")) to i + 1
    }

    private fun heading(i: Int): Pair<MarkdownBlock, Int>? {
        val match = HEADING.matchEntire(lines[i]) ?: return null
        return MarkdownBlock.Heading(match.groupValues[1].length, Markdown.inline(match.groupValues[2])) to i + 1
    }

    private fun rule(i: Int): Pair<MarkdownBlock, Int>? =
        if (RULE.matches(lines[i])) MarkdownBlock.Rule to i + 1 else null

    private fun quote(start: Int): Pair<MarkdownBlock, Int>? {
        if (QUOTE.find(lines[start])?.range?.first != 0) return null
        val body = mutableListOf<String>()
        var i = start
        while (i < lines.size && QUOTE.find(lines[i])?.range?.first == 0) body += lines[i++].replaceFirst(QUOTE, "")
        return MarkdownBlock.Quote(BlockParser(body).blocks()) to i
    }

    private fun isTableStart(i: Int): Boolean =
        '|' in lines[i] && i + 1 < lines.size && '|' in lines[i + 1] && TABLE_DIVIDER.matches(lines[i + 1])

    private fun table(start: Int): Pair<MarkdownBlock, Int>? {
        if (!isTableStart(start)) return null
        val head = splitRow(lines[start])
        val alignments = splitRow(lines[start + 1]).map(::alignment)
        val rows = mutableListOf<List<String>>()
        var i = start + 2
        while (i < lines.size && '|' in lines[i] && lines[i].isNotBlank()) rows += splitRow(lines[i++])
        val table = MarkdownBlock.Table(
            header = head.map(Markdown::inline),
            alignments = head.indices.map { alignments.getOrElse(it) { Alignment.Default } },
            rows = rows.map { row -> head.indices.map { Markdown.inline(row.getOrElse(it) { "" }) } },
        )
        return table to i
    }

    private fun list(start: Int): Pair<MarkdownBlock, Int>? {
        val first = LIST_ITEM.matchEntire(lines[start]) ?: return null
        val indent = first.groupValues[1].length
        val ordered = first.groupValues[2][0].isDigit()
        fun sameList(match: MatchResult?) =
            match != null && match.groupValues[1].length == indent && match.groupValues[2][0].isDigit() == ordered

        class Item(val lines: MutableList<String>, val contentIndent: Int)
        val items = mutableListOf<Item>()
        var loose = false
        var i = start
        while (i < lines.size) {
            val line = lines[i]
            val match = LIST_ITEM.matchEntire(line)
            if (sameList(match)) {
                val content = match!!.groupValues[3]
                items += Item(mutableListOf(content), line.length - content.length)
                i++
                continue
            }
            if (match != null && indentOf(line) <= indent) break
            val item = items.last()
            when {
                line.isBlank() -> {
                    val next = lines.getOrNull(i + 1)
                    if (next == null || (indentOf(next) <= indent && !sameList(LIST_ITEM.matchEntire(next)))) break
                    item.lines += ""
                    loose = true
                }
                indentOf(line) > indent -> item.lines += line.substring(minOf(indentOf(line), item.contentIndent))
                startsBlock(line) -> break
                else -> item.lines += line.trim()
            }
            i++
        }
        val number = first.groupValues[2].takeWhile { it.isDigit() }.toIntOrNull() ?: 1
        val parsed = items.map { item ->
            val task = TASK.find(item.lines[0])
            if (task != null) item.lines[0] = item.lines[0].substring(task.range.last + 1)
            ListItem(BlockParser(item.lines).blocks(), task?.let { it.groupValues[1] != " " })
        }
        return MarkdownBlock.ListBlock(ordered, if (ordered) number else 1, parsed, loose) to i
    }

    private fun paragraph(start: Int): Pair<MarkdownBlock, Int> {
        val body = mutableListOf(lines[start].trim())
        var i = start + 1
        while (i < lines.size && lines[i].isNotBlank() && !startsBlock(lines[i]) && !isTableStart(i)) body += lines[i++].trim()
        return MarkdownBlock.Paragraph(Markdown.inline(body.joinToString("\n"))) to i
    }

    private fun startsBlock(line: String): Boolean =
        FENCE.containsMatchIn(line) || HEADING.matches(line) || RULE.matches(line) ||
            QUOTE.find(line)?.range?.first == 0 || LIST_ITEM.matches(line)

    private fun indentOf(line: String) = line.length - line.trimStart().length

    private fun splitRow(line: String): List<String> {
        var row = line.trim()
        if (row.startsWith("|")) row = row.substring(1)
        if (row.endsWith("|") && !row.endsWith("\\|")) row = row.dropLast(1)
        return row.split(Regex("""(?<!\\)\|""")).map { it.trim().replace("\\|", "|") }
    }

    private fun alignment(divider: String): Alignment {
        val left = divider.startsWith(":")
        val right = divider.endsWith(":")
        return when {
            left && right -> Alignment.Center
            right -> Alignment.End
            left -> Alignment.Start
            else -> Alignment.Default
        }
    }
}

/**
 * Code spans and links first, so their contents are never read as emphasis;
 * then emphasis, which nests.
 */
private object InlineParser {
    private val CODE_SPAN = Regex("""(?<!`)(`+)(?!`)([\s\S]*?[^`])\1(?!`)""")
    private val LINK = Regex("""!?\[([^\]]+)]\(((?:[^()\s\u0000]|\([^()\s\u0000]*\))+)(?:\s+"[^"]*")?\)""")
    private val BARE_URL = Regex("""(^|[\s(])(https?://[^\s<>"'`\u0000]+)""")
    private val SAFE_HREF = Regex("""^(https?://|mailto:)""", RegexOption.IGNORE_CASE)

    private enum class Mark { Bold, Italic, Strike }

    private val EMPHASIS = listOf(
        Regex("""\*\*(?=\S)([\s\S]*?\S)\*\*""") to Mark.Bold,
        Regex("""(^|[^\w])__(?=\S)([\s\S]*?\S)__(?!\w)""") to Mark.Bold,
        Regex("""(^|[^*\w])\*(?=[^\s*])([^*]*?[^\s*])\*(?!\*)""") to Mark.Italic,
        Regex("""(^|[^\w])_(?=[^\s_])([^_]*?[^\s_])_(?!\w)""") to Mark.Italic,
        Regex("""~~(?=\S)([\s\S]*?\S)~~""") to Mark.Strike,
    )

    fun parse(text: String): List<Span> {
        val out = mutableListOf<Span>()
        var position = 0
        while (position < text.length) {
            val code = CODE_SPAN.find(text, position)
            val link = LINK.find(text, position)
            val bare = BARE_URL.find(text, position)
            val next = listOfNotNull(code, link, bare).minByOrNull { it.range.first }
            if (next == null) {
                emphasis(text.substring(position), Span(""), out)
                break
            }
            when (next) {
                code -> {
                    emphasis(text.substring(position, next.range.first), Span(""), out)
                    out += Span(next.groupValues[2].trim(), code = true)
                }
                link -> {
                    emphasis(text.substring(position, next.range.first), Span(""), out)
                    val href = next.groupValues[2]
                    emphasis(next.groupValues[1], Span("", link = href.takeIf { SAFE_HREF.containsMatchIn(it) }), out)
                }
                else -> {
                    val before = next.groupValues[1]
                    val raw = next.groupValues[2]
                    val url = raw.replace(Regex("""[.,:;!?)\]]+$"""), "")
                    emphasis(text.substring(position, next.range.first) + before, Span(""), out)
                    out += Span(url, link = url)
                    // Trailing punctuation is not part of the address.
                    emphasis(raw.substring(url.length), Span(""), out)
                }
            }
            position = next.range.last + 1
        }
        return merge(out)
    }

    private class Found(val match: MatchResult, val mark: Mark) {
        // Patterns with a leading context group keep that character outside the emphasis.
        private val hasContext = match.groups.size == 3
        val prefix: String = if (hasContext) match.groupValues[1] else ""
        val inner: String = match.groupValues[if (hasContext) 2 else 1]
        val start: Int = match.range.first + prefix.length
    }

    private fun emphasis(text: String, style: Span, out: MutableList<Span>) {
        var position = 0
        while (position < text.length) {
            val found = EMPHASIS
                .mapNotNull { (regex, mark) -> regex.find(text, position)?.let { Found(it, mark) } }
                .minByOrNull { it.start }
            if (found == null) {
                out += style.copy(text = text.substring(position))
                return
            }
            out += style.copy(text = text.substring(position, found.start))
            val nested = when (found.mark) {
                Mark.Bold -> style.copy(bold = true)
                Mark.Italic -> style.copy(italic = true)
                Mark.Strike -> style.copy(strike = true)
            }
            emphasis(found.inner, nested, out)
            position = found.match.range.last + 1
        }
    }

    private fun merge(spans: List<Span>): List<Span> {
        val out = mutableListOf<Span>()
        for (span in spans) {
            if (span.text.isEmpty()) continue
            val last = out.lastOrNull()
            if (last != null && last.copy(text = "") == span.copy(text = "")) {
                out[out.lastIndex] = last.copy(text = last.text + span.text)
            } else {
                out += span
            }
        }
        return out
    }
}

/** The plain text of spans, e.g. for copying or accessibility. */
fun List<Span>.plainText(): String = joinToString("") { it.text }

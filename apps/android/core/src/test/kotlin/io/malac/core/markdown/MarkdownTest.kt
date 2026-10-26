package io.malac.core.markdown

import io.malac.core.Fixtures
import io.malac.core.protocol.AgentEvent
import io.malac.core.protocol.ServerMessage
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertIs

class MarkdownTest {
    @Test
    fun theRecordedSummaryParses() {
        val text = Fixtures.receivedMessages.filterIsInstance<ServerMessage.Event>()
            .map { it.envelope.event }.filterIsInstance<AgentEvent.TurnFinished>()
            .first { it.text.startsWith("## Summary") }.text
        val blocks = Markdown.parse(text)
        val heading = assertIs<MarkdownBlock.Heading>(blocks[0])
        assertEquals(2, heading.level)
        assertEquals("Summary", heading.content.plainText())
        val list = assertIs<MarkdownBlock.ListBlock>(blocks[1])
        assertEquals(false, list.ordered)
        val first = assertIs<MarkdownBlock.Paragraph>(list.items[0].blocks.single())
        assertEquals(Span("Step 1 —", bold = true), first.content[0])
        val code = blocks.filterIsInstance<MarkdownBlock.Code>().single()
        assertEquals("rust", code.language)
        assertEquals("fn main() {\n    println!(\"hello\");\n}", code.code)
    }

    @Test
    fun inlineStyles() {
        assertEquals(
            listOf(
                Span("a "),
                Span("bold", bold = true),
                Span(" and "),
                Span("it", italic = true),
                Span(", "),
                Span("x * y", code = true),
                Span(", "),
                Span("gone", strike = true),
                Span(" "),
                Span("docs", link = "https://malac.io"),
            ),
            Markdown.inline("a **bold** and *it*, `x * y`, ~~gone~~ [docs](https://malac.io)"),
        )
        assertEquals(listOf(Span("snake_case_name")), Markdown.inline("snake_case_name"))
        assertEquals(
            listOf(Span("a "), Span("b ", bold = true), Span("c", bold = true, italic = true), Span(" d", bold = true)),
            Markdown.inline("a **b *c* d**"),
        )
        assertEquals(
            listOf(Span("see "), Span("https://example.com/a", link = "https://example.com/a"), Span(".")),
            Markdown.inline("see https://example.com/a."),
        )
        assertEquals(listOf(Span("bad")), Markdown.inline("[bad](javascript:alert)"))
    }

    @Test
    fun blocks() {
        val blocks = Markdown.parse(
            """
            # Title
            first line
            second line

            > quoted **text**

            1. one
            2. two
               - nested
            - [x] done

            | a | b |
            |:--|--:|
            | 1 | 2 |

            ---
            ```
            unclosed
            """.trimIndent(),
        )
        assertEquals(MarkdownBlock.Heading(1, listOf(Span("Title"))), blocks[0])
        assertEquals(MarkdownBlock.Paragraph(listOf(Span("first line\nsecond line"))), blocks[1])
        val quote = assertIs<MarkdownBlock.Quote>(blocks[2])
        assertEquals(MarkdownBlock.Paragraph(listOf(Span("quoted "), Span("text", bold = true))), quote.blocks.single())
        val ordered = assertIs<MarkdownBlock.ListBlock>(blocks[3])
        assertEquals(true, ordered.ordered)
        assertEquals(2, ordered.items.size)
        assertIs<MarkdownBlock.ListBlock>(ordered.items[1].blocks[1])
        val tasks = assertIs<MarkdownBlock.ListBlock>(blocks[4])
        assertEquals(true, tasks.items.single().task)
        val table = assertIs<MarkdownBlock.Table>(blocks[5])
        assertEquals(listOf(Alignment.Start, Alignment.End), table.alignments)
        assertEquals("2", table.rows.single()[1].plainText())
        assertEquals(MarkdownBlock.Rule, blocks[6])
        assertEquals(MarkdownBlock.Code("", "unclosed"), blocks[7])
    }
}

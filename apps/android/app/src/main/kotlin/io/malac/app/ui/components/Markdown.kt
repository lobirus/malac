package io.malac.app.ui.components

import android.content.ClipData
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.ClipEntry
import androidx.compose.ui.platform.LocalClipboard
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.withLink
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import io.malac.app.R
import io.malac.app.ui.theme.malac
import io.malac.core.markdown.Alignment as ColumnAlignment
import io.malac.core.markdown.Markdown
import io.malac.core.markdown.MarkdownBlock
import io.malac.core.markdown.Span
import kotlinx.coroutines.launch

/** Markdown as native text: parsed once per text, styled with the theme. */
@Composable
fun MarkdownText(markdown: String, modifier: Modifier = Modifier, style: TextStyle = MaterialTheme.typography.bodyMedium) {
    val blocks = remember(markdown) { Markdown.parse(markdown) }
    SelectionContainer(modifier) {
        Blocks(blocks, style)
    }
}

@Composable
private fun Blocks(blocks: List<MarkdownBlock>, style: TextStyle, spacing: Int = 8) {
    Column(verticalArrangement = Arrangement.spacedBy(spacing.dp)) {
        for (block in blocks) Block(block, style)
    }
}

@Composable
private fun Block(block: MarkdownBlock, style: TextStyle) {
    val typography = MaterialTheme.typography
    when (block) {
        is MarkdownBlock.Heading -> Text(
            text = rememberSpans(block.content),
            style = when (block.level) {
                1 -> typography.titleLarge
                2 -> typography.titleMedium
                else -> typography.titleSmall
            }.copy(fontWeight = FontWeight.SemiBold),
            modifier = Modifier.semantics { heading() },
        )
        is MarkdownBlock.Paragraph -> Text(rememberSpans(block.content), style = style)
        is MarkdownBlock.Code -> CodeBlock(block.code, block.language)
        is MarkdownBlock.Quote -> Row(Modifier.height(IntrinsicSize.Min)) {
            Box(Modifier.width(2.dp).fillMaxHeight().background(MaterialTheme.colorScheme.outline))
            Box(Modifier.padding(start = 12.dp)) {
                Blocks(block.blocks, style.copy(color = MaterialTheme.malac.text2))
            }
        }
        is MarkdownBlock.ListBlock -> Column(verticalArrangement = Arrangement.spacedBy(if (block.loose) 8.dp else 2.dp)) {
            block.items.forEachIndexed { index, item ->
                Row {
                    val marker = when {
                        item.task == true -> "☑"
                        item.task == false -> "☐"
                        block.ordered -> "${block.start + index}."
                        else -> "•"
                    }
                    Text(marker, style = style, color = MaterialTheme.malac.text2, modifier = Modifier.widthIn(min = 20.dp))
                    Box(Modifier.padding(start = 4.dp)) { Blocks(item.blocks, style, spacing = 4) }
                }
            }
        }
        is MarkdownBlock.Table -> Table(block, style)
        MarkdownBlock.Rule -> HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
    }
}

@Composable
fun CodeBlock(code: String, language: String, modifier: Modifier = Modifier) {
    val clipboard = LocalClipboard.current
    val scope = rememberCoroutineScope()
    Surface(
        modifier = modifier.fillMaxWidth(),
        shape = MaterialTheme.shapes.small,
        color = MaterialTheme.colorScheme.surfaceContainer,
        border = BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
    ) {
        Column {
            Row(Modifier.fillMaxWidth().padding(start = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(language, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.malac.text3, modifier = Modifier.weight(1f))
                TextButton(onClick = { scope.launch { clipboard.setClipEntry(ClipEntry(ClipData.newPlainText(language, code))) } }) {
                    Text(stringResource(R.string.copy_code), style = MaterialTheme.typography.labelSmall)
                }
            }
            Text(
                text = code,
                style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace),
                softWrap = false,
                modifier = Modifier.horizontalScroll(rememberScrollState()).padding(start = 12.dp, end = 12.dp, bottom = 12.dp),
            )
        }
    }
}

@Composable
private fun Table(table: MarkdownBlock.Table, style: TextStyle) {
    val border = MaterialTheme.colorScheme.outlineVariant
    Column(Modifier.horizontalScroll(rememberScrollState())) {
        val rows = listOf(table.header) + table.rows
        rows.forEachIndexed { index, row ->
            Row {
                row.forEachIndexed { column, cell ->
                    Text(
                        text = rememberSpans(cell),
                        style = if (index == 0) style.copy(fontWeight = FontWeight.SemiBold) else style,
                        textAlign = when (table.alignments.getOrNull(column)) {
                            ColumnAlignment.Center -> TextAlign.Center
                            ColumnAlignment.End -> TextAlign.End
                            else -> TextAlign.Start
                        },
                        modifier = Modifier.widthIn(min = 72.dp, max = 240.dp).padding(horizontal = 8.dp, vertical = 6.dp),
                    )
                }
            }
            HorizontalDivider(color = border)
        }
    }
}

@Composable
private fun rememberSpans(spans: List<Span>): AnnotatedString {
    val link = TextLinkStyles(SpanStyle(color = MaterialTheme.malac.green))
    val code = SpanStyle(fontFamily = FontFamily.Monospace, background = MaterialTheme.colorScheme.surfaceContainerHighest)
    return remember(spans, link, code) { annotated(spans, link, code) }
}

private fun annotated(spans: List<Span>, link: TextLinkStyles, code: SpanStyle): AnnotatedString = buildAnnotatedString {
    for (span in spans) {
        val style = SpanStyle(
            fontWeight = if (span.bold) FontWeight.SemiBold else null,
            fontStyle = if (span.italic) FontStyle.Italic else null,
            textDecoration = if (span.strike) TextDecoration.LineThrough else null,
        ).let { if (span.code) it.merge(code) else it }
        val url = span.link
        withStyle(style) {
            if (url != null) withLink(LinkAnnotation.Url(url, link)) { append(span.text) } else append(span.text)
        }
    }
}

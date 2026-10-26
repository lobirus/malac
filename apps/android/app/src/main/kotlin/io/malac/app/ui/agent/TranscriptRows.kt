package io.malac.app.ui.agent

import android.graphics.BitmapFactory
import android.util.Base64
import androidx.compose.animation.animateContentSize
import androidx.compose.foundation.Image
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.KeyboardArrowDown
import androidx.compose.material.icons.automirrored.filled.KeyboardArrowRight
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.malac.app.R
import io.malac.app.ui.components.MarkdownText
import io.malac.app.ui.theme.malac
import io.malac.core.format.baseName
import io.malac.core.format.firstLine
import io.malac.core.format.formatCost
import io.malac.core.format.formatDuration
import io.malac.core.format.formatTokens
import io.malac.core.format.formatUsage
import io.malac.core.protocol.Block
import io.malac.core.protocol.Level
import io.malac.core.protocol.TodoStatus
import io.malac.core.protocol.TurnStatus
import io.malac.core.transcript.Draft
import io.malac.core.transcript.TranscriptItem
import io.malac.core.transcript.TurnSummary
import io.malac.core.transcript.toolTitle

@Composable
fun UserRow(item: TranscriptItem.User) {
    Surface(
        color = MaterialTheme.colorScheme.surfaceContainerHigh,
        shape = MaterialTheme.shapes.medium,
        modifier = Modifier.fillMaxWidth(),
    ) {
        Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(stringResource(R.string.you), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.malac.text2)
            if (item.images.isNotEmpty()) Thumbnails(item.images)
            if (item.text.isNotEmpty()) SelectionContainer { Text(item.text, style = MaterialTheme.typography.bodyMedium) }
            if (item.attachments.isNotEmpty()) {
                FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    for (attachment in item.attachments) Pill(baseName(attachment.path))
                }
            }
        }
    }
}

@Composable
fun AssistantRow(item: TranscriptItem.Assistant) {
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        MarkdownText(item.markdown)
        if (item.interrupted) {
            Text(stringResource(R.string.interrupted), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.malac.text3)
        }
    }
}

/** A collapsed block, such as the model's reasoning, that opens on tap. */
@Composable
fun Disclosure(label: String, preview: String, body: String, live: Boolean = false) {
    var open by rememberSaveable { mutableStateOf(false) }
    Column(Modifier.animateContentSize()) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .clickable(role = Role.Button, onClickLabel = stringResource(if (open) R.string.collapse else R.string.expand)) { open = !open }
                .semantics(mergeDescendants = true) {}
                .heightIn(min = 40.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Icon(
                if (open) Icons.Filled.KeyboardArrowDown else Icons.AutoMirrored.Filled.KeyboardArrowRight,
                contentDescription = null,
                tint = MaterialTheme.malac.text3,
                modifier = Modifier.size(18.dp),
            )
            Text(label, style = MaterialTheme.typography.labelLarge, color = MaterialTheme.malac.text2)
            if (live) CircularProgressIndicator(Modifier.size(12.dp), strokeWidth = 1.5.dp)
            if (!open) {
                Text(
                    preview,
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.malac.text3,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f),
                )
            }
        }
        if (open) {
            SelectionContainer {
                Text(body, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.malac.text2, modifier = Modifier.padding(start = 24.dp))
            }
        }
    }
}

@Composable
fun PlanCard(item: TranscriptItem.Plan) {
    val done = item.items.count { it.status == TodoStatus.Completed }
    var open by rememberSaveable(item.key, item.superseded) { mutableStateOf(!item.superseded) }
    Column(Modifier.animateContentSize()) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .clickable(role = Role.Button) { open = !open }
                .semantics(mergeDescendants = true) {}
                .heightIn(min = 40.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(stringResource(R.string.plan), style = MaterialTheme.typography.labelLarge)
            Text("$done/${item.items.size}", style = MaterialTheme.typography.labelMedium, color = MaterialTheme.malac.text2)
        }
        if (open) {
            for (todo in item.items) {
                val (mark, color) = when (todo.status) {
                    TodoStatus.Completed -> "✓" to MaterialTheme.malac.green
                    TodoStatus.InProgress -> "•" to MaterialTheme.colorScheme.onSurface
                    TodoStatus.Pending -> "○" to MaterialTheme.malac.text3
                }
                val state = stringResource(
                    when (todo.status) {
                        TodoStatus.Completed -> R.string.todo_done
                        TodoStatus.InProgress -> R.string.todo_in_progress
                        TodoStatus.Pending -> R.string.todo_pending
                    },
                )
                Row(Modifier.semantics(mergeDescendants = true) { stateDescription = state }.padding(vertical = 2.dp)) {
                    Text(mark, color = color, modifier = Modifier.padding(end = 8.dp))
                    Text(
                        todo.content,
                        style = MaterialTheme.typography.bodyMedium,
                        color = if (todo.status == TodoStatus.Completed) MaterialTheme.malac.text3 else MaterialTheme.colorScheme.onSurface,
                    )
                }
            }
            item.error?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.malac.red) }
        }
    }
}

@Composable
fun EventLine(text: String) {
    Text(text, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.malac.text3)
}

@Composable
fun NoticeRow(item: TranscriptItem.Notice) {
    val color = when (item.level) {
        Level.Error -> MaterialTheme.malac.red
        Level.Warning -> MaterialTheme.malac.amber
        Level.Info -> MaterialTheme.malac.text2
    }
    Text(item.message, style = MaterialTheme.typography.bodySmall, color = color)
}

/** Duration, tokens, cost and context of a finished turn, and its status unless it completed. */
@Composable
fun TurnFooter(summary: TurnSummary) {
    val parts = buildList {
        when (summary.status) {
            TurnStatus.Completed -> Unit
            TurnStatus.Failed -> add(stringResource(R.string.phase_failed))
            TurnStatus.Cancelled -> add(stringResource(R.string.phase_stopped))
        }
        summary.durationMillis?.let { add(formatDuration(it)) }
        summary.usage?.let { usage ->
            add(formatUsage(usage))
            formatCost(usage.cost).takeIf { it.isNotEmpty() }?.let { add(it) }
        }
        if (summary.contextTokens > 0) add(stringResource(R.string.context_tokens, formatTokens(summary.contextTokens)))
    }
    Column {
        HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
        Text(
            parts.joinToString(" · "),
            style = MaterialTheme.typography.labelSmall,
            color = if (summary.status == TurnStatus.Failed) MaterialTheme.malac.red else MaterialTheme.malac.text3,
            modifier = Modifier.padding(top = 6.dp),
        )
    }
}

/** What the agent is streaming right now. */
@Composable
fun DraftRow(draft: Draft) {
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        if (draft.thinking.isNotEmpty()) {
            Disclosure(stringResource(R.string.thinking), firstLine(draft.thinking.takeLast(200)), draft.thinking, live = draft.text.isEmpty())
        }
        if (draft.text.isNotEmpty()) MarkdownText(draft.text)
        draft.preparingTool?.let { tool ->
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                CircularProgressIndicator(Modifier.size(12.dp), strokeWidth = 1.5.dp)
                Text(
                    stringResource(R.string.preparing_tool, toolTitle(tool).title.lowercase()),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.malac.text2,
                )
            }
        }
    }
}

@Composable
fun Pill(text: String) {
    Surface(
        shape = RoundedCornerShape(50),
        color = MaterialTheme.colorScheme.surfaceContainerHighest,
    ) {
        Text(
            text,
            style = MaterialTheme.typography.labelSmall,
            color = MaterialTheme.malac.text2,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.padding(horizontal = 8.dp, vertical = 2.dp),
        )
    }
}

/** Images from the transcript, decoded once each. */
@Composable
fun Thumbnails(images: List<Block.Image>) {
    val description = pluralStringResource(R.plurals.images, images.size, images.size)
    Row(
        Modifier.horizontalScroll(rememberScrollState()).semantics(mergeDescendants = true) {},
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        for (image in images) {
            val bitmap = remember(image.data) {
                runCatching { Base64.decode(image.data, Base64.DEFAULT) }.getOrNull()
                    ?.let { BitmapFactory.decodeByteArray(it, 0, it.size) }
                    ?.asImageBitmap()
            }
            if (bitmap != null) {
                Image(
                    bitmap = bitmap,
                    contentDescription = description,
                    contentScale = ContentScale.Crop,
                    modifier = Modifier.size(96.dp).clip(MaterialTheme.shapes.small),
                )
            }
        }
    }
}

/** Monospace output, cut to [previewLines] until expanded. */
@Composable
fun OutputBlock(text: String, isError: Boolean = false, previewLines: Int = 24) {
    val lines = remember(text) { text.lines() }
    var all by rememberSaveable(text) { mutableStateOf(false) }
    val long = lines.size > previewLines * 2
    Column {
        Surface(
            color = MaterialTheme.colorScheme.surfaceContainer,
            shape = MaterialTheme.shapes.small,
            modifier = Modifier.fillMaxWidth(),
        ) {
            SelectionContainer {
                Text(
                    if (long && !all) lines.take(previewLines).joinToString("\n") else text,
                    style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace),
                    color = if (isError) MaterialTheme.malac.red else MaterialTheme.colorScheme.onSurface,
                    softWrap = false,
                    modifier = Modifier.horizontalScroll(rememberScrollState()).padding(10.dp),
                )
            }
        }
        if (long) {
            Text(
                stringResource(if (all) R.string.show_less else R.string.show_all_lines, lines.size),
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.malac.green,
                modifier = Modifier
                    .clickable(role = Role.Button) { all = !all }
                    .heightIn(min = 48.dp)
                    .padding(vertical = 14.dp),
            )
        }
    }
}

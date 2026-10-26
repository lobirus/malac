package io.malac.app.ui.agent

import androidx.compose.animation.animateContentSize
import androidx.compose.foundation.background
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
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
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
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.malac.app.R
import io.malac.app.ui.components.CodeBlock
import io.malac.app.ui.theme.malac
import io.malac.core.format.firstLine
import io.malac.core.format.formatDuration
import io.malac.core.format.relativePath
import io.malac.core.protocol.Decision
import io.malac.core.protocol.MalacJson
import io.malac.core.transcript.DiffLine
import io.malac.core.transcript.ToolCall
import io.malac.core.transcript.ToolState
import io.malac.core.transcript.describeTool
import io.malac.core.transcript.diffLines
import io.malac.core.transcript.toolTitle
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull

private val PrettyJson = Json(MalacJson) { prettyPrint = true }

/**
 * A tool call as one compact row that expands to its input and output. A
 * call waiting for approval shows its buttons without being expanded.
 */
@Composable
fun ToolRow(call: ToolCall, cwd: String?, answering: Boolean, onDecide: (String, Decision) -> Unit) {
    val title = remember(call.name) { toolTitle(call.name) }
    val detail = remember(call.input, cwd) { describeTool(call.name, call.input, cwd) }
    val diff = remember(call.input) { if (call.name == "edit") diffLines(call.input.text("old_string"), call.input.text("new_string")) else null }
    var open by rememberSaveable(call.id) { mutableStateOf(diff != null && diff.count { it.op != DiffLine.Op.Same } <= 16) }
    val state = stateText(call.state)
    Column(Modifier.animateContentSize()) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .clickable(role = Role.Button, onClickLabel = stringResource(if (open) R.string.collapse else R.string.expand)) { open = !open }
                .semantics(mergeDescendants = true) { stateDescription = state }
                .heightIn(min = 44.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            StateMark(call.state)
            Text(title.title, style = MaterialTheme.typography.labelLarge)
            title.tag?.let { Pill(it) }
            Text(
                detail,
                style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace),
                color = MaterialTheme.malac.text2,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )
            diff?.let { lines ->
                Text("+${lines.count { it.op == DiffLine.Op.Added }}", style = MaterialTheme.typography.labelSmall, color = MaterialTheme.malac.green)
                Text("−${lines.count { it.op == DiffLine.Op.Removed }}", style = MaterialTheme.typography.labelSmall, color = MaterialTheme.malac.red)
            }
            call.millis?.takeIf { call.isFinished }?.let {
                Text(formatDuration(it), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.malac.text3)
            }
        }
        if (call.state == ToolState.Failed || call.state == ToolState.Denied) {
            Text(
                firstLine(call.output).ifEmpty { stringResource(R.string.phase_failed) },
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.malac.red,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.padding(start = 24.dp, bottom = 4.dp),
            )
        }
        val requestId = call.requestId
        if (call.state == ToolState.AwaitingApproval && requestId != null) {
            ApprovalButtons(enabled = !answering, onDecide = { onDecide(requestId, it) }, modifier = Modifier.padding(start = 24.dp, bottom = 8.dp))
        }
        if (open) {
            Column(Modifier.padding(start = 24.dp, bottom = 8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                ToolInput(call, cwd, diff)
                ToolOutput(call)
                if (call.images.isNotEmpty()) Thumbnails(call.images)
            }
        }
    }
}

@Composable
private fun ToolInput(call: ToolCall, cwd: String?, diff: List<DiffLine>?) {
    val input = call.input
    when {
        call.name == "bash" -> CodeBlock(input.text("command"), "sh")
        diff != null -> {
            Text(relativePath(input.text("path"), cwd), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.malac.text2)
            DiffView(diff)
        }
        call.name == "write" -> {
            Text(relativePath(input.text("path"), cwd), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.malac.text2)
            OutputBlock(input.text("content"))
        }
        call.name in setOf("read", "glob", "grep", "web_fetch") -> Unit
        (input as? JsonObject)?.isNotEmpty() == true -> CodeBlock(PrettyJson.encodeToString(JsonElement.serializer(), input), "json")
    }
}

@Composable
private fun ToolOutput(call: ToolCall) {
    val output = call.output
    when {
        output == null && !call.isFinished -> Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            if (call.state == ToolState.Running || call.state == ToolState.Pending) CircularProgressIndicator(Modifier.size(12.dp), strokeWidth = 1.5.dp)
            Text(stateText(call.state), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.malac.text2)
        }
        output == null -> Unit
        // An edit or write that worked says so in its header; its output adds nothing.
        (call.name == "edit" || call.name == "write") && !call.isError -> Unit
        else -> OutputBlock(output.ifEmpty { stringResource(R.string.no_output) }, isError = call.isError)
    }
}

@Composable
private fun DiffView(lines: List<DiffLine>) {
    val colors = MaterialTheme.malac
    Surface(color = MaterialTheme.colorScheme.surfaceContainer, shape = MaterialTheme.shapes.small, modifier = Modifier.fillMaxWidth()) {
        Column(Modifier.horizontalScroll(rememberScrollState()).padding(vertical = 6.dp)) {
            for (line in lines) {
                val (sign, background) = when (line.op) {
                    DiffLine.Op.Added -> "+" to colors.diffAdd
                    DiffLine.Op.Removed -> "−" to colors.diffDel
                    DiffLine.Op.Same -> " " to MaterialTheme.colorScheme.surfaceContainer
                }
                Row(Modifier.background(background).padding(horizontal = 8.dp)) {
                    Text(sign, style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace), modifier = Modifier.width(14.dp))
                    Text(line.text, style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace), softWrap = false)
                }
            }
        }
    }
}

@Composable
fun ApprovalButtons(enabled: Boolean, onDecide: (Decision) -> Unit, modifier: Modifier = Modifier) {
    FlowRow(modifier, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Button(onClick = { onDecide(Decision.Allow) }, enabled = enabled) { Text(stringResource(R.string.allow)) }
        OutlinedButton(onClick = { onDecide(Decision.AllowSession) }, enabled = enabled) { Text(stringResource(R.string.allow_always)) }
        OutlinedButton(
            onClick = { onDecide(Decision.Deny) },
            enabled = enabled,
            colors = ButtonDefaults.outlinedButtonColors(contentColor = MaterialTheme.malac.red),
        ) { Text(stringResource(R.string.deny)) }
    }
}

@Composable
private fun StateMark(state: ToolState) {
    val colors = MaterialTheme.malac
    when (state) {
        ToolState.Running, ToolState.Pending -> CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 1.5.dp)
        else -> {
            val (mark, color) = when (state) {
                ToolState.Done -> "✓" to colors.text3
                ToolState.Failed, ToolState.Denied -> "✕" to colors.red
                ToolState.AwaitingApproval -> "!" to colors.amber
                else -> "–" to colors.text3
            }
            Text(mark, color = color, style = MaterialTheme.typography.labelLarge, modifier = Modifier.width(16.dp))
        }
    }
}

@Composable
private fun stateText(state: ToolState): String = stringResource(
    when (state) {
        ToolState.Pending -> R.string.tool_pending
        ToolState.Running -> R.string.phase_running
        ToolState.AwaitingApproval -> R.string.phase_approval
        ToolState.Done -> R.string.phase_done
        ToolState.Failed -> R.string.phase_failed
        ToolState.Denied -> R.string.tool_denied
        ToolState.Skipped -> R.string.tool_skipped
    },
)

private fun JsonElement.text(key: String): String = ((this as? JsonObject)?.get(key) as? JsonPrimitive)?.contentOrNull.orEmpty()

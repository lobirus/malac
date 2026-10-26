package io.malac.app.ui.agent

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.malac.app.R
import io.malac.app.ui.components.MarkdownText
import io.malac.app.ui.components.Panel
import io.malac.app.ui.components.StatusDot
import io.malac.app.ui.components.phaseColor
import io.malac.app.ui.label
import io.malac.app.ui.theme.malac
import io.malac.core.format.firstLine
import io.malac.core.format.formatCost
import io.malac.core.format.formatDuration
import io.malac.core.format.formatTokens
import io.malac.core.protocol.AgentStatus
import io.malac.core.state.AgentState
import io.malac.core.state.Phase
import io.malac.core.transcript.SubagentCall
import java.time.Duration
import java.time.Instant

/** An `agent_spawn` call: the subagent's live status, its task and report, and a way into its transcript. */
@Composable
fun SubagentCard(call: SubagentCall, child: AgentState?, waiting: Boolean, nested: Int, onOpen: (String) -> Unit) {
    val status = call.status(child)
    val phase = if (waiting) Phase.AwaitingApproval else Phase.of(status)
    val label = if (call.abandoned && child == null) stringResource(R.string.not_started) else phase.label()
    Panel {
        Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Row(
                Modifier.semantics(mergeDescendants = true) { stateDescription = label },
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                StatusDot(phase)
                Text(call.label, style = MaterialTheme.typography.titleSmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
                call.agentType?.let { Pill(it) }
                if (call.readOnly) Pill(stringResource(R.string.read_only))
                Text(label, style = MaterialTheme.typography.labelMedium, color = phaseColor(phase), modifier = Modifier.weight(1f))
            }
            val millis = call.millis
            val started = child?.created
            val meta = buildList {
                child?.model?.takeIf { it.isNotEmpty() }?.let { add(it) }
                when {
                    millis != null && status != AgentStatus.Running -> add(formatDuration(millis))
                    status == AgentStatus.Running && started != null -> add(formatDuration(Duration.between(started, Instant.now()).toMillis()))
                }
                val tokens = child?.let { it.usage.inputTokens + it.usage.outputTokens } ?: 0
                if (tokens > 0) add("${formatTokens(tokens)} tok")
                child?.usage?.cost?.let(::formatCost)?.takeIf { it.isNotEmpty() }?.let { add(it) }
                if (nested > 0) add(pluralStringResource(R.plurals.subagents, nested, nested))
            }
            if (meta.isNotEmpty()) {
                Text(meta.joinToString(" · "), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.malac.text3)
            }
            val activity = child?.activity.orEmpty()
            if (status == AgentStatus.Running && activity.isNotEmpty()) {
                Text(activity, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.malac.text2, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            if (call.task.isNotEmpty()) Disclosure(stringResource(R.string.task), firstLine(call.task), call.task)
            val report = call.report(child)
            if (report.isNotEmpty()) MarkdownText(report, style = MaterialTheme.typography.bodySmall)
            val childId = call.child
            if (childId != null) {
                TextButton(onClick = { onOpen(childId) }, contentPadding = PaddingValues(horizontal = 0.dp)) {
                    Text(stringResource(R.string.open_transcript))
                }
            }
        }
    }
}

package io.malac.app.ui.approvals

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import io.malac.app.R
import io.malac.app.ui.Commands
import io.malac.app.ui.agent.ApprovalButtons
import io.malac.app.ui.components.EmptyState
import io.malac.app.ui.components.Panel
import io.malac.app.ui.components.ScreenTopBar
import io.malac.app.ui.theme.malac
import io.malac.core.format.truncate
import io.malac.core.net.DaemonConnection
import io.malac.core.protocol.PendingApproval
import io.malac.core.state.DaemonState

/**
 * Every request waiting for an answer, from every workspace. A request
 * leaves the list as soon as any client answers it.
 */
@Composable
fun ApprovalsScreen(state: DaemonState, status: DaemonConnection.Status, commands: Commands, onOpenAgent: (String) -> Unit) {
    Column(Modifier.fillMaxSize()) {
        ScreenTopBar(title = stringResource(R.string.nav_approvals))
        val pending = state.pending.values.toList()
        if (pending.isEmpty()) {
            EmptyState(stringResource(R.string.no_approvals), stringResource(R.string.no_approvals_body))
            return@Column
        }
        LazyColumn(contentPadding = PaddingValues(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            items(pending, key = { it.id }) { approval ->
                ApprovalCard(
                    approval = approval,
                    place = state.place(approval),
                    enabled = status == DaemonConnection.Status.Online && approval.id !in commands.answering,
                    commands = commands,
                    onOpenAgent = onOpenAgent,
                )
            }
        }
    }
}

@Composable
private fun ApprovalCard(
    approval: PendingApproval,
    place: String,
    enabled: Boolean,
    commands: Commands,
    onOpenAgent: (String) -> Unit,
) {
    Panel {
        Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Column(
                Modifier
                    .clickable(role = Role.Button, onClickLabel = stringResource(R.string.open_transcript)) { onOpenAgent(approval.agent) }
                    .semantics(mergeDescendants = true) {},
                verticalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                Text(
                    stringResource(R.string.approval_title, approval.agentLabel.ifEmpty { "agent" }, approval.tool),
                    style = MaterialTheme.typography.titleSmall,
                )
                if (place.isNotEmpty()) Text(place, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.malac.text2)
                if (approval.summary.isNotEmpty()) {
                    Text(
                        truncate(approval.summary, 600),
                        style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace),
                    )
                }
            }
            ApprovalButtons(enabled = enabled, onDecide = { commands.approve(approval.id, it) })
        }
    }
}

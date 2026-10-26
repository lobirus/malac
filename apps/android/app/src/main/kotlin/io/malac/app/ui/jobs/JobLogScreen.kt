package io.malac.app.ui.jobs

import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import io.malac.app.R
import io.malac.app.connection.ActiveDaemon
import io.malac.app.ui.Commands
import io.malac.app.ui.components.EmptyState
import io.malac.app.ui.components.ScreenTopBar
import io.malac.app.ui.label
import io.malac.app.ui.theme.malac
import io.malac.core.format.displayPath
import io.malac.core.net.CommandException
import io.malac.core.protocol.Command
import io.malac.core.protocol.JobOutput
import io.malac.core.protocol.ServerMessage
import io.malac.core.state.DaemonState
import kotlinx.coroutines.delay

private const val TAIL_LINES = 400
private const val POLL_MILLIS = 2_000L

/** A job's log tail, refreshed every two seconds while it runs. */
@Composable
fun JobLogScreen(active: ActiveDaemon, state: DaemonState, commands: Commands, id: String, onBack: () -> Unit) {
    val job = state.jobs[id]
    var output by remember { mutableStateOf<JobOutput?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var refresh by remember { mutableIntStateOf(0) }
    var confirmStop by remember { mutableStateOf(false) }
    val running = job?.isRunning == true

    LaunchedEffect(id, running, refresh) {
        do {
            try {
                output = (active.connection.request(Command.JobOutputRequest(id, TAIL_LINES)) as? ServerMessage.JobOutputReply)?.output
                error = null
            } catch (failure: CommandException) {
                error = failure.message
            }
            if (running) delay(POLL_MILLIS)
        } while (running)
    }

    Column(Modifier.fillMaxSize()) {
        ScreenTopBar(
            title = job?.command.orEmpty(),
            subtitle = job?.let { listOfNotNull(it.phase.label(), exitText(it), displayPath(it.cwd, state.daemon?.userHome)).joinToString(" · ") },
            onBack = onBack,
            actions = {
                IconButton(onClick = { refresh++ }) {
                    Icon(Icons.Filled.Refresh, contentDescription = stringResource(R.string.refresh))
                }
                if (running) {
                    IconButton(onClick = { confirmStop = true }) {
                        Icon(painterResource(R.drawable.ic_stop), contentDescription = stringResource(R.string.stop_job), tint = MaterialTheme.malac.red)
                    }
                }
            },
        )
        if (job == null) {
            EmptyState(stringResource(R.string.job_gone))
            return@Column
        }
        val scroll = rememberScrollState()
        LaunchedEffect(output?.text) { scroll.scrollTo(scroll.maxValue) }
        Column(
            Modifier.fillMaxSize().verticalScroll(scroll).padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            output?.takeIf { it.truncated }?.let {
                Text(
                    stringResource(R.string.job_tail, TAIL_LINES, (it.size / 1024).toInt()),
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.malac.text3,
                )
            }
            error?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.malac.red) }
            SelectionContainer {
                Text(
                    output?.text?.ifEmpty { stringResource(R.string.no_output_yet) } ?: stringResource(R.string.loading),
                    style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace),
                    softWrap = false,
                    modifier = Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()),
                )
            }
        }
    }

    if (confirmStop && job != null) {
        AlertDialog(
            onDismissRequest = { confirmStop = false },
            title = { Text(stringResource(R.string.stop_job_title)) },
            text = { Text(stringResource(R.string.stop_job_body, job.command)) },
            confirmButton = {
                TextButton(onClick = {
                    confirmStop = false
                    commands.send(Command.JobKill(job.id))
                }) { Text(stringResource(R.string.stop_job), color = MaterialTheme.malac.red) }
            },
            dismissButton = { TextButton(onClick = { confirmStop = false }) { Text(stringResource(R.string.cancel)) } },
        )
    }
}

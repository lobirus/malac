package io.malac.app.ui.jobs

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import io.malac.app.R
import io.malac.app.connection.ActiveDaemon
import io.malac.app.ui.components.ConnectionBanner
import io.malac.app.ui.components.EmptyState
import io.malac.app.ui.components.ScreenTopBar
import io.malac.app.ui.workspaces.StatusRow
import io.malac.core.format.displayPath
import io.malac.core.format.formatDuration
import io.malac.core.net.DaemonConnection
import io.malac.core.protocol.JobStatus
import io.malac.core.protocol.JobView
import io.malac.core.state.DaemonState
import io.malac.core.state.Phase
import kotlinx.coroutines.delay
import java.time.Duration
import java.time.Instant

@Composable
fun JobsScreen(state: DaemonState, status: DaemonConnection.Status, active: ActiveDaemon, onOpenJob: (String) -> Unit) {
    // Runtimes of running jobs tick once a second.
    var now by remember { mutableStateOf(Instant.now()) }
    val anyRunning = state.jobs.values.any { it.isRunning }
    LaunchedEffect(anyRunning) {
        while (anyRunning) {
            delay(1_000)
            now = Instant.now()
        }
    }
    Column(Modifier.fillMaxSize()) {
        ScreenTopBar(title = stringResource(R.string.nav_jobs))
        ConnectionBanner(status, state.shutdownReason, active.connection::reconnectNow)
        val jobs = state.jobs.values.sortedByDescending { it.started }
        if (jobs.isEmpty()) {
            EmptyState(stringResource(R.string.no_jobs), stringResource(R.string.no_jobs_body))
            return@Column
        }
        LazyColumn {
            items(jobs, key = { it.id }) { job ->
                val detail = listOfNotNull(displayPath(job.cwd, state.daemon?.userHome), runtime(job, now), exitText(job)).joinToString(" · ")
                StatusRow(job.phase, job.command, detail, onClick = { onOpenJob(job.id) }, showPhase = job.isRunning)
            }
        }
    }
}

val JobView.phase: Phase
    get() = when (status) {
        JobStatus.Running -> Phase.Running
        JobStatus.Killed -> Phase.Stopped
        JobStatus.Exited -> if (exitCode == null || exitCode == 0) Phase.Done else Phase.Failed
        JobStatus.Unknown -> Phase.Idle
    }

fun runtime(job: JobView, now: Instant): String? {
    val started = job.started ?: return null
    return formatDuration(Duration.between(started, job.finished ?: now).toMillis())
}

@Composable
fun exitText(job: JobView): String? = when (job.status) {
    JobStatus.Running -> null
    JobStatus.Exited -> job.exitCode?.let { stringResource(R.string.job_exited_code, it) } ?: stringResource(R.string.job_exited)
    JobStatus.Killed -> stringResource(R.string.job_killed)
    JobStatus.Unknown -> stringResource(R.string.job_unknown)
}

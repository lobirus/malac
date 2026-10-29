package io.malac.app.notify

import android.content.Context
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import androidx.work.workDataOf
import io.malac.app.AppGraph
import io.malac.app.MalacApplication
import io.malac.app.data.SavedDaemon
import io.malac.core.net.DaemonApi
import io.malac.core.protocol.AgentStatus
import io.malac.core.protocol.TurnStatus
import io.malac.core.state.DaemonState
import io.malac.core.state.check
import kotlinx.coroutines.flow.first
import java.util.concurrent.TimeUnit

/**
 * While the app is in the background its WebSocket is closed, so every
 * saved daemon is polled about every 15 minutes, the shortest period
 * WorkManager allows, for new approvals and finished agents; a push that
 * names requests without their details checks its daemon at once.
 */
class BackgroundCheckWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result {
        val graph = (applicationContext as MalacApplication).graph
        if (graph.connections.inForeground) return Result.success()
        val only = inputData.getString(DAEMON)
        for (daemon in graph.daemons.daemons.first()) {
            if (only == null || daemon.id == only) check(graph, daemon)
        }
        return Result.success()
    }

    private suspend fun check(graph: AppGraph, daemon: SavedDaemon) {
        val endpoint = graph.daemons.endpoint(daemon) ?: return
        val snapshot = runCatching { DaemonApi(graph.http, endpoint).snapshot() }.getOrNull() ?: return
        val result = check(snapshot, graph.ledgers.get(daemon.id))
        val state = DaemonState().reduce(snapshot).state
        result.approvals.forEach { graph.notifier.approval(daemon, it, state.place(it)) }
        // Answered elsewhere, or gone with their agent.
        result.resolved.forEach(graph.notifier::cancelApproval)
        for (agent in result.finished) {
            val status = when (agent.status) {
                AgentStatus.Failed -> TurnStatus.Failed
                AgentStatus.Cancelled -> TurnStatus.Cancelled
                else -> TurnStatus.Completed
            }
            val name = agent.title.ifEmpty { agent.label }
            graph.notifier.turnFinished(daemon, agent.id, name, status, agent.report.orEmpty())
        }
        graph.ledgers.put(daemon.id, result.ledger)
    }

    companion object {
        private const val NAME = "background-check"
        private const val DAEMON = "daemon"

        private val online = Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build()

        fun schedule(context: Context) {
            val request = PeriodicWorkRequestBuilder<BackgroundCheckWorker>(15, TimeUnit.MINUTES)
                .setConstraints(online)
                .build()
            WorkManager.getInstance(context).enqueueUniquePeriodicWork(NAME, ExistingPeriodicWorkPolicy.KEEP, request)
        }

        /** Checks one daemon as soon as the network allows. */
        fun checkNow(context: Context, daemon: String) {
            val request = OneTimeWorkRequestBuilder<BackgroundCheckWorker>()
                .setInputData(workDataOf(DAEMON to daemon))
                .setConstraints(online)
                .build()
            WorkManager.getInstance(context).enqueueUniqueWork("$NAME-$daemon", ExistingWorkPolicy.REPLACE, request)
        }
    }
}

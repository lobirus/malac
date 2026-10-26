package io.malac.app.notify

import android.content.Context
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.NetworkType
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import io.malac.app.MalacApplication
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
 * WorkManager allows, for new approvals and finished agents.
 */
class BackgroundCheckWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result {
        val graph = (applicationContext as MalacApplication).graph
        if (graph.connections.inForeground) return Result.success()
        for (daemon in graph.daemons.daemons.first()) {
            val endpoint = graph.daemons.endpoint(daemon) ?: continue
            val snapshot = runCatching { DaemonApi(graph.http, endpoint).snapshot() }.getOrNull() ?: continue
            val result = check(snapshot, graph.ledgers.get(daemon.id))
            val state = DaemonState().reduce(snapshot).state
            result.approvals.forEach { graph.notifier.approval(daemon, it, state.place(it)) }
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
        return Result.success()
    }

    companion object {
        private const val NAME = "background-check"

        fun schedule(context: Context) {
            val request = PeriodicWorkRequestBuilder<BackgroundCheckWorker>(15, TimeUnit.MINUTES)
                .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build())
                .build()
            WorkManager.getInstance(context).enqueueUniquePeriodicWork(NAME, ExistingPeriodicWorkPolicy.KEEP, request)
        }
    }
}

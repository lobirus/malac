package io.malac.app.notify

import android.app.Activity
import android.content.Context
import android.content.pm.PackageManager
import android.util.Log
import io.malac.app.connection.ConnectionManager
import io.malac.app.data.DaemonRepository
import io.malac.app.data.LedgerStore
import io.malac.app.data.SavedDaemon
import io.malac.core.net.DaemonApi
import io.malac.core.state.Push
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import okhttp3.OkHttpClient
import org.unifiedpush.android.connector.UnifiedPush
import org.unifiedpush.android.connector.data.PushEndpoint

/** Whether approvals come by push, and if not, what it takes. */
sealed interface PushState {
    /** No UnifiedPush distributor is installed. */
    data object Unavailable : PushState

    data object Off : PushState

    /** Pushes come through [distributor], an app's name. */
    data class On(val distributor: String) : PushState
}

/**
 * Push through UnifiedPush: a distributor app (ntfy, for one) keeps one
 * connection for every app that uses it and wakes malac when a daemon
 * pushes. Each saved daemon is its own registration, with the daemon's
 * VAPID key; the endpoint the distributor gives back goes to the daemon
 * from [MalacPushService]. The pushes carry what a notification shows,
 * so one posts even when the daemon cannot be reached yet.
 */
class UnifiedPushRegistrar(
    private val context: Context,
    private val http: OkHttpClient,
    private val daemons: DaemonRepository,
    private val ledgers: LedgerStore,
    private val notifier: Notifier,
    private val connections: ConnectionManager,
    private val scope: CoroutineScope,
) : PushRegistrar {
    private val mutableState = MutableStateFlow(currentState())
    val state: StateFlow<PushState> = mutableState.asStateFlow()

    /** Reads the state again, e.g. after a distributor was installed. */
    fun refresh() {
        mutableState.value = currentState()
    }

    private fun currentState(): PushState {
        val distributor = UnifiedPush.getAckDistributor(context) ?: UnifiedPush.getSavedDistributor(context)
        return when {
            distributor != null -> PushState.On(appName(distributor))
            UnifiedPush.getDistributors(context).isEmpty() -> PushState.Unavailable
            else -> PushState.Off
        }
    }

    private fun appName(packageName: String): String = try {
        val info = context.packageManager.getApplicationInfo(packageName, 0)
        context.packageManager.getApplicationLabel(info).toString()
    } catch (_: PackageManager.NameNotFoundException) {
        packageName
    }

    /** Picks the distributor (asking when there are several) and registers every daemon. */
    fun turnOn(activity: Activity) {
        UnifiedPush.tryUseCurrentOrDefaultDistributor(activity) { chosen ->
            refresh()
            if (chosen) scope.launch { registerAll() }
        }
    }

    /** Unregisters every daemon, which also lets the distributor go. */
    fun turnOff() {
        scope.launch {
            daemons.daemons.first().forEach { unregister(it) }
            refresh()
        }
    }

    suspend fun registerAll() {
        if (UnifiedPush.getSavedDistributor(context) == null) return
        daemons.daemons.first().forEach { register(it) }
    }

    override suspend fun register(daemon: SavedDaemon) {
        if (UnifiedPush.getSavedDistributor(context) == null) return
        val api = daemons.endpoint(daemon)?.let { DaemonApi(http, it) } ?: return
        // Daemons older than push support answer 404, and then there is nothing to register with.
        val status = runCatching { api.push() }.getOrNull() ?: return
        if (!status.enabled || status.vapidKey.isEmpty()) return
        UnifiedPush.register(context, instance = daemon.id, vapid = status.vapidKey)
    }

    override suspend fun unregister(daemon: SavedDaemon) {
        UnifiedPush.unregister(context, daemon.id)
        daemons.endpoint(daemon)?.let { endpoint -> runCatching { DaemonApi(http, endpoint).unregisterPush() } }
    }

    /** The distributor's endpoint for a daemon: the daemon pushes there from now on. */
    suspend fun newEndpoint(daemonId: String, endpoint: PushEndpoint) {
        // The daemon sends encrypted pushes only.
        val keys = endpoint.pubKeySet ?: return
        val daemon = daemon(daemonId) ?: return
        val api = daemons.endpoint(daemon)?.let { DaemonApi(http, it) } ?: return
        runCatching { api.registerPush(endpoint.url, keys.pubKey, keys.auth) }
            .onFailure { Log.w(TAG, "cannot register for pushes with ${daemon.name}", it) }
        refresh()
    }

    suspend fun unregistered(daemonId: String) {
        daemon(daemonId)?.let { daemon ->
            daemons.endpoint(daemon)?.let { endpoint -> runCatching { DaemonApi(http, endpoint).unregisterPush() } }
        }
        refresh()
    }

    /** A push from a daemon; `null` when it could not be read, and the daemon is asked instead. */
    suspend fun received(daemonId: String, push: Push?) {
        val daemon = daemon(daemonId) ?: return
        when (push) {
            is Push.Approvals -> {
                // In front, on this daemon, its own connection shows requests as they come.
                val watching = connections.inForeground && connections.active.value?.daemon?.id == daemonId
                if (!watching) {
                    val ledger = ledgers.get(daemonId)
                    val fresh = push.notify.filter { it.approval.id !in ledger.requests }
                    fresh.forEach { notifier.approval(daemon, it.approval, it.place) }
                    ledgers.put(daemonId, ledger.copy(requests = ledger.requests + fresh.map { it.approval.id }))
                }
                if (push.fetch) BackgroundCheckWorker.checkNow(context, daemonId)
            }
            is Push.Resolved -> {
                push.requests.forEach(notifier::cancelApproval)
                val ledger = ledgers.get(daemonId)
                ledgers.put(daemonId, ledger.copy(requests = ledger.requests - push.requests))
            }
            null -> BackgroundCheckWorker.checkNow(context, daemonId)
        }
    }

    private suspend fun daemon(id: String): SavedDaemon? = daemons.daemons.first().firstOrNull { it.id == id }

    private companion object {
        const val TAG = "malac-push"
    }
}

package io.malac.app.connection

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.ProcessLifecycleOwner
import io.malac.app.data.DaemonRepository
import io.malac.app.data.LedgerStore
import io.malac.app.data.SavedDaemon
import io.malac.app.files.SharedFileStore
import io.malac.app.notify.Notifier
import io.malac.core.net.DaemonApi
import io.malac.core.net.DaemonConnection
import io.malac.core.protocol.AgentStatus
import io.malac.core.state.Effect
import io.malac.core.state.Ledger
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import okhttp3.OkHttpClient

/** The selected daemon, its connection, and the files its agents sent. */
data class ActiveDaemon(val daemon: SavedDaemon, val connection: DaemonConnection, val api: DaemonApi, val files: SharedFileStore)

/**
 * Keeps one WebSocket to the selected daemon while the app is visible and
 * for a short grace period after, so a quick switch to another app does not
 * drop it. In the background, [io.malac.app.notify.BackgroundCheckWorker]
 * takes over.
 */
class ConnectionManager(
    private val context: Context,
    private val http: OkHttpClient,
    private val daemons: DaemonRepository,
    private val ledgers: LedgerStore,
    private val notifier: Notifier,
    private val scope: CoroutineScope,
) : DefaultLifecycleObserver {
    private val _active = MutableStateFlow<ActiveDaemon?>(null)
    val active: StateFlow<ActiveDaemon?> = _active.asStateFlow()

    private val _notices = MutableSharedFlow<String>(extraBufferCapacity = 8)

    /** Daemon notices and command errors, for a snackbar. */
    val notices: SharedFlow<String> = _notices.asSharedFlow()

    @Volatile
    var inForeground: Boolean = false
        private set

    private var effects: Job? = null
    private var pendingStop: Job? = null

    fun start() {
        ProcessLifecycleOwner.get().lifecycle.addObserver(this)
        context.getSystemService(ConnectivityManager::class.java).registerDefaultNetworkCallback(
            object : ConnectivityManager.NetworkCallback() {
                override fun onAvailable(network: Network) {
                    _active.value?.connection?.reconnectNow()
                }
            },
        )
        // On the main thread, like the lifecycle callbacks, so they never race.
        scope.launch(Dispatchers.Main.immediate) {
            daemons.selected.collect { daemon ->
                val current = _active.value
                if (current != null && daemon != null && current.daemon.id == daemon.id) {
                    _active.value = current.copy(daemon = daemon)
                } else {
                    switchTo(daemon)
                }
            }
        }
    }

    fun notice(message: String) {
        _notices.tryEmit(message)
    }

    override fun onStart(owner: LifecycleOwner) {
        inForeground = true
        pendingStop?.cancel()
        _active.value?.connection?.start()
    }

    override fun onStop(owner: LifecycleOwner) {
        inForeground = false
        val active = _active.value ?: return
        rememberSeen(active)
        pendingStop = scope.launch(Dispatchers.Main.immediate) {
            delay(GRACE_MILLIS)
            active.connection.stop()
        }
    }

    private fun switchTo(daemon: SavedDaemon?) {
        effects?.cancel()
        _active.value?.connection?.stop()
        val endpoint = daemon?.let(daemons::endpoint)
        if (daemon == null || endpoint == null) {
            _active.value = null
            return
        }
        val connection = DaemonConnection(endpoint, http, scope)
        val api = DaemonApi(http, endpoint)
        val active = ActiveDaemon(daemon, connection, api, SharedFileStore(context, api))
        _active.value = active
        effects = scope.launch { connection.effects.collect { handle(active, it) } }
        if (inForeground) connection.start()
    }

    private fun handle(active: ActiveDaemon, effect: Effect) {
        when (effect) {
            is Effect.Notice -> _notices.tryEmit(effect.message)
            is Effect.ApprovalRequested -> if (!inForeground) {
                notifier.approval(active.daemon, effect.approval, active.connection.state.value.place(effect.approval))
            }
            is Effect.ApprovalResolved -> notifier.cancelApproval(effect.requestId)
            is Effect.TurnFinished -> if (!inForeground) {
                notifier.turnFinished(active.daemon, effect.agent.id, effect.agent.displayName, effect.status, effect.text)
            }
            // Played and stopped by io.malac.app.voice.Speech, which follows the active daemon itself.
            is Effect.Say, is Effect.SayCancelled -> Unit
            is Effect.Resync -> Unit
        }
    }

    // What the user saw in the app is not notified again by the background check.
    private fun rememberSeen(active: ActiveDaemon) {
        val state = active.connection.state.value
        if (!state.loaded) return
        val running = state.agents.values.filter { it.isTopLevel && it.live && it.status == AgentStatus.Running }.map { it.id }
        ledgers.put(active.daemon.id, Ledger(requests = state.pending.keys, running = running.toSet()))
    }

    private companion object {
        const val GRACE_MILLIS = 60_000L
    }
}

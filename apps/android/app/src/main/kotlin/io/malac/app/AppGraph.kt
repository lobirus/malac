package io.malac.app

import android.content.Context
import io.malac.app.connection.ConnectionManager
import io.malac.app.data.DaemonRepository
import io.malac.app.data.LedgerStore
import io.malac.app.data.SavedDaemon
import io.malac.app.data.TokenVault
import io.malac.app.notify.NoPushRegistrar
import io.malac.app.notify.Notifier
import io.malac.app.notify.PushRegistrar
import io.malac.app.voice.Speech
import io.malac.app.voice.VoicePrefs
import io.malac.core.net.DaemonApi
import io.malac.core.net.DaemonConnection
import io.malac.core.net.HttpPairingService
import io.malac.core.pairing.PairingService
import io.malac.core.protocol.Command
import io.malac.core.protocol.Decision
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.first
import kotlinx.serialization.SerializationException
import okhttp3.OkHttpClient
import java.io.IOException
import java.util.concurrent.TimeUnit

/** The app's long-lived objects. */
class AppGraph(context: Context, val crashes: CrashLog) {
    val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    val http: OkHttpClient = OkHttpClient.Builder()
        .connectTimeout(10, TimeUnit.SECONDS)
        // Detects a dead connection after a network change sooner than TCP would.
        .pingInterval(25, TimeUnit.SECONDS)
        .build()

    val vault = TokenVault(context)
    val daemons = DaemonRepository(context, vault)
    val ledgers = LedgerStore(context)
    val notifier = Notifier(context)
    val pairing: PairingService = HttpPairingService(http)
    val push: PushRegistrar = NoPushRegistrar
    val connections = ConnectionManager(context, http, daemons, ledgers, notifier, scope)
    val voicePrefs = VoicePrefs(context)
    val speech = Speech(context, connections, voicePrefs, scope)

    /**
     * Forgets a daemon: revokes this device's token there when the daemon
     * answers, then deletes it here. An unreachable daemon keeps the device
     * listed until it is revoked on the computer.
     */
    suspend fun forget(daemon: SavedDaemon) {
        daemons.endpoint(daemon)?.let { endpoint ->
            try {
                DaemonApi(http, endpoint).revokeSelf()
            } catch (error: IOException) {
                // Offline or already revoked.
            } catch (error: SerializationException) {
                // Not a reply this app understands; the local token goes anyway.
            }
        }
        push.unregister(daemon)
        ledgers.remove(daemon.id)
        daemons.forget(daemon.id)
    }

    /** Answers a request from a notification: over the open WebSocket if there is one, else over REST. */
    suspend fun answer(daemonId: String, requestId: String, decision: Decision) {
        val command = Command.Approve(requestId, decision)
        val active = connections.active.value
        val sent = active?.daemon?.id == daemonId &&
            active.connection.status.value == DaemonConnection.Status.Online &&
            active.connection.send(command)
        if (!sent) {
            val daemon = daemons.daemons.first().firstOrNull { it.id == daemonId } ?: return
            val endpoint = daemons.endpoint(daemon) ?: return
            runCatching { DaemonApi(http, endpoint).command(command) }
        }
        notifier.cancelApproval(requestId)
    }
}

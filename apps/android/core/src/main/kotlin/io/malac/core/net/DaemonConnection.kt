package io.malac.core.net

import io.malac.core.protocol.Command
import io.malac.core.protocol.ServerMessage
import io.malac.core.protocol.decodeServerMessage
import io.malac.core.protocol.encode
import io.malac.core.state.DaemonState
import io.malac.core.state.Effect
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.TimeoutCancellationException
import kotlinx.coroutines.withTimeout
import kotlinx.coroutines.withTimeoutOrNull
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicLong

/** No reply because the connection closed or timed out, or an `error` reply when [refused]. */
class CommandException(message: String, val refused: Boolean = false) : Exception(message)

/**
 * One WebSocket to a daemon, reconnecting with backoff until [stop]. Every
 * message is applied to [state] in order; replies also complete the
 * [request] that sent the command.
 */
class DaemonConnection(
    private val endpoint: Endpoint,
    private val client: OkHttpClient,
    private val scope: CoroutineScope,
    private val minDelayMillis: Long = 500,
    private val maxDelayMillis: Long = 10_000,
    private val requestTimeoutMillis: Long = 30_000,
) {
    sealed interface Status {
        data object Connecting : Status

        data object Online : Status

        /** Disconnected; the next attempt is in [retryInMillis]. */
        data class Offline(val reason: String, val retryInMillis: Long) : Status

        /** The daemon does not know the token: it was revoked. Pair again. */
        data object Unauthorized : Status

        data object Stopped : Status
    }

    private val _state = MutableStateFlow(DaemonState())
    val state: StateFlow<DaemonState> = _state.asStateFlow()

    private val _status = MutableStateFlow<Status>(Status.Stopped)
    val status: StateFlow<Status> = _status.asStateFlow()

    private val _effects = MutableSharedFlow<Effect>(extraBufferCapacity = 64)

    /** Notices, approvals and finished turns, for the UI and notifications. */
    val effects: SharedFlow<Effect> = _effects.asSharedFlow()

    private val incoming = Channel<ServerMessage>(Channel.UNLIMITED)
    private val wake = Channel<Unit>(Channel.CONFLATED)
    private val waiting = ConcurrentHashMap<String, CompletableDeferred<ServerMessage>>()
    private val nextRef = AtomicLong(1)
    private val delayMillis = AtomicLong(minDelayMillis)

    @Volatile private var socket: WebSocket? = null
    private var jobs: List<Job> = emptyList()

    fun start() {
        if (jobs.isNotEmpty()) return
        jobs = listOf(scope.launch { consume() }, scope.launch { run() })
    }

    fun stop() {
        jobs.forEach { it.cancel() }
        jobs = emptyList()
        socket?.close(1000, null)
        socket = null
        failWaiting("Disconnected from malac")
        _status.value = Status.Stopped
    }

    /** Skips the backoff, e.g. when the network came back. */
    fun reconnectNow() {
        delayMillis.set(minDelayMillis)
        if (_status.value is Status.Offline) wake.trySend(Unit)
    }

    /** Sends [command] without waiting for its reply. */
    fun send(command: Command): Boolean = socket?.send(command.encode()) == true

    /** Sends [command] and returns its reply; an `error` reply throws [CommandException]. */
    suspend fun request(command: Command): ServerMessage {
        val ref = "a${nextRef.getAndIncrement()}"
        val reply = CompletableDeferred<ServerMessage>()
        waiting[ref] = reply
        try {
            if (socket?.send(command.encode(ref)) != true) throw CommandException("Not connected to malac")
            val message = try {
                withTimeout(requestTimeoutMillis) { reply.await() }
            } catch (timeout: TimeoutCancellationException) {
                // Not a cancellation of the caller: report it like any failed command.
                throw CommandException("malac did not answer in time")
            }
            if (message is ServerMessage.Error) throw CommandException(message.message, refused = true)
            return message
        } finally {
            waiting.remove(ref)
        }
    }

    /** Applies a local change, e.g. dropping a request the daemon said was already answered. */
    fun edit(change: (DaemonState) -> DaemonState) = _state.update(change)

    private suspend fun run() {
        while (true) {
            _status.value = Status.Connecting
            val closed = CompletableDeferred<Close>()
            // The token goes in a header, as for REST, so it stays out of URLs and logs.
            val request = Request.Builder()
                .url(endpoint.base.resolve("api/ws")!!)
                .header("Authorization", "Bearer ${endpoint.token}")
                .build()
            socket = client.newWebSocket(request, Listener(closed))
            val close = closed.await()
            socket = null
            failWaiting("Disconnected from malac")
            if (close.unauthorized) {
                _status.value = Status.Unauthorized
                return
            }
            val wait = delayMillis.get()
            delayMillis.set((wait * 2).coerceAtMost(maxDelayMillis))
            _status.value = Status.Offline(close.reason, wait)
            withTimeoutOrNull(wait) { wake.receive() }
        }
    }

    private suspend fun consume() {
        for (message in incoming) {
            var effects = emptyList<Effect>()
            _state.update { current -> current.reduce(message).also { effects = it.effects }.state }
            if (message is ServerMessage.Snapshot) {
                delayMillis.set(minDelayMillis)
                _status.value = Status.Online
            }
            message.clientRef?.let { waiting[it]?.complete(message) }
            for (effect in effects) {
                if (effect is Effect.Resync) send(Command.Resync(effect.workspace)) else _effects.emit(effect)
            }
        }
    }

    private fun failWaiting(reason: String) {
        waiting.values.forEach { it.completeExceptionally(CommandException(reason)) }
    }

    private class Close(val reason: String, val unauthorized: Boolean = false)

    private inner class Listener(private val closed: CompletableDeferred<Close>) : WebSocketListener() {
        override fun onMessage(webSocket: WebSocket, text: String) {
            incoming.trySend(decodeServerMessage(text))
        }

        override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
            webSocket.close(1000, null)
            closed.complete(Close(reason.ifEmpty { "Connection closed" }))
        }

        override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
            closed.complete(Close(reason.ifEmpty { "Connection closed" }))
        }

        // A 403 says why, e.g. that remote access is off; that can change, so it is retried.
        override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
            val refusal = response?.use { it.body.string().take(300).trim() }.orEmpty()
            closed.complete(Close(refusal.ifEmpty { t.message ?: "Cannot reach malac" }, unauthorized = response?.code == 401))
        }
    }
}

package io.malac.core.net

import io.malac.core.protocol.TalkCommand
import io.malac.core.protocol.TalkMessage
import io.malac.core.protocol.decodeTalkMessage
import io.malac.core.protocol.encode
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.receiveAsFlow
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import okio.ByteString
import okio.ByteString.Companion.toByteString
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Talk mode's WebSocket, `/api/voice/talk`, through [tab]'s active agent or
 * [agent]. Audio goes both ways as binary frames of 16-bit little-endian mono
 * PCM at 24 kHz; everything else is JSON. Unlike [DaemonConnection] it does
 * not reconnect: a dropped conversation ends.
 */
class TalkConnection(
    private val client: OkHttpClient,
    private val endpoint: Endpoint,
    private val tab: String? = null,
    private val agent: String? = null,
) {
    sealed interface Event {
        data class Message(val message: TalkMessage) : Event

        /** What the voice says, to play as it arrives. */
        class Audio(val pcm: ByteArray) : Event

        /** The socket closed; nothing follows. */
        data class Closed(val reason: String) : Event
    }

    private val channel = Channel<Event>(Channel.UNLIMITED)
    private val ended = AtomicBoolean(false)

    @Volatile private var socket: WebSocket? = null

    /** Everything the daemon sends, in order, ending with [Event.Closed]. For one collector. */
    val events: Flow<Event> = channel.receiveAsFlow()

    fun open() {
        if (socket != null || ended.get()) return
        val url = endpoint.base.resolve("api/voice/talk")!!.newBuilder()
            .apply {
                tab?.let { addQueryParameter("tab", it) }
                agent?.let { addQueryParameter("agent", it) }
            }
            .build()
        // The token goes in a header, as on the main WebSocket.
        val request = Request.Builder().url(url).header("Authorization", "Bearer ${endpoint.token}").build()
        socket = client.newWebSocket(request, Listener())
    }

    /** Sends a frame of microphone audio; false once the socket is gone. */
    fun sendAudio(pcm: ByteArray, offset: Int = 0, length: Int = pcm.size): Boolean =
        socket?.send(pcm.toByteString(offset, length)) == true

    fun send(command: TalkCommand): Boolean = socket?.send(command.encode()) == true

    /** Asks the daemon to end talk mode, then closes. */
    fun stop() {
        send(TalkCommand.Stop)
        socket?.close(1000, null)
    }

    private fun end(reason: String) {
        if (ended.compareAndSet(false, true)) {
            channel.trySend(Event.Closed(reason))
            channel.close()
        }
        socket = null
    }

    private inner class Listener : WebSocketListener() {
        override fun onMessage(webSocket: WebSocket, text: String) {
            val message = decodeTalkMessage(text)
            channel.trySend(Event.Message(message))
            if (message is TalkMessage.Closed) {
                webSocket.close(1000, null)
                end(message.reason)
            }
        }

        override fun onMessage(webSocket: WebSocket, bytes: ByteString) {
            channel.trySend(Event.Audio(bytes.toByteArray()))
        }

        override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
            webSocket.close(1000, null)
            end(reason.ifEmpty { "Talk mode ended" })
        }

        override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
            end(reason.ifEmpty { "Talk mode ended" })
        }

        // A refusal says why, e.g. that talk mode is off or has no model.
        override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
            val refusal = response?.use { it.body.string().take(300).trim() }.orEmpty()
            end(refusal.ifEmpty { unreachable(webSocket.request().url.host, t) })
        }
    }
}

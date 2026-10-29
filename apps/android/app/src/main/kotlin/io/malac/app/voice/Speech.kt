package io.malac.app.voice

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.media.MediaDataSource
import android.media.MediaPlayer
import android.os.Handler
import android.os.Looper
import io.malac.app.connection.ActiveDaemon
import io.malac.app.connection.ConnectionManager
import io.malac.core.net.CommandException
import io.malac.core.net.DaemonConnection
import io.malac.core.net.DaemonException
import io.malac.core.protocol.Command
import io.malac.core.protocol.ServerMessage
import io.malac.core.state.Effect
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.launch
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withContext
import java.io.IOException
import kotlin.coroutines.resume

/**
 * malac's speech on this phone. While the daemon's speech is ready and
 * "Speak on this device" is on, the connection says it plays speech
 * (`voice_listen`), again after every reconnect, and the sentences the
 * daemon picks this phone for are fetched and played one at a time. Once
 * the user answers an approval or sends a prompt they are stale: the daemon
 * cancels them (`say_cancelled`) and they stop at once.
 */
class Speech(
    context: Context,
    private val connections: ConnectionManager,
    private val prefs: VoicePrefs,
    private val scope: CoroutineScope,
) {
    private val audio = context.getSystemService(AudioManager::class.java)
    private val main = Handler(Looper.getMainLooper())
    private val wake = Channel<Unit>(Channel.CONFLATED)

    // Guarded by itself: the sentences waiting, and the one being fetched or played.
    private val waiting = ArrayDeque<Pair<ActiveDaemon, ServerMessage.Say>>()
    private var playing: Pair<ServerMessage.Say, Job>? = null

    private val _lines = MutableSharedFlow<String>(extraBufferCapacity = 8)

    /** Each sentence as it starts playing, to show briefly. */
    val lines: SharedFlow<String> = _lines.asSharedFlow()

    /** Talk mode holds speech while it runs, so two voices never talk at once. */
    @Volatile var held: Boolean = false
        set(value) {
            field = value
            if (value) skip()
        }

    private val focus = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN_TRANSIENT_MAY_DUCK)
        .setAudioAttributes(ATTRIBUTES)
        .setOnAudioFocusChangeListener { change ->
            if (change == AudioManager.AUDIOFOCUS_LOSS || change == AudioManager.AUDIOFOCUS_LOSS_TRANSIENT) skip()
        }
        .build()

    fun start() {
        scope.launch { connections.active.collectLatest { active -> if (active != null) follow(active) } }
        scope.launch { play() }
        scope.launch { prefs.speakHere.collect { on -> if (!on) skip() } }
    }

    private data class Listen(val online: Boolean, val wanted: Boolean, val listening: Boolean)

    private suspend fun follow(active: ActiveDaemon) = coroutineScope {
        val connection = active.connection
        launch {
            connection.effects.collect { effect ->
                when (effect) {
                    is Effect.Say -> {
                        synchronized(waiting) { waiting.addLast(active to effect.say) }
                        wake.trySend(Unit)
                    }
                    is Effect.SayCancelled -> stop { it.id in effect.ids }
                    else -> Unit
                }
            }
        }
        // A new connection has no listener, so this asks again after every reconnect.
        combine(connection.status, connection.state, prefs.speakHere) { status, state, here ->
            Listen(status == DaemonConnection.Status.Online, here && state.voice?.speech?.ready == true, state.listener != null)
        }
            .distinctUntilChanged()
            .collect { (online, wanted, listening) ->
                if (online && wanted != listening) {
                    try {
                        connection.request(Command.VoiceListen(wanted))
                    } catch (error: CommandException) {
                        // Asked again on the next change or reconnect.
                    }
                }
            }
    }

    /** The user answered an approval here: what reads approvals aloud is stale, without waiting for the daemon to say so. */
    fun approvalAnswered() = stop { it.reason == "approval" }

    private suspend fun play() {
        while (true) {
            wake.receive()
            var focused = false
            try {
                while (true) {
                    // Fetching and playing are one job, so stopping a sentence also drops its fetch.
                    val job = synchronized(waiting) {
                        val (active, say) = waiting.removeFirstOrNull() ?: return@synchronized null
                        scope.launch {
                            val clip = fetch(active, say) ?: return@launch
                            withContext(Dispatchers.Main) {
                                if (!focused) focused = audio.requestAudioFocus(focus) == AudioManager.AUDIOFOCUS_REQUEST_GRANTED
                                _lines.tryEmit(say.text)
                                playClip(clip)
                            }
                        }.also { playing = say to it }
                    } ?: break
                    job.join()
                    synchronized(waiting) { playing = null }
                }
            } finally {
                if (focused) audio.abandonAudioFocusRequest(focus)
            }
        }
    }

    // A clip is kept ten minutes; one that cannot be fetched stays unheard.
    private suspend fun fetch(active: ActiveDaemon, say: ServerMessage.Say): ByteArray? {
        if (held || !prefs.speakHere.value) return null
        val clip = try {
            active.api.sayAudio(say.id).bytes
        } catch (error: DaemonException) {
            return null
        }
        return clip.takeUnless { held || !prefs.speakHere.value }
    }

    private suspend fun playClip(bytes: ByteArray) = suspendCancellableCoroutine<Unit> { continuation ->
        val player = MediaPlayer()
        fun finish() {
            player.release()
            if (continuation.isActive) continuation.resume(Unit)
        }
        continuation.invokeOnCancellation { main.post { player.release() } }
        try {
            player.setAudioAttributes(ATTRIBUTES)
            player.setDataSource(BytesSource(bytes))
            player.setOnPreparedListener { it.start() }
            player.setOnCompletionListener { finish() }
            player.setOnErrorListener { _, _, _ ->
                finish()
                true
            }
            player.prepareAsync()
        } catch (error: IOException) {
            finish()
        } catch (error: IllegalStateException) {
            finish()
        } catch (error: IllegalArgumentException) {
            finish()
        }
    }

    private fun skip() = stop { true }

    /** Drops the waiting sentences [which] picks, and stops the current one at once if it is one. */
    private fun stop(which: (ServerMessage.Say) -> Boolean) {
        val current = synchronized(waiting) {
            waiting.removeAll { (_, say) -> which(say) }
            playing?.takeIf { (say, _) -> which(say) }?.second
        }
        current?.cancel()
    }

    /** A clip in memory, as MediaPlayer reads it. */
    private class BytesSource(private val bytes: ByteArray) : MediaDataSource() {
        override fun readAt(position: Long, buffer: ByteArray, offset: Int, size: Int): Int {
            if (position >= bytes.size) return -1
            val count = minOf(size.toLong(), bytes.size - position).toInt()
            System.arraycopy(bytes, position.toInt(), buffer, offset, count)
            return count
        }

        override fun getSize(): Long = bytes.size.toLong()

        override fun close() = Unit
    }

    private companion object {
        val ATTRIBUTES: AudioAttributes = AudioAttributes.Builder()
            .setUsage(AudioAttributes.USAGE_ASSISTANT)
            .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
            .build()
    }
}

package io.malac.app.ui.talk

import android.content.Context
import android.media.MediaRecorder
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import io.malac.app.R
import io.malac.app.voice.Microphone
import io.malac.app.voice.Speech
import io.malac.app.voice.TalkAudio
import io.malac.core.net.Endpoint
import io.malac.core.net.TalkConnection
import io.malac.core.protocol.TalkMessage
import io.malac.core.voice.PcmFrames
import io.malac.core.voice.TALK_RATE
import io.malac.core.voice.TalkState
import io.malac.core.voice.level
import io.malac.core.voice.pcmBytes
import kotlinx.coroutines.launch
import okhttp3.OkHttpClient

/**
 * One spoken conversation through [tab]'s active agent or [agent]: the
 * microphone streams to the daemon once it is ready, the voice plays as it
 * arrives, and malac's own speech waits until the conversation ends.
 */
class TalkViewModel(
    context: Context,
    http: OkHttpClient,
    endpoint: Endpoint,
    tab: String?,
    agent: String?,
    private val speech: Speech,
) : ViewModel() {
    private val app = context.applicationContext
    private val connection = TalkConnection(http, endpoint, tab, agent)
    private val audio = TalkAudio(app)
    private val frames = PcmFrames()
    private val microphone = Microphone(app, TALK_RATE, MediaRecorder.AudioSource.VOICE_COMMUNICATION, ::heard)
    private val main = Handler(Looper.getMainLooper())
    private var ended = false

    var state by mutableStateOf(TalkState())
        private set

    /** The microphone's level, 0 while muted. */
    var level by mutableFloatStateOf(0f)
        private set

    var muted by mutableStateOf(false)
        private set

    @Volatile private var sending = true
    @Volatile private var shown = 0L

    /** Whether the voice is audible. */
    val playing = audio.playing

    init {
        speech.held = true
        audio.start()
        connection.open()
        viewModelScope.launch { connection.events.collect(::handle) }
    }

    fun toggleMute() {
        muted = !muted
        sending = !muted
        if (muted) level = 0f
    }

    /** Ends the conversation; nothing plays or records after. */
    fun end() {
        if (ended) return
        ended = true
        connection.stop()
        release()
    }

    override fun onCleared() {
        end()
    }

    private fun handle(event: TalkConnection.Event) {
        when (event) {
            is TalkConnection.Event.Message -> {
                val message = event.message
                state = state.reduce(message)
                when (message) {
                    is TalkMessage.Ready -> if (!microphone.start(cleanUp = true)) {
                        state = state.failed(app.getString(R.string.microphone_unavailable))
                    }
                    TalkMessage.Listening -> audio.flush()
                    else -> Unit
                }
            }
            is TalkConnection.Event.Audio -> if (!ended) {
                audio.play(event.pcm)
                state = state.speaking()
            }
            is TalkConnection.Event.Closed -> {
                state = state.ended(event.reason)
                ended = true
                release()
            }
        }
    }

    private fun release() {
        microphone.stop()
        audio.stop()
        speech.held = false
        level = 0f
    }

    // On the microphone's thread: ~100 ms frames to the daemon, and the level for the meter.
    private fun heard(samples: ShortArray, count: Int) {
        if (!sending) {
            frames.clear()
            return
        }
        frames.push(pcmBytes(samples, count)) { connection.sendAudio(it) }
        val now = SystemClock.uptimeMillis()
        if (now - shown >= 100) {
            shown = now
            val value = level(samples, count)
            main.post { if (sending && !ended) level = value }
        }
    }
}

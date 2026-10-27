package io.malac.app.voice

import android.content.Context
import android.media.MediaRecorder
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import io.malac.app.R
import io.malac.core.net.DaemonApi
import io.malac.core.net.DaemonException
import io.malac.core.voice.DICTATION_RATE
import io.malac.core.voice.level
import io.malac.core.voice.pcmBytes
import io.malac.core.voice.pcmMillis
import io.malac.core.voice.wav
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.ByteArrayOutputStream

/**
 * Dictation for a composer: records 16 kHz mono until stopped, sends the
 * WAV to the daemon's dictation model and hands back the text.
 */
class Dictation(private val context: Context, private val api: DaemonApi, private val scope: CoroutineScope) {
    sealed interface State {
        data object Idle : State

        data class Recording(val millis: Long = 0, val level: Float = 0f) : State

        data object Transcribing : State
    }

    var state by mutableStateOf<State>(State.Idle)
        private set

    private val main = Handler(Looper.getMainLooper())
    private val pcm = ByteArrayOutputStream()
    private var microphone: Microphone? = null
    private var onText: (String) -> Unit = {}
    private var onError: (String) -> Unit = {}

    @Volatile private var shown = 0L

    /** Starts recording; [onText] gets what was said once [finish] is called, [onError] what went wrong. */
    fun start(onText: (String) -> Unit, onError: (String) -> Unit) {
        if (state != State.Idle) return
        this.onText = onText
        this.onError = onError
        synchronized(pcm) { pcm.reset() }
        val microphone = Microphone(context, DICTATION_RATE, MediaRecorder.AudioSource.VOICE_RECOGNITION, ::heard)
        if (!microphone.start()) {
            onError(context.getString(R.string.microphone_unavailable))
            return
        }
        this.microphone = microphone
        state = State.Recording()
    }

    /** Stops recording and transcribes what was said. */
    fun finish() {
        if (state !is State.Recording) return
        stopMicrophone()
        val audio = synchronized(pcm) { pcm.toByteArray().also { pcm.reset() } }
        // A tap and a tap again with nothing said in between.
        if (pcmMillis(audio.size.toLong(), DICTATION_RATE) < MIN_MILLIS) {
            state = State.Idle
            return
        }
        state = State.Transcribing
        scope.launch {
            try {
                val file = withContext(Dispatchers.Default) { wav(audio, DICTATION_RATE) }
                val text = api.transcribe(file).text.trim()
                if (text.isEmpty()) onError(context.getString(R.string.dictation_nothing_heard)) else onText(text)
            } catch (error: DaemonException) {
                onError(context.getString(R.string.dictation_failed, error.message.orEmpty()))
            } finally {
                state = State.Idle
            }
        }
    }

    fun cancel() {
        stopMicrophone()
        synchronized(pcm) { pcm.reset() }
        if (state is State.Recording) state = State.Idle
    }

    private fun stopMicrophone() {
        microphone?.stop()
        microphone = null
    }

    // On the microphone's thread.
    private fun heard(samples: ShortArray, count: Int) {
        val bytes = pcmBytes(samples, count)
        val size = synchronized(pcm) {
            pcm.write(bytes)
            pcm.size()
        }
        val now = SystemClock.uptimeMillis()
        if (now - shown >= 100) {
            shown = now
            val update = State.Recording(pcmMillis(size.toLong(), DICTATION_RATE), level(samples, count))
            main.post { if (state is State.Recording) state = update }
        }
        if (size >= MAX_BYTES) main.post { finish() }
    }

    private companion object {
        const val MIN_MILLIS = 300L

        /** Ten minutes, well inside the daemon's 25 MB. */
        const val MAX_BYTES = DICTATION_RATE * 2 * 600
    }
}

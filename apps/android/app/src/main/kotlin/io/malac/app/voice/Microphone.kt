package io.malac.app.voice

import android.Manifest
import android.annotation.SuppressLint
import android.content.Context
import android.content.pm.PackageManager
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.audiofx.AcousticEchoCanceler
import android.media.audiofx.AudioEffect
import android.media.audiofx.NoiseSuppressor
import androidx.core.content.ContextCompat

/** Whether the app may record audio. */
fun Context.canRecord(): Boolean =
    ContextCompat.checkSelfPermission(this, Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED

/**
 * 16-bit mono PCM from the microphone at [rate], read on a thread of its own
 * in 20 ms chunks and handed to [onPcm] there until [stop]. [source] is a
 * `MediaRecorder.AudioSource`: voice recognition for dictation, voice
 * communication for talk mode, where Android cancels the speaker's echo.
 */
class Microphone(
    private val context: Context,
    private val rate: Int,
    private val source: Int,
    private val onPcm: (samples: ShortArray, count: Int) -> Unit,
) {
    private var record: AudioRecord? = null
    private var thread: Thread? = null
    private val effects = mutableListOf<AudioEffect>()

    @Volatile private var running = false

    /** False when it cannot record: no permission, or another app holds the microphone. */
    @SuppressLint("MissingPermission") // canRecord() checks it first
    fun start(cleanUp: Boolean = false): Boolean {
        if (running) return true
        if (!context.canRecord()) return false
        val minimum = AudioRecord.getMinBufferSize(rate, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
        if (minimum <= 0) return false
        val record = try {
            // Half a second of room, so a slow reader loses nothing.
            AudioRecord(source, rate, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT, maxOf(minimum, rate))
        } catch (error: SecurityException) {
            return false
        } catch (error: IllegalArgumentException) {
            return false
        }
        if (record.state != AudioRecord.STATE_INITIALIZED) {
            record.release()
            return false
        }
        if (cleanUp) attachEffects(record.audioSessionId)
        try {
            record.startRecording()
        } catch (error: IllegalStateException) {
            release(record)
            return false
        }
        if (record.recordingState != AudioRecord.RECORDSTATE_RECORDING) {
            release(record)
            return false
        }
        this.record = record
        running = true
        thread = Thread({ read(record) }, "malac-microphone").apply { start() }
        return true
    }

    fun stop() {
        if (!running) return
        running = false
        val record = record ?: return
        runCatching { record.stop() }
        thread?.join(1_000)
        thread = null
        release(record)
        this.record = null
    }

    private fun read(record: AudioRecord) {
        val buffer = ShortArray(rate / 50)
        while (running) {
            val count = record.read(buffer, 0, buffer.size)
            if (count < 0) break
            if (count > 0 && running) onPcm(buffer, count)
        }
    }

    // The platform's echo canceller and noise suppressor, where the phone has them.
    private fun attachEffects(session: Int) {
        if (AcousticEchoCanceler.isAvailable()) AcousticEchoCanceler.create(session)?.let { effects += it }
        if (NoiseSuppressor.isAvailable()) NoiseSuppressor.create(session)?.let { effects += it }
        for (effect in effects) runCatching { effect.setEnabled(true) }
    }

    private fun release(record: AudioRecord) {
        effects.forEach { runCatching { it.release() } }
        effects.clear()
        record.release()
    }
}

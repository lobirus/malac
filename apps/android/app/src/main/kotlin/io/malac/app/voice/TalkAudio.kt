package io.malac.app.voice

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioDeviceInfo
import android.media.AudioFocusRequest
import android.media.AudioFormat
import android.media.AudioManager
import android.media.AudioTrack
import android.os.Build
import io.malac.core.voice.TALK_FRAME_BYTES
import io.malac.core.voice.TALK_RATE
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicLong

/**
 * Talk mode's sound: the phone in communication mode, as for a call, so
 * its echo canceller hears what it plays, on the speaker unless a headset is
 * connected; and the voice's 24 kHz PCM streamed to an [AudioTrack] as it
 * arrives, cut off at once by [flush] when the user starts speaking.
 */
class TalkAudio(context: Context) {
    private val audio = context.getSystemService(AudioManager::class.java)
    private val queue = LinkedBlockingQueue<Chunk>()
    private val generation = AtomicInteger()
    private val written = AtomicLong()
    private val _playing = MutableStateFlow(false)

    /** Whether the voice is audible right now. */
    val playing: StateFlow<Boolean> = _playing.asStateFlow()

    private var track: AudioTrack? = null
    private var writer: Thread? = null
    private var previousMode = AudioManager.MODE_NORMAL

    @Volatile private var running = false

    private class Chunk(val pcm: ByteArray, val generation: Int)

    private val focus = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN_TRANSIENT)
        .setAudioAttributes(ATTRIBUTES)
        .build()

    fun start() {
        if (running) return
        audio.requestAudioFocus(focus)
        previousMode = audio.mode
        audio.mode = AudioManager.MODE_IN_COMMUNICATION
        route()
        val format = AudioFormat.Builder()
            .setSampleRate(TALK_RATE)
            .setChannelMask(AudioFormat.CHANNEL_OUT_MONO)
            .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
            .build()
        val minimum = AudioTrack.getMinBufferSize(TALK_RATE, AudioFormat.CHANNEL_OUT_MONO, AudioFormat.ENCODING_PCM_16BIT)
        val track = AudioTrack.Builder()
            .setAudioAttributes(ATTRIBUTES)
            .setAudioFormat(format)
            .setBufferSizeInBytes(maxOf(minimum, TALK_FRAME_BYTES * 2))
            .setTransferMode(AudioTrack.MODE_STREAM)
            .build()
        track.play()
        this.track = track
        running = true
        writer = Thread({ write(track) }, "malac-talk-playback").apply { start() }
    }

    /** Queues PCM to play after what is already queued. */
    fun play(pcm: ByteArray) {
        if (running) queue.put(Chunk(pcm, generation.get()))
    }

    /** Stops playing at once and drops what is queued: the user started speaking. */
    fun flush() {
        generation.incrementAndGet()
        queue.clear()
        val track = track ?: return
        runCatching {
            track.pause()
            track.flush()
            track.play()
        }
        written.set(0)
        _playing.value = false
    }

    fun stop() {
        if (!running) return
        running = false
        queue.clear()
        writer?.join(1_000)
        writer = null
        track?.let { runCatching { it.stop() }; it.release() }
        track = null
        _playing.value = false
        unroute()
        audio.mode = previousMode
        audio.abandonAudioFocusRequest(focus)
    }

    private fun write(track: AudioTrack) {
        while (running) {
            val chunk = queue.poll(20, TimeUnit.MILLISECONDS)
            if (chunk == null) {
                // Played out: the head has passed everything written.
                if (_playing.value && track.playbackHeadPosition.toLong() >= written.get()) _playing.value = false
                continue
            }
            if (chunk.generation != generation.get()) continue
            _playing.value = true
            var offset = 0
            // In slices, so a flush cuts in within 20 ms.
            while (offset < chunk.pcm.size && running && chunk.generation == generation.get()) {
                val count = track.write(chunk.pcm, offset, minOf(SLICE_BYTES, chunk.pcm.size - offset))
                if (count <= 0) break
                offset += count
                written.addAndGet(count / 2L)
            }
        }
    }

    private fun route() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            val devices = audio.availableCommunicationDevices
            val device = devices.firstOrNull { it.type in HEADSETS } ?: devices.firstOrNull { it.type == AudioDeviceInfo.TYPE_BUILTIN_SPEAKER }
            device?.let { audio.setCommunicationDevice(it) }
        } else {
            val headset = audio.getDevices(AudioManager.GET_DEVICES_OUTPUTS).any { it.type in HEADSETS }
            @Suppress("DEPRECATION")
            audio.isSpeakerphoneOn = !headset
        }
    }

    private fun unroute() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            audio.clearCommunicationDevice()
        } else {
            @Suppress("DEPRECATION")
            audio.isSpeakerphoneOn = false
        }
    }

    private companion object {
        /** 20 ms of 24 kHz mono 16-bit audio. */
        const val SLICE_BYTES = TALK_RATE / 50 * 2

        val ATTRIBUTES: AudioAttributes = AudioAttributes.Builder()
            .setUsage(AudioAttributes.USAGE_VOICE_COMMUNICATION)
            .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
            .build()

        val HEADSETS = setOf(
            AudioDeviceInfo.TYPE_WIRED_HEADSET,
            AudioDeviceInfo.TYPE_WIRED_HEADPHONES,
            AudioDeviceInfo.TYPE_BLUETOOTH_SCO,
            AudioDeviceInfo.TYPE_USB_HEADSET,
            AudioDeviceInfo.TYPE_BLE_HEADSET,
        )
    }
}

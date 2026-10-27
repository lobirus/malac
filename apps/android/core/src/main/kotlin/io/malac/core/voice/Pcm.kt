package io.malac.core.voice

import java.io.ByteArrayOutputStream
import kotlin.math.log10
import kotlin.math.sqrt

/** Dictation records at this rate: every transcription model takes 16 kHz mono 16-bit WAV. */
const val DICTATION_RATE = 16_000

/** Talk mode's audio, both ways. */
const val TALK_RATE = 24_000

/** About 100 ms of talk audio, the size of each frame sent: 2400 samples of 2 bytes. */
const val TALK_FRAME_BYTES = TALK_RATE / 10 * 2

/** A WAV file of 16-bit little-endian PCM [pcm]. */
fun wav(pcm: ByteArray, sampleRate: Int, channels: Int = 1): ByteArray {
    val bits = 16
    val blockAlign = channels * bits / 8
    val out = ByteArrayOutputStream(44 + pcm.size)
    fun int(value: Int) = repeat(4) { out.write(value ushr (8 * it) and 0xff) }
    fun short(value: Int) = repeat(2) { out.write(value ushr (8 * it) and 0xff) }
    out.write("RIFF".toByteArray())
    int(36 + pcm.size)
    out.write("WAVE".toByteArray())
    out.write("fmt ".toByteArray())
    int(16)
    short(1)
    short(channels)
    int(sampleRate)
    int(sampleRate * blockAlign)
    short(blockAlign)
    short(bits)
    out.write("data".toByteArray())
    int(pcm.size)
    out.write(pcm)
    return out.toByteArray()
}

/** [count] samples as 16-bit little-endian bytes. */
fun pcmBytes(samples: ShortArray, count: Int = samples.size): ByteArray {
    val bytes = ByteArray(count * 2)
    for (index in 0 until count) {
        val sample = samples[index].toInt()
        bytes[index * 2] = sample.toByte()
        bytes[index * 2 + 1] = (sample shr 8).toByte()
    }
    return bytes
}

/** How loud [count] samples are, from 0 (silence, or -60 dBFS) to 1 (full scale), for a level meter. */
fun level(samples: ShortArray, count: Int = samples.size): Float {
    if (count <= 0) return 0f
    var sum = 0.0
    for (index in 0 until count) {
        val sample = samples[index] / 32768.0
        sum += sample * sample
    }
    val rms = sqrt(sum / count)
    if (rms <= 0.0) return 0f
    return ((20 * log10(rms) + 60) / 60).toFloat().coerceIn(0f, 1f)
}

/** Milliseconds of 16-bit mono audio at [sampleRate] in [bytes]. */
fun pcmMillis(bytes: Long, sampleRate: Int): Long = bytes * 1000 / (sampleRate * 2L)

/** Cuts a stream of PCM into frames of [frameBytes], whatever sizes it arrives in. */
class PcmFrames(private val frameBytes: Int = TALK_FRAME_BYTES) {
    private val pending = ByteArray(frameBytes)
    private var filled = 0

    /** Adds [length] bytes from [offset] and calls [emit] with each frame they complete. */
    fun push(bytes: ByteArray, offset: Int = 0, length: Int = bytes.size - offset, emit: (ByteArray) -> Unit) {
        var from = offset
        val end = offset + length
        while (from < end) {
            val take = minOf(frameBytes - filled, end - from)
            System.arraycopy(bytes, from, pending, filled, take)
            filled += take
            from += take
            if (filled == frameBytes) {
                emit(pending.copyOf())
                filled = 0
            }
        }
    }

    /** Drops a partial frame, e.g. after muting. */
    fun clear() {
        filled = 0
    }
}

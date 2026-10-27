package io.malac.core.voice

import io.malac.core.protocol.Role
import io.malac.core.protocol.TalkMessage
import java.nio.ByteBuffer
import java.nio.ByteOrder
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

class VoiceTest {
    @Test
    fun wavHasACanonicalHeader() {
        val pcm = pcmBytes(shortArrayOf(0, 1, -1, Short.MAX_VALUE, Short.MIN_VALUE))
        val file = wav(pcm, DICTATION_RATE)
        val header = ByteBuffer.wrap(file).order(ByteOrder.LITTLE_ENDIAN)
        assertEquals("RIFF", String(file, 0, 4))
        assertEquals(36 + pcm.size, header.getInt(4))
        assertEquals("WAVEfmt ", String(file, 8, 8))
        assertEquals(16, header.getInt(16))
        assertEquals(1, header.getShort(20).toInt(), "PCM")
        assertEquals(1, header.getShort(22).toInt(), "mono")
        assertEquals(16_000, header.getInt(24))
        assertEquals(32_000, header.getInt(28), "bytes per second")
        assertEquals(2, header.getShort(32).toInt())
        assertEquals(16, header.getShort(34).toInt())
        assertEquals("data", String(file, 36, 4))
        assertEquals(pcm.size, header.getInt(40))
        assertContentEquals(pcm, file.copyOfRange(44, file.size))
    }

    @Test
    fun samplesAreLittleEndian() {
        assertContentEquals(byteArrayOf(0x34, 0x12, 0xff.toByte(), 0xff.toByte(), 0, 0x80.toByte()), pcmBytes(shortArrayOf(0x1234, -1, Short.MIN_VALUE)))
        assertContentEquals(byteArrayOf(0x34, 0x12), pcmBytes(shortArrayOf(0x1234, 7), count = 1))
    }

    @Test
    fun levelGoesFromSilenceToFullScale() {
        assertEquals(0f, level(ShortArray(160)))
        assertEquals(0f, level(ShortArray(0)))
        assertEquals(1f, level(ShortArray(160) { Short.MAX_VALUE }), 0.001f)
        // A tenth of full scale is -20 dBFS: two thirds up a 60 dB meter.
        assertEquals(2 / 3f, level(ShortArray(160) { 3277 }), 0.01f)
        assertEquals(0f, level(ShortArray(160) { 1 }), "below -60 dBFS")
    }

    @Test
    fun talkAudioIsCutIntoHundredMillisecondFrames() {
        assertEquals(4800, TALK_FRAME_BYTES)
        assertEquals(100, pcmMillis(TALK_FRAME_BYTES.toLong(), TALK_RATE))
        assertEquals(1500, pcmMillis(48_000, DICTATION_RATE))
        val frames = mutableListOf<ByteArray>()
        val framer = PcmFrames(4)
        framer.push(byteArrayOf(1, 2, 3)) { frames += it }
        assertTrue(frames.isEmpty())
        framer.push(byteArrayOf(4, 5, 6, 7, 8, 9, 10)) { frames += it }
        assertEquals(listOf(listOf<Byte>(1, 2, 3, 4), listOf<Byte>(5, 6, 7, 8)), frames.map { it.toList() })
        framer.clear()
        framer.push(byteArrayOf(0, 11, 12, 13, 14, 0), offset = 1, length = 4) { frames += it }
        assertEquals(listOf<Byte>(11, 12, 13, 14), frames.last().toList())
    }

    @Test
    fun dictationGoesInAtTheCursor() {
        assertEquals(Inserted("hello world", 11), insertDictation("", 0, 0, " hello world "))
        assertEquals(Inserted("fix the bug please", 11), insertDictation("fix please", 3, 3, "the bug"))
        assertEquals(Inserted("fix the bug.", 11), insertDictation("fix.", 3, 3, "the bug"))
        assertEquals(Inserted("say hi", 6), insertDictation("say hello", 4, 9, "hi"))
        assertEquals(Inserted("say hi", 6), insertDictation("say hello", 9, 4, "hi"), "a backwards selection")
        assertEquals(Inserted("line\nnext", 9), insertDictation("line\n", 5, 5, "next"))
        assertEquals(Inserted("same", 2), insertDictation("same", 2, 2, "  "), "nothing heard")
        assertEquals(Inserted("a b", 3), insertDictation("a", 40, 40, "b"), "a stale cursor")
    }

    @Test
    fun talkStateFollowsTheConversation() {
        var state = TalkState()
        assertEquals(TalkState.Phase.Connecting, state.phase)
        assertTrue(!state.streaming)
        state = state.reduce(TalkMessage.Ready("fake/gpt-realtime", "marin"))
        assertEquals(TalkState.Phase.Listening, state.phase)
        assertEquals("marin", state.voice)
        assertTrue(state.streaming)
        state = state.reduce(TalkMessage.Thinking)
        assertEquals(TalkState.Phase.Thinking, state.phase)
        state = state.reduce(TalkMessage.Tool("ask_malac", "Asked malac: say hello"))
        state = state.reduce(TalkMessage.Transcript(Role.Assistant, "Hel", final = false))
        state = state.reduce(TalkMessage.Transcript(Role.Assistant, "lo", final = false))
        assertEquals(TalkState.Phase.Speaking, state.phase)
        // The user's words were transcribed after the voice began answering.
        state = state.reduce(TalkMessage.Transcript(Role.User, "say hello ", final = true))
        state = state.reduce(TalkMessage.Transcript(Role.Assistant, "Hello.", final = true))
        state = state.reduce(TalkMessage.Done)
        assertEquals(TalkState.Phase.Listening, state.phase)
        assertEquals(
            listOf("Asked malac: say hello", "user: say hello", "assistant: Hello."),
            state.lines.map { line ->
                when (line) {
                    is TalkLine.Said -> "${line.role.name.lowercase()}: ${line.text}"
                    is TalkLine.Tool -> line.summary
                }
            },
        )
        assertEquals(3, state.lines.map { it.key }.toSet().size, "keys are unique")
        assertTrue((state.lines.last() as TalkLine.Said).final)

        state = state.reduce(TalkMessage.Transcript(Role.Assistant, "Again", final = false))
        assertEquals("Again", (state.lines.last() as TalkLine.Said).text, "a new answer starts a new line")
        state = state.reduce(TalkMessage.Listening)
        assertEquals(TalkState.Phase.Listening, state.phase)

        state = state.reduce(TalkMessage.Error("the provider hung up"))
        assertEquals("the provider hung up", state.error)
        state = state.reduce(TalkMessage.Closed("talk mode ended"))
        assertEquals(TalkState.Phase.Ended, state.phase)
        assertEquals("talk mode ended", state.ended)
        assertEquals("talk mode ended", state.ended("socket closed").ended, "the first reason stays")
        assertEquals(TalkState.Phase.Ended, state.speaking().phase)
    }

    @Test
    fun talkStateKeepsTheLatestLines() {
        var state = TalkState().reduce(TalkMessage.Ready())
        repeat(TalkState.MAX_LINES + 5) { state = state.reduce(TalkMessage.Tool("malac_status", "Checked on malac $it")) }
        assertEquals(TalkState.MAX_LINES, state.lines.size)
        assertEquals("Checked on malac ${TalkState.MAX_LINES + 4}", (state.lines.last() as TalkLine.Tool).summary)
        assertNull(state.error)
    }
}

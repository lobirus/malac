package io.malac.core.net

import io.malac.core.protocol.VoiceFeature
import io.malac.core.voice.DICTATION_RATE
import io.malac.core.voice.wav
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonPrimitive
import mockwebserver3.MockResponse
import mockwebserver3.MockWebServer
import okhttp3.OkHttpClient
import okio.Buffer
import java.nio.file.Files
import kotlin.test.AfterTest
import kotlin.test.BeforeTest
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

/** The REST calls of dictation, speech, voice settings and shared files. */
class VoiceApiTest {
    private val server = MockWebServer()
    private val api by lazy { DaemonApi(OkHttpClient(), Endpoint(server.url("/"), "token")) }

    private val status = """{"dictation":{"enabled":true,"model":"fake/whisper-1","models":["fake/whisper-1"]},""" +
        """"speech":{"enabled":true,"model":"system","configured":"system","models":["fake/tts-1","system"]},""" +
        """"talk":{"enabled":false,"model":"fake/gpt-realtime","models":["fake/gpt-realtime"]},"voices":["alloy"]}"""

    @BeforeTest
    fun start() = server.start()

    @AfterTest
    fun stop() = server.close()

    @Test
    fun settingsAreReadAndChanged() = runBlocking {
        server.enqueue(MockResponse.Builder().body(status).build())
        server.enqueue(MockResponse.Builder().body(status).build())
        val voice = api.voice(refresh = true)
        assertEquals(VoiceFeature(enabled = true, model = "fake/whisper-1", models = listOf("fake/whisper-1")), voice.dictation)
        val read = server.takeRequest()
        assertEquals("GET", read.method)
        assertEquals("/api/voice?refresh=true", read.target)
        assertEquals("Bearer token", read.headers["Authorization"])

        api.setVoice(mapOf("speech" to JsonPrimitive(false), "speech_model" to JsonNull))
        val change = server.takeRequest()
        assertEquals("PUT", change.method)
        assertEquals("/api/voice", change.url.encodedPath)
        assertEquals("""{"speech":false,"speech_model":null}""", change.body?.utf8())
    }

    @Test
    fun dictationPostsTheWav() = runBlocking {
        server.enqueue(MockResponse.Builder().body("""{"text":"hello there","model":"fake/whisper-1"}""").build())
        val recording = wav(ByteArray(3200), DICTATION_RATE)
        val heard = api.transcribe(recording, language = "en")
        assertEquals("hello there", heard.text)
        val request = server.takeRequest()
        assertEquals("POST", request.method)
        assertEquals("/api/voice/transcribe?language=en", request.target)
        assertEquals("audio/wav", request.headers["Content-Type"])
        assertContentEquals(recording, request.body?.toByteArray())
    }

    @Test
    fun aRefusalSaysWhy() = runBlocking {
        server.enqueue(MockResponse.Builder().code(500).body("""{"error":"dictation is off or no model can transcribe"}""").build())
        val error = assertFailsWith<DaemonException> { api.transcribe(ByteArray(44)) }
        assertEquals("dictation is off or no model can transcribe", error.message)
        assertEquals("/api/voice/transcribe", server.takeRequest().target, "no language: the daemon's setting")
    }

    @Test
    fun clipsAndSharedFilesDownload() = runBlocking {
        val audio = ByteArray(1000) { it.toByte() }
        server.enqueue(MockResponse.Builder().addHeader("Content-Type", "audio/mpeg").body(Buffer().write(audio)).build())
        val clip = api.sayAudio("0193abc")
        assertEquals("audio/mpeg", clip.mediaType)
        assertContentEquals(audio, clip.bytes)
        assertEquals("/api/voice/say/0193abc", server.takeRequest().target)

        server.enqueue(MockResponse.Builder().addHeader("Content-Type", "image/png").body(Buffer().write(audio)).build())
        assertEquals("image/png", api.shared("0193def").mediaType)
        assertEquals("/api/shared/0193def", server.takeRequest().target)

        server.enqueue(MockResponse.Builder().addHeader("Content-Type", "application/pdf").body(Buffer().write(audio)).build())
        val folder = Files.createTempDirectory("shared").toFile()
        val file = folder.resolve("0193def/report.pdf")
        api.saveShared("0193def", file)
        assertContentEquals(audio, file.readBytes())
        assertEquals(listOf("report.pdf"), file.parentFile.list()?.toList(), "no partial file is left")
        assertEquals("/api/shared/0193def?download=true", server.takeRequest().target)

        server.enqueue(MockResponse.Builder().code(404).body("no such shared file").build())
        val missing = assertFailsWith<DaemonException> { api.saveShared("gone", folder.resolve("gone/x")) }
        assertEquals(404, missing.status)
        assertEquals("no such shared file", missing.message)
        assertTrue(!folder.resolve("gone/x").exists())
        assertTrue(folder.deleteRecursively())
    }
}

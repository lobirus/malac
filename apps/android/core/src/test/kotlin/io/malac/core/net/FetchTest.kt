package io.malac.core.net

import io.malac.core.protocol.Block
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import kotlinx.serialization.DeserializationStrategy
import kotlinx.serialization.encoding.Decoder
import kotlinx.serialization.json.JsonElement
import mockwebserver3.MockResponse
import mockwebserver3.MockWebServer
import okhttp3.OkHttpClient
import okhttp3.Request
import java.util.concurrent.Executors
import kotlin.test.AfterTest
import kotlin.test.BeforeTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNotEquals
import kotlin.test.assertTrue

/** How REST replies are read: off the caller's thread, and failing only with [DaemonException]. */
class FetchTest {
    private val server = MockWebServer()
    private val client = OkHttpClient()

    @BeforeTest
    fun start() = server.start()

    @AfterTest
    fun stop() = server.close()

    private fun api() = DaemonApi(client, Endpoint(server.url("/"), "token"))

    @Test
    fun repliesAreReadOffTheCallersThread() = runBlocking {
        server.enqueue(MockResponse.Builder().body("""{"a":1}""").build())
        var readOn: String? = null
        val recording = object : DeserializationStrategy<JsonElement> {
            override val descriptor = JsonElement.serializer().descriptor

            override fun deserialize(decoder: Decoder): JsonElement {
                readOn = Thread.currentThread().name
                return JsonElement.serializer().deserialize(decoder)
            }
        }
        // Stands in for Android's main thread, where network reads throw.
        Executors.newSingleThreadExecutor { Thread(it, "caller") }.asCoroutineDispatcher().use { caller ->
            withContext(caller) { client.fetch(Request.Builder().url(server.url("/x")).build(), recording) }
        }
        assertNotEquals("caller", readOn)
    }

    @Test
    fun aReplyThatDoesNotDecodeIsADaemonException() = runBlocking {
        server.enqueue(MockResponse.Builder().body("<html>not malac</html>").build())
        val error = assertFailsWith<DaemonException> { api().session("a1") }
        assertEquals(200, error.status)
        assertTrue(error.message.orEmpty().startsWith("unexpected reply"), error.message)
    }

    @Test
    fun aLargeHistoryDecodes() = runBlocking {
        val screenshot = "A".repeat(3_000_000)
        val body = """{"meta":{"id":"a1","label":"main","model":"m","cwd":"/w"},"history":[""" +
            """{"role":"user","content":[{"type":"text","text":"look"},{"type":"image","media_type":"image/png","data":"$screenshot"}]}]}"""
        server.enqueue(MockResponse.Builder().body(body).build())
        val transcript = api().session("a1")
        val image = transcript.history.single().content.filterIsInstance<Block.Image>().single()
        assertEquals(screenshot.length, image.data.length)
        assertEquals("/api/sessions/a1", server.takeRequest().url.encodedPath)
    }
}

package io.malac.core.net

import io.malac.core.protocol.Role
import io.malac.core.protocol.TalkCommand
import io.malac.core.protocol.TalkMessage
import io.malac.core.voice.TALK_FRAME_BYTES
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import mockwebserver3.MockResponse
import mockwebserver3.MockWebServer
import okhttp3.OkHttpClient
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import okio.ByteString
import okio.ByteString.Companion.toByteString
import kotlin.test.AfterTest
import kotlin.test.BeforeTest
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals

class TalkConnectionTest {
    private val server = MockWebServer()
    private val texts = Channel<String>(Channel.UNLIMITED)

    /** Talk mode as the daemon runs it: ready, then each frame of audio is answered with a line and audio. */
    private inner class FakeTalk : WebSocketListener() {
        override fun onOpen(webSocket: WebSocket, response: okhttp3.Response) {
            webSocket.send("""{"type":"ready","model":"fake/gpt-realtime","voice":"marin"}""")
        }

        override fun onMessage(webSocket: WebSocket, bytes: ByteString) {
            webSocket.send("""{"type":"listening"}""")
            webSocket.send("""{"type":"transcript","role":"user","text":"${bytes.size} bytes","final":true}""")
            webSocket.send(ByteArray(960) { 7 }.toByteString())
        }

        override fun onMessage(webSocket: WebSocket, text: String) {
            texts.trySend(text)
            if (text.contains("\"stop\"")) {
                webSocket.send("""{"type":"closed","reason":"talk mode ended"}""")
                webSocket.close(1000, null)
            }
        }

        override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
            webSocket.close(1000, null)
        }
    }

    @BeforeTest
    fun start() = server.start()

    @AfterTest
    fun stop() = server.close()

    @Test
    fun audioGoesBothWaysUntilStopped() = runBlocking {
        server.enqueue(MockResponse.Builder().webSocketUpgrade(FakeTalk()).build())
        val talk = TalkConnection(OkHttpClient(), Endpoint(server.url("/"), "secret"), tab = "t1", agent = "a1")
        val events = mutableListOf<TalkConnection.Event>()
        talk.open()
        withTimeout(5_000) {
            talk.events.collect { event ->
                events += event
                val message = (event as? TalkConnection.Event.Message)?.message
                if (message is TalkMessage.Ready) talk.sendAudio(ByteArray(TALK_FRAME_BYTES + 10), offset = 10, length = TALK_FRAME_BYTES)
                if (event is TalkConnection.Event.Audio) {
                    talk.send(TalkCommand.Text("hi"))
                    talk.stop()
                }
            }
        }
        val request = server.takeRequest()
        assertEquals("/api/voice/talk?tab=t1&agent=a1", request.target)
        assertEquals("Bearer secret", request.headers["Authorization"])

        val messages = events.filterIsInstance<TalkConnection.Event.Message>().map { it.message }
        assertEquals(TalkMessage.Ready("fake/gpt-realtime", "marin"), messages[0])
        assertEquals(TalkMessage.Listening, messages[1])
        assertEquals(TalkMessage.Transcript(Role.User, "$TALK_FRAME_BYTES bytes", true), messages[2])
        assertContentEquals(ByteArray(960) { 7 }, events.filterIsInstance<TalkConnection.Event.Audio>().single().pcm)
        assertEquals(TalkConnection.Event.Closed("talk mode ended"), events.last())
        assertEquals(listOf("""{"type":"text","text":"hi"}""", """{"type":"stop"}"""), withTimeout(5_000) { listOf(texts.receive(), texts.receive()) })
    }

    @Test
    fun aRefusalEndsTheConversationWithItsReason() = runBlocking {
        server.enqueue(MockResponse.Builder().code(500).body("talk mode is off").build())
        val talk = TalkConnection(OkHttpClient(), Endpoint(server.url("/"), "secret"))
        talk.open()
        val events = withTimeout(5_000) { talk.events.toList() }
        assertEquals(listOf<TalkConnection.Event>(TalkConnection.Event.Closed("talk mode is off")), events)
        assertEquals("/api/voice/talk", server.takeRequest().target)
    }
}

package io.malac.core.net

import io.malac.core.Fixtures
import io.malac.core.protocol.Command
import io.malac.core.protocol.MalacJson
import io.malac.core.protocol.ServerMessage
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import mockwebserver3.MockResponse
import mockwebserver3.MockWebServer
import okhttp3.OkHttpClient
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import kotlin.test.AfterTest
import kotlin.test.BeforeTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertIs

class DaemonConnectionTest {
    private val server = MockWebServer()
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private val received = Channel<String>(Channel.UNLIMITED)
    private val snapshot = Fixtures.received[Fixtures.receivedMessages.indexOfFirst { it is ServerMessage.Snapshot && it.clientRef != null }]

    /** A daemon that sends a snapshot, answers `ping` and refuses everything else. */
    private inner class FakeDaemon(
        private val greeting: List<String> = listOf(snapshot),
        /** Closes the socket when asked for a snapshot, like a daemon that restarts. */
        private val restartOnResync: Boolean = false,
    ) : WebSocketListener() {
        override fun onOpen(webSocket: WebSocket, response: okhttp3.Response) {
            greeting.forEach(webSocket::send)
        }

        override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
            webSocket.close(1000, null)
        }

        override fun onMessage(webSocket: WebSocket, text: String) {
            received.trySend(text)
            val message = MalacJson.parseToJsonElement(text).jsonObject
            if (restartOnResync && message["type"]?.jsonPrimitive?.content == "snapshot") {
                webSocket.close(1001, "restarting")
                return
            }
            val ref = message["client_ref"]?.jsonPrimitive?.content ?: return
            when (message["type"]?.jsonPrimitive?.content) {
                "ping" -> webSocket.send("""{"type":"pong","client_ref":"$ref"}""")
                else -> webSocket.send("""{"type":"error","client_ref":"$ref","message":"no"}""")
            }
        }
    }

    @BeforeTest
    fun start() = server.start()

    @AfterTest
    fun stop() {
        scope.cancel()
        server.close()
    }

    private fun connection() = DaemonConnection(
        endpoint = Endpoint(server.url("/"), "secret"),
        client = OkHttpClient(),
        scope = scope,
        minDelayMillis = 20,
        requestTimeoutMillis = 2_000,
    )

    @Test
    fun connectsAndCorrelatesReplies() = runBlocking {
        server.enqueue(MockResponse.Builder().webSocketUpgrade(FakeDaemon()).build())
        val connection = connection()
        connection.start()
        withTimeout(5_000) { connection.status.first { it == DaemonConnection.Status.Online } }
        val request = server.takeRequest()
        assertEquals("Bearer secret", request.headers["Authorization"])
        assertEquals(null, request.url.queryParameter("token"), "the token stays out of the URL")
        assertEquals(null, request.headers["Origin"])
        assertEquals(Fixtures.WORKSPACE, connection.state.value.workspace?.id)

        assertIs<ServerMessage.Pong>(connection.request(Command.Ping))
        val error = assertFailsWith<CommandException> { connection.request(Command.JobList) }
        assertEquals("no", error.message)
        connection.stop()
    }

    @Test
    fun laggingResyncsAndAClosedSocketReconnects() = runBlocking {
        val lagged = """{"type":"lagged","skipped":3}"""
        server.enqueue(MockResponse.Builder().webSocketUpgrade(FakeDaemon(listOf(snapshot, lagged), restartOnResync = true)).build())
        server.enqueue(MockResponse.Builder().webSocketUpgrade(FakeDaemon()).build())
        val connection = connection()
        connection.start()
        val resync = withTimeout(5_000) { received.receive() }
        assertEquals("""{"type":"snapshot","workspace":"${Fixtures.WORKSPACE}"}""", resync)

        withTimeout(5_000) {
            while (server.requestCount < 2 || connection.status.value != DaemonConnection.Status.Online) delay(10)
        }
        assertEquals(Fixtures.WORKSPACE, connection.state.value.workspace?.id)
        connection.stop()
    }

    @Test
    fun aRefusalSaysWhyAndIsRetried() = runBlocking {
        server.enqueue(MockResponse.Builder().code(403).body("remote access is off").build())
        server.enqueue(MockResponse.Builder().webSocketUpgrade(FakeDaemon()).build())
        val connection = connection()
        val reasons = mutableListOf<String>()
        val watcher = scope.launch {
            connection.status.collect { (it as? DaemonConnection.Status.Offline)?.let { offline -> reasons += offline.reason } }
        }
        connection.start()
        withTimeout(5_000) { connection.status.first { it == DaemonConnection.Status.Online } }
        watcher.cancel()
        assertEquals(listOf("remote access is off"), reasons)
        connection.stop()
    }

    @Test
    fun aRevokedTokenStopsRetrying() = runBlocking {
        server.enqueue(MockResponse.Builder().code(401).body("""{"error":"unknown token"}""").build())
        val connection = connection()
        connection.start()
        withTimeout(5_000) { connection.status.first { it == DaemonConnection.Status.Unauthorized } }
        assertEquals(1, server.requestCount)
        connection.stop()
    }
}

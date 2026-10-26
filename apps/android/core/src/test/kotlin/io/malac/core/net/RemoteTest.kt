package io.malac.core.net

import io.malac.core.Fixtures
import io.malac.core.pairing.PairingLink
import io.malac.core.protocol.Command
import io.malac.core.protocol.DeviceList
import io.malac.core.protocol.Hello
import io.malac.core.protocol.KeepAwake
import io.malac.core.protocol.MalacJson
import io.malac.core.protocol.ServerMessage
import io.malac.core.protocol.encode
import io.malac.core.state.DaemonState
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.builtins.ListSerializer
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.int
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import mockwebserver3.MockResponse
import mockwebserver3.MockWebServer
import okhttp3.HttpUrl.Companion.toHttpUrl
import okhttp3.OkHttpClient
import kotlin.test.AfterTest
import kotlin.test.BeforeTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertIs
import kotlin.test.assertTrue

/** Pairing, devices, remote access and keep-awake, against responses recorded from a pairing-capable daemon. */
class RemoteTest {
    private val server = MockWebServer()
    private val client = OkHttpClient()

    @BeforeTest
    fun start() = server.start()

    @AfterTest
    fun stop() = server.close()

    private fun json(name: String) = Fixtures.text("remote/$name.json")

    /** A recorded `{status, body}` failure, as the daemon sent it. */
    private fun failure(name: String): MockResponse {
        val recorded = MalacJson.parseToJsonElement(json(name)).jsonObject
        val body = recorded.getValue("body")
        val text = if (body is JsonPrimitive) body.content else body.toString()
        return MockResponse.Builder().code(recorded.getValue("status").jsonPrimitive.int).body(text).build()
    }

    private fun ok(name: String) = MockResponse.Builder().body(json(name)).build()

    @Test
    fun remoteMessagesDecodeAndUpdateTheState() {
        val messages = MalacJson.decodeFromString(ListSerializer(ServerMessage.serializer()), json("messages"))
        assertTrue(messages.none { it is ServerMessage.Unknown }, messages.toString())
        val snapshot = assertIs<ServerMessage.Snapshot>(messages.first())
        assertEquals("127.0.0.1:7791", snapshot.remote?.listen)
        assertEquals("http://127.0.0.1:7791", snapshot.remote?.urls?.first()?.url)
        assertEquals("nothing is running", snapshot.awake?.reason)

        val state = messages.fold(DaemonState()) { state, message -> state.reduce(message).state }
        assertEquals(KeepAwake.Busy, state.awake?.mode)
        assertEquals(true, state.awake?.awake)
        assertEquals("caffeinate", state.awake?.method)
        assertEquals(1, state.remote?.devices)
        assertEquals(false, state.remote?.enabled)
    }

    @Test
    fun keepAwakeEncodes() {
        assertEquals(
            MalacJson.parseToJsonElement("""{"type":"keep_awake","mode":"always"}"""),
            MalacJson.parseToJsonElement(Command.SetKeepAwake(KeepAwake.Always).encode()),
        )
    }

    @Test
    fun theCodesLinkParses() {
        val reply = MalacJson.parseToJsonElement(json("pairing-code")).jsonObject
        val url = reply.getValue("links").jsonArray.first().jsonObject.getValue("url").jsonPrimitive.content
        val link = PairingLink.parse(url)!!
        assertEquals("http://127.0.0.1:7791/".toHttpUrl(), link.base)
        assertEquals("ABCDE-FGHJK", link.code)
        assertEquals(reply.getValue("code").jsonPrimitive.content, link.code)
    }

    @Test
    fun pairingAsksHelloThenTradesTheCode() = runBlocking {
        assertEquals(Hello(app = "malac", version = "1.0.1", remote = false, pairing = true), MalacJson.decodeFromString(Hello.serializer(), json("hello")))
        server.enqueue(ok("hello"))
        server.enqueue(ok("paired"))
        val paired = HttpPairingService(client).pair(server.url("/"), "ABCDE-FGHJK", "Pixel 9")
        assertEquals("<device token>", paired.token)
        assertEquals("93c5e175", paired.device?.id)
        assertEquals(true, paired.device?.current)

        assertEquals("/api/hello", server.takeRequest().url.encodedPath)
        val pair = server.takeRequest()
        assertEquals("/api/pair", pair.url.encodedPath)
        assertEquals(null, pair.headers["Authorization"])
        assertEquals(
            MalacJson.parseToJsonElement("""{"code":"ABCDE-FGHJK","name":"Pixel 9","kind":"app"}"""),
            MalacJson.parseToJsonElement(pair.body?.utf8().orEmpty()),
        )
    }

    @Test
    fun aWrongCodeIsForbidden() = runBlocking {
        server.enqueue(ok("hello"))
        server.enqueue(failure("pair-invalid"))
        val error = assertFailsWith<DaemonException> { HttpPairingService(client).pair(server.url("/"), "ABCDE-FGHJK", "Pixel 9") }
        assertTrue(error.forbidden)
        assertEquals("the pairing code is wrong, used or expired", error.message)
    }

    @Test
    fun aRevokedTokenIsUnauthorized() = runBlocking {
        server.enqueue(failure("revoked"))
        val error = assertFailsWith<DaemonException> { DaemonApi(client, Endpoint(server.url("/"), "old")).snapshot() }
        assertTrue(error.unauthorized)
        assertEquals("missing or invalid token", error.message)
    }

    @Test
    fun forgettingADaemonRevokesThisDevice() = runBlocking {
        val device = MalacJson.decodeFromString(DeviceList.serializer(), json("devices")).devices.single()
        val recorded = MalacJson.parseToJsonElement(json("devices")).jsonObject.getValue("devices").jsonArray.single()
        server.enqueue(ok("devices"))
        server.enqueue(MockResponse.Builder().body("""{"revoked":$recorded}""").build())
        DaemonApi(client, Endpoint(server.url("/"), "secret")).revokeSelf()

        assertEquals("/api/devices", server.takeRequest().url.encodedPath)
        val revoke = server.takeRequest()
        assertEquals("DELETE", revoke.method)
        assertEquals("/api/devices/${device.id}", revoke.url.encodedPath)
        assertEquals("Bearer secret", revoke.headers["Authorization"])
    }
}

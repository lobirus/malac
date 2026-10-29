package io.malac.core.net

import kotlinx.coroutines.runBlocking
import mockwebserver3.MockResponse
import mockwebserver3.MockWebServer
import okhttp3.OkHttpClient
import kotlin.test.AfterTest
import kotlin.test.BeforeTest
import kotlin.test.Test
import kotlin.test.assertEquals

/** Registering for pushes, as `crates/malac/src/web/push.rs` expects it. */
class PushApiTest {
    private val server = MockWebServer()
    private val api by lazy { DaemonApi(OkHttpClient(), Endpoint(server.url("/"), "token")) }

    @BeforeTest
    fun start() = server.start()

    @AfterTest
    fun stop() = server.close()

    @Test
    fun readsTheKeyAndRegisters() = runBlocking {
        server.enqueue(
            MockResponse.Builder().body("""{"enabled":true,"wait_seconds":15,"vapid_key":"BKey","registered":false}""").build(),
        )
        server.enqueue(MockResponse.Builder().body("""{"registered":true}""").build())
        server.enqueue(MockResponse.Builder().body("""{"registered":false,"removed":true}""").build())

        val status = api.push()
        assertEquals("BKey", status.vapidKey)
        assertEquals(15, status.waitSeconds)
        assertEquals("GET", server.takeRequest().method)

        api.registerPush("https://ntfy.sh/upAbc?up=1", "BPub", "secret")
        val register = server.takeRequest()
        assertEquals("PUT", register.method)
        assertEquals("/api/push", register.url.encodedPath)
        assertEquals("Bearer token", register.headers["Authorization"])
        assertEquals("""{"endpoint":"https://ntfy.sh/upAbc?up=1","p256dh":"BPub","auth":"secret"}""", register.body?.utf8())

        api.unregisterPush()
        assertEquals("DELETE", server.takeRequest().method)
    }
}

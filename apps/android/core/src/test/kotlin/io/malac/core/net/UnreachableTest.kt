package io.malac.core.net

import kotlinx.coroutines.runBlocking
import okhttp3.HttpUrl.Companion.toHttpUrl
import okhttp3.OkHttpClient
import java.net.SocketTimeoutException
import java.net.UnknownHostException
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertTrue

/** What the user reads when a daemon cannot be reached, Tailscale addresses above all. */
class UnreachableTest {
    @Test
    fun tellsTailnetAddressesApart() {
        assertTrue(isTailnet("laptop.tail759694.ts.net"))
        assertTrue(isTailnet("LAPTOP.TAIL759694.TS.NET"))
        assertTrue(isTailnet("100.75.183.108"))
        assertTrue(isTailnet("100.64.0.1"))
        assertFalse(isTailnet("100.128.0.1"))
        assertFalse(isTailnet("192.168.1.84"))
        assertFalse(isTailnet("devbox.local"))
        assertFalse(isTailnet("example.net"))
    }

    @Test
    fun explainsWhatToDo() {
        val name = unreachable("laptop.tail759694.ts.net", UnknownHostException("laptop.tail759694.ts.net"))
        assertTrue(name.startsWith("laptop.tail759694.ts.net is a Tailscale address"), name)
        assertTrue("Private DNS" in name, name)

        val address = unreachable("100.75.183.108", SocketTimeoutException("connect timed out"))
        assertTrue("Turn on Tailscale on this phone" in address, address)
        assertFalse("Private DNS" in address, address)

        assertEquals("Cannot find devbox.local. Check the address, and that this phone is online.", unreachable("devbox.local", UnknownHostException("devbox.local")))
        assertEquals("connect timed out", unreachable("192.168.1.84", SocketTimeoutException("connect timed out")))
    }

    @Test
    fun pairingWithATailnetNameOffTheTailnetSaysSo() = runBlocking {
        // A phone that is not on the tailnet cannot resolve the name at all.
        val client = OkHttpClient.Builder().dns { throw UnknownHostException(it) }.build()
        val error = assertFailsWith<DaemonException> {
            HttpPairingService(client).pair("https://laptop.tail759694.ts.net/".toHttpUrl(), "ABCDE-FGHJK", "Pixel 9")
        }
        assertEquals(0, error.status)
        assertTrue(error.message.orEmpty().startsWith("laptop.tail759694.ts.net is a Tailscale address"), error.message)
    }
}

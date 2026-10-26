package io.malac.core.pairing

import okhttp3.HttpUrl.Companion.toHttpUrl
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

class PairingTest {
    @Test
    fun codesNormalizeLikeTheDaemon() {
        assertEquals("ABCDE1GH10", PairingCode.normalize("abcde-igh lo"))
        assertEquals("ABCDE-FGHJK", PairingCode.format("abcdefghjk"))
        assertTrue(PairingCode.isValid("ABCDE-FGHJK"))
        assertTrue(PairingCode.isValid("abcde fghjk"))
        assertFalse(PairingCode.isValid("ABCDE-FGHJ"))
        assertFalse(PairingCode.isValid("ABCDE-FGHJU"), "U is not in Crockford's alphabet")
    }

    @Test
    fun pairingLinks() {
        val lan = PairingLink.parse("http://192.168.1.20:7700/#pair=abcde-fghjk")!!
        assertEquals("http://192.168.1.20:7700/".toHttpUrl(), lan.base)
        assertEquals("ABCDE-FGHJK", lan.code)
        assertNull(lan.token)

        val tunnel = PairingLink.parse(" https://pig.trycloudflare.com/#pair=ABCDE-FGHJK\n")!!
        assertEquals("https://pig.trycloudflare.com/".toHttpUrl(), tunnel.base)

        val token = PairingLink.parse("http://127.0.0.1:7700/?token=8995bbb2")!!
        assertEquals("8995bbb2", token.token)
        assertNull(token.code)

        val proxied = PairingLink.parse("https://example.com/malac/#pair=ABCDE-FGHJK")!!
        assertEquals("https://example.com/malac/api/ws".toHttpUrl(), proxied.base.resolve("api/ws"))

        val bare = PairingLink.parse("192.168.1.20:7700")!!
        assertEquals("http://192.168.1.20:7700/".toHttpUrl(), bare.base)
        assertNull(PairingLink.parse("not a url at all"))
    }
}

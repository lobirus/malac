package io.malac.core.pairing

import io.malac.core.protocol.DeviceView
import okhttp3.HttpUrl
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull

/**
 * A one-time pairing code from `malac remote pair`: ten characters of
 * Crockford's base32, shown as `ABCDE-FGHJK`.
 */
object PairingCode {
    private const val ALPHABET = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"
    const val LENGTH = 10

    /** Letters and digits only, upper case, with the look-alikes O, I and L read as 0, 1 and 1. */
    fun normalize(code: String): String = code
        .filter { it in 'a'..'z' || it in 'A'..'Z' || it in '0'..'9' }
        .uppercase()
        .map {
            when (it) {
                'O' -> '0'
                'I', 'L' -> '1'
                else -> it
            }
        }
        .joinToString("")

    fun isValid(code: String): Boolean = normalize(code).let { it.length == LENGTH && it.all { c -> c in ALPHABET } }

    fun format(code: String): String = normalize(code).let { if (it.length == LENGTH) "${it.take(5)}-${it.drop(5)}" else it }
}

/**
 * What a QR code or a pasted link says about a daemon: its address and
 * either a pairing code (`<base>/#pair=CODE`) or a token (`<base>/?token=…`).
 */
data class PairingLink(val base: HttpUrl, val code: String? = null, val token: String? = null) {
    companion object {
        fun parse(text: String): PairingLink? {
            val trimmed = text.trim()
            val withScheme = if ("://" in trimmed) trimmed else "http://$trimmed"
            val url = withScheme.toHttpUrlOrNull() ?: return null
            val fragment = url.fragment.orEmpty()
            val code = fragment.split('&').firstNotNullOfOrNull { part ->
                part.substringAfter("pair=", "").takeIf { part.startsWith("pair=") && it.isNotEmpty() }
            }
            val token = url.queryParameter("token")?.takeIf { it.isNotEmpty() }
            return PairingLink(baseOf(url), code?.let(PairingCode::format), token)
        }

        /** The daemon's address without the query or fragment; a path is kept, for a daemon behind a reverse proxy. */
        fun baseOf(url: HttpUrl): HttpUrl {
            val path = url.encodedPath.let { if (it.endsWith("/")) it else "$it/" }
            return url.newBuilder().encodedPath(path).query(null).fragment(null).build()
        }
    }
}

/** A device token the daemon issued for this phone. */
data class Paired(val token: String, val device: DeviceView?, val version: String)

/** Trades a pairing code for a device token. Behind an interface so pairing can follow the daemon. */
fun interface PairingService {
    suspend fun pair(base: HttpUrl, code: String, deviceName: String): Paired
}

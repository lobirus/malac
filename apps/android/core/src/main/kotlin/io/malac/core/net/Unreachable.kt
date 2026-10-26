package io.malac.core.net

import java.io.InterruptedIOException
import java.net.SocketException
import java.net.UnknownHostException

/**
 * Why [host] could not be reached, in words for the user. A Tailscale
 * address works only on devices in that tailnet, which is the usual reason
 * it fails from a phone: the name does not resolve, or the address times out.
 */
fun unreachable(host: String, error: Throwable): String {
    val network = error is UnknownHostException || error is SocketException || error is InterruptedIOException
    return when {
        isTailnet(host) && network -> buildString {
            append("$host is a Tailscale address, which only devices on your tailnet can reach. ")
            append("Turn on Tailscale on this phone, signed in to the same tailnet, then try again.")
            if (error is UnknownHostException) append(" If it is on, set Private DNS (Settings → Network) to Off or Automatic.")
        }
        error is UnknownHostException -> "Cannot find $host. Check the address, and that this phone is online."
        else -> error.message ?: "Cannot reach the daemon"
    }
}

/** A MagicDNS name, or an address in Tailscale's 100.64.0.0/10 range. */
fun isTailnet(host: String): Boolean {
    if (host.endsWith(".ts.net", ignoreCase = true)) return true
    val parts = host.split('.').map { it.toIntOrNull() ?: return false }
    return parts.size == 4 && parts[0] == 100 && parts[1] in 64..127
}

package io.malac.app.ui.daemons

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import io.malac.app.AppGraph
import io.malac.core.net.DaemonApi
import io.malac.core.net.DaemonException
import io.malac.core.net.Endpoint
import io.malac.core.pairing.PairingCode
import io.malac.core.pairing.PairingLink
import kotlinx.coroutines.launch
import kotlinx.serialization.SerializationException

/** Pairing a new daemon: from a scanned link, or an address with a code or a token. */
class AddDaemonViewModel(private val graph: AppGraph, defaultDeviceName: String) : ViewModel() {
    var address by mutableStateOf("")
    var code by mutableStateOf("")
    var token by mutableStateOf("")
    var name by mutableStateOf("")
    var deviceName by mutableStateOf(defaultDeviceName)
    var busy by mutableStateOf(false)
        private set
    var error by mutableStateOf<String?>(null)
        private set

    val canPair: Boolean
        get() = !busy && PairingLink.parse(address) != null && (PairingCode.isValid(code) || token.isNotBlank())

    /** Fills the form from a QR code or a pasted link. */
    fun useLink(text: String): Boolean {
        val link = PairingLink.parse(text) ?: return false
        address = link.base.toString()
        link.code?.let { code = it }
        link.token?.let { token = it }
        error = null
        return true
    }

    fun pair(onDone: () -> Unit) {
        val link = PairingLink.parse(address) ?: return
        val base = link.base
        busy = true
        error = null
        viewModelScope.launch {
            try {
                val label = name.trim().ifEmpty { base.host }
                if (PairingCode.isValid(code)) {
                    val paired = graph.pairing.pair(base, PairingCode.format(code), deviceName.trim().ifEmpty { "Android" })
                    val daemon = graph.daemons.add(label, base, paired.token, paired.device?.id)
                    graph.scope.launch { graph.push.register(daemon) }
                } else {
                    // A token from `?token=` is checked before it is saved.
                    DaemonApi(graph.http, Endpoint(base, token.trim())).snapshot()
                    graph.daemons.add(label, base, token.trim(), null)
                }
                onDone()
            } catch (failure: DaemonException) {
                error = failure.message
            } catch (failure: SerializationException) {
                error = failure.message
            } finally {
                busy = false
            }
        }
    }
}

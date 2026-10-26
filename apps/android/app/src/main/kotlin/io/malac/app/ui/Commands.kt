package io.malac.app.ui

import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import io.malac.app.connection.ActiveDaemon
import io.malac.core.net.CommandException
import io.malac.core.protocol.Command
import io.malac.core.protocol.Decision
import io.malac.core.protocol.ServerMessage
import io.malac.core.state.withoutRequest
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch

/** Sends a screen's commands and reports failures instead of throwing them. */
class Commands(
    private val active: ActiveDaemon,
    private val scope: CoroutineScope,
    private val onReport: (String) -> Unit,
) {
    /** Requests answered here and not settled yet; their buttons are disabled. */
    var answering by mutableStateOf(emptySet<String>())
        private set

    fun report(message: String) = onReport(message)

    fun send(command: Command, onReply: (ServerMessage) -> Unit = {}) {
        scope.launch {
            try {
                onReply(active.connection.request(command))
            } catch (error: CommandException) {
                report(error.message.orEmpty())
            }
        }
    }

    fun approve(requestId: String, decision: Decision) {
        answering = answering + requestId
        scope.launch {
            try {
                active.connection.request(Command.Approve(requestId, decision))
            } catch (error: CommandException) {
                // Refused means another client answered first; the request is gone.
                if (error.refused) active.connection.edit { it.withoutRequest(requestId) } else report(error.message.orEmpty())
            } finally {
                answering = answering - requestId
            }
        }
    }
}

@Composable
fun rememberCommands(active: ActiveDaemon, report: (String) -> Unit): Commands {
    val scope = rememberCoroutineScope()
    return remember(active) { Commands(active, scope, report) }
}

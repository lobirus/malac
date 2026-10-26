package io.malac.app.ui.agent

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import io.malac.app.connection.ActiveDaemon
import io.malac.core.net.CommandException
import io.malac.core.net.DaemonException
import io.malac.core.protocol.Block
import io.malac.core.protocol.Command
import io.malac.core.protocol.Message
import io.malac.core.protocol.ServerMessage
import io.malac.core.state.DaemonState
import io.malac.core.transcript.Draft
import io.malac.core.transcript.Transcript
import io.malac.core.transcript.TranscriptItem
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.conflate
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.flowOn
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

/** The rows of a transcript and whether its saved history is still loading. */
data class TranscriptUi(
    val items: List<TranscriptItem> = emptyList(),
    val draft: Draft? = null,
    val loading: Boolean = true,
    val error: String? = null,
)

/**
 * One agent's transcript and composer. The transcript is rebuilt from the
 * saved history and the agent's event log whenever either changes, which
 * keeps it a pure function of the daemon state.
 */
class AgentViewModel(private val active: ActiveDaemon, val agentId: String?) : ViewModel() {
    private class History(val messages: List<Message>, val cutoff: Long, val error: String?)

    private val history = MutableStateFlow<History?>(null)
    private val outgoing = MutableStateFlow<List<Pair<String, List<Block.Image>>>>(emptyList())

    var draftText by mutableStateOf("")
    var images by mutableStateOf<List<PickedImage>>(emptyList())
        private set
    var sending by mutableStateOf(false)
        private set

    /** The model for a new agent; an existing agent's model changes right away. */
    var newAgentModel by mutableStateOf<String?>(null)

    val transcript: StateFlow<TranscriptUi> = combine(active.connection.state, history, outgoing) { state, history, outgoing ->
        build(state, history, outgoing)
    }
        .conflate()
        .flowOn(Dispatchers.Default)
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), TranscriptUi(loading = agentId != null))

    init {
        if (agentId != null) {
            viewModelScope.launch {
                // A snapshot clears the event logs, so the history is fetched again with a new cutoff.
                active.connection.state.map { it.generation }.distinctUntilChanged().collectLatest { load(agentId) }
            }
        }
    }

    private suspend fun load(agent: String) {
        val cutoff = active.connection.state.value.lastSeq
        history.value = try {
            History(active.api.session(agent).history, cutoff, null)
        } catch (error: DaemonException) {
            // A brand-new agent may not have a saved transcript yet.
            if (error.status == 404) History(emptyList(), cutoff, null) else History(emptyList(), cutoff, error.message)
        }
    }

    private fun build(state: DaemonState, history: History?, outgoing: List<Pair<String, List<Block.Image>>>): TranscriptUi {
        val agent = agentId ?: return TranscriptUi(loading = false)
        history ?: return TranscriptUi(loading = true)
        val transcript = Transcript()
        outgoing.forEach { (text, images) -> transcript.expectImages(text, images) }
        transcript.load(history.messages)
        state.log(agent).filter { it.seq > history.cutoff }.forEach(transcript::apply)
        transcript.claim(state.pendingFor(agent))
        transcript.linkChildren(state.childrenOf(agent))
        return TranscriptUi(transcript.items, transcript.draft, loading = false, error = history.error)
    }

    fun addImages(picked: List<PickedImage>) {
        images = (images + picked).take(MAX_IMAGES)
    }

    fun removeImage(image: PickedImage) {
        images = images - image
    }

    /** Prompts this agent, or starts a new one in [tab]; [onStarted] gets the new agent's id. */
    fun send(tab: String?, onStarted: (String) -> Unit, onError: (String) -> Unit) {
        val text = draftText.trim()
        if ((text.isEmpty() && images.isEmpty()) || sending) return
        val attached = images
        val command = Command.Prompt(
            text = text,
            agent = agentId,
            tab = if (agentId == null) tab else null,
            images = attached.map { it.input },
            model = if (agentId == null) newAgentModel else null,
        )
        sending = true
        if (attached.isNotEmpty()) outgoing.update { it + (text to attached.map { image -> image.block }) }
        viewModelScope.launch {
            try {
                val ack = active.connection.request(command) as? ServerMessage.Ack
                draftText = ""
                images = emptyList()
                if (agentId == null) ack?.agent?.let(onStarted)
            } catch (error: CommandException) {
                onError(error.message.orEmpty())
            } finally {
                sending = false
            }
        }
    }

    private companion object {
        const val MAX_IMAGES = 6
    }
}

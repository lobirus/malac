package io.malac.app.ui.agent

import android.content.Context
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.input.TextFieldValue
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import io.malac.app.connection.ActiveDaemon
import io.malac.app.voice.Dictation
import io.malac.core.net.CommandException
import io.malac.core.net.DaemonException
import io.malac.core.protocol.Block
import io.malac.core.protocol.Command
import io.malac.core.protocol.DecisionRecord
import io.malac.core.protocol.Message
import io.malac.core.protocol.PendingApproval
import io.malac.core.protocol.ServerMessage
import io.malac.core.state.DaemonState
import io.malac.core.transcript.Draft
import io.malac.core.transcript.Transcript
import io.malac.core.transcript.TranscriptItem
import io.malac.core.transcript.imagesSavedAt
import io.malac.core.voice.insertDictation
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
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
class AgentViewModel(context: Context, private val active: ActiveDaemon, val agentId: String?) : ViewModel() {
    /** A saved transcript, read after [cutoff]; [open] are the requests that were waiting then. */
    private class History(
        val messages: List<Message>,
        val decisions: List<DecisionRecord>,
        val cutoff: Long,
        val open: List<PendingApproval>,
        val error: String?,
    )

    private val history = MutableStateFlow<History?>(null)
    private val outgoing = MutableStateFlow<List<Pair<String, List<Block.Image>>>>(emptyList())

    /** The prompt being written, with its cursor, where dictation goes. */
    var prompt by mutableStateOf(TextFieldValue(""))
    val draftText: String get() = prompt.text
    var images by mutableStateOf<List<PickedImage>>(emptyList())
        private set
    var sending by mutableStateOf(false)
        private set

    /** The model for a new agent; an existing agent's model changes right away. */
    var newAgentModel by mutableStateOf<String?>(null)

    val dictation = Dictation(context.applicationContext, active.api, viewModelScope)

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
            viewModelScope.launch {
                // Live tool results show their images once the history has them, like the web UI.
                combine(active.connection.state, history) { state, history -> history?.let { imagesSavedAt(state.log(agentId), it.cutoff) } }
                    .distinctUntilChanged()
                    .flowOn(Dispatchers.Default)
                    .collectLatest { saved ->
                        if (saved != null) {
                            delay(400)
                            load(agentId)
                        }
                    }
            }
        }
    }

    private suspend fun load(agent: String) {
        val state = active.connection.state.value
        val cutoff = state.lastSeq
        val open = state.pendingFor(agent)
        history.value = try {
            val saved = active.api.session(agent)
            History(saved.history, saved.decisions, cutoff, open, null)
        } catch (error: DaemonException) {
            // A brand-new agent may not have a saved transcript yet.
            History(emptyList(), emptyList(), cutoff, open, error.message.takeUnless { error.status == 404 })
        }
    }

    private fun build(state: DaemonState, history: History?, outgoing: List<Pair<String, List<Block.Image>>>): TranscriptUi {
        val agent = agentId ?: return TranscriptUi(loading = false)
        history ?: return TranscriptUi(loading = true)
        val transcript = Transcript()
        outgoing.forEach { (text, images) -> transcript.expectImages(text, images) }
        transcript.load(history.messages, history.decisions)
        // Requests asked before the cutoff are answered after it: claimed first, the answer finds its row.
        transcript.claim(history.open)
        state.log(agent).filter { it.seq > history.cutoff }.forEach(transcript::apply)
        transcript.claim(state.pendingFor(agent))
        transcript.linkChildren(state.childrenOf(agent))
        return TranscriptUi(transcript.items, transcript.draft, loading = false, error = history.error)
    }

    /** Starts dictation, or stops it and puts what was said at the cursor. */
    fun dictate(onError: (String) -> Unit) {
        when (dictation.state) {
            Dictation.State.Idle -> dictation.start(onText = ::insert, onError = onError)
            is Dictation.State.Recording -> dictation.finish()
            Dictation.State.Transcribing -> Unit
        }
    }

    private fun insert(spoken: String) {
        val value = prompt
        val inserted = insertDictation(value.text, value.selection.start, value.selection.end, spoken)
        prompt = TextFieldValue(inserted.text, TextRange(inserted.cursor))
    }

    override fun onCleared() {
        dictation.cancel()
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
                prompt = TextFieldValue("")
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

package io.malac.app.ui.agent

import androidx.compose.foundation.gestures.scrollBy
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.KeyboardArrowDown
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SmallFloatingActionButton
import androidx.compose.material3.FloatingActionButtonDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import io.malac.app.R
import io.malac.app.connection.ActiveDaemon
import io.malac.app.ui.Commands
import io.malac.app.ui.components.ConnectionBanner
import io.malac.app.ui.components.EmptyState
import io.malac.app.ui.components.ScreenTopBar
import io.malac.app.ui.label
import io.malac.core.format.firstLine
import io.malac.core.format.truncate
import io.malac.core.net.DaemonConnection
import io.malac.core.protocol.Command
import io.malac.core.state.DaemonState
import io.malac.core.state.Phase
import io.malac.core.transcript.TranscriptItem
import kotlinx.coroutines.flow.filter
import kotlinx.coroutines.launch

/**
 * An agent's transcript with its composer, or with [agentId] null a
 * composer that starts a new agent in [newInTab].
 */
@Composable
fun AgentScreen(
    active: ActiveDaemon,
    commands: Commands,
    agentId: String?,
    newInTab: String?,
    status: DaemonConnection.Status,
    onOpenAgent: (String) -> Unit,
    onStarted: (String) -> Unit,
    onBack: () -> Unit,
) {
    val viewModel = viewModel(key = agentId ?: "new:$newInTab") { AgentViewModel(active, agentId) }
    val state by active.connection.state.collectAsStateWithLifecycle()
    val transcript by viewModel.transcript.collectAsStateWithLifecycle()
    val agent = agentId?.let { state.agents[it] }
    val tab = agent?.let { state.tabOf(it.id) } ?: state.tab(newInTab)
    val phase = agent?.let { state.phase(it) }
    val online = status == DaemonConnection.Status.Online

    Column(Modifier.fillMaxSize()) {
        ScreenTopBar(
            title = agent?.displayName ?: stringResource(R.string.new_agent),
            subtitle = listOfNotNull(phase?.label(), agent?.model?.ifEmpty { null }, tab?.name).joinToString(" · "),
            onBack = onBack,
        )
        ConnectionBanner(status, state.shutdownReason, active.connection::reconnectNow)
        Box(Modifier.weight(1f)) {
            when {
                agentId == null -> EmptyState(stringResource(R.string.new_agent_hint, tab?.name.orEmpty()))
                transcript.loading -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { CircularProgressIndicator() }
                transcript.items.isEmpty() && transcript.draft == null -> EmptyState(
                    agent?.displayName.orEmpty(),
                    stringResource(if (agent?.live == true) R.string.waiting_first_message else R.string.no_messages),
                )
                else -> TranscriptList(transcript, state, agent?.cwd, commands, onOpenAgent)
            }
        }
        Composer(
            viewModel = viewModel,
            models = state.models,
            model = if (agent != null) agent.model else viewModel.newAgentModel ?: tab?.options?.model ?: state.defaultModel,
            mode = tab?.mode,
            running = phase == Phase.Running || phase == Phase.AwaitingApproval,
            // A saved agent of another workspace is not in the state, but prompting it resumes it.
            enabled = online && (agentId != null || tab != null),
            onModel = { model ->
                if (agentId != null) {
                    commands.send(Command.SetModel(agentId, model))
                } else {
                    viewModel.newAgentModel = model
                }
            },
            onMode = { mode -> tab?.let { commands.send(Command.SetMode(mode, it.id)) } },
            onSend = { viewModel.send(tab?.id, onStarted, onError = { commands.report(it) }) },
            onStop = { agentId?.let { commands.send(Command.Cancel(it)) } },
        )
    }
}

/** Far enough to reach the end of the last row, however long it is. */
private const val TO_END = 1_000_000f

@Composable
private fun TranscriptList(
    transcript: TranscriptUi,
    state: DaemonState,
    cwd: String?,
    commands: Commands,
    onOpenAgent: (String) -> Unit,
) {
    val list = rememberLazyListState()
    val scope = rememberCoroutineScope()
    // Follows the end while the user has not scrolled away from it.
    var pinned by remember { mutableStateOf(true) }
    LaunchedEffect(list) {
        snapshotFlow { list.isScrollInProgress }.filter { !it }.collect { pinned = !list.canScrollForward }
    }
    val size = transcript.items.size + if (transcript.draft != null) 1 else 0
    val growth = transcript.draft?.let { it.text.length + it.thinking.length } ?: 0
    LaunchedEffect(size, growth) {
        if (pinned && size > 0) {
            list.scrollToItem(size - 1)
            list.scrollBy(TO_END)
        }
    }
    Box(Modifier.fillMaxSize()) {
        LazyColumn(
            state = list,
            contentPadding = PaddingValues(16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
            modifier = Modifier.fillMaxSize(),
        ) {
            transcript.error?.let { error ->
                item(key = "error") { EventLine(stringResource(R.string.history_failed, error)) }
            }
            items(transcript.items, key = { it.key }, contentType = { it::class }) { item ->
                TranscriptRow(item, state, cwd, commands, onOpenAgent)
            }
            transcript.draft?.let { draft ->
                item(key = "draft") { DraftRow(draft) }
            }
        }
        if (!pinned) {
            SmallFloatingActionButton(
                onClick = {
                    pinned = true
                    scope.launch {
                        list.scrollToItem((size - 1).coerceAtLeast(0))
                        list.scrollBy(TO_END)
                    }
                },
                containerColor = MaterialTheme.colorScheme.surfaceContainerHighest,
                elevation = FloatingActionButtonDefaults.elevation(0.dp, 0.dp, 0.dp, 0.dp),
                modifier = Modifier.align(Alignment.BottomEnd).padding(16.dp),
            ) {
                Icon(Icons.Filled.KeyboardArrowDown, contentDescription = stringResource(R.string.jump_to_latest))
            }
        }
    }
}

@Composable
private fun TranscriptRow(item: TranscriptItem, state: DaemonState, cwd: String?, commands: Commands, onOpenAgent: (String) -> Unit) {
    val modifier = Modifier.fillMaxWidth()
    Box(modifier) {
        when (item) {
            is TranscriptItem.User -> UserRow(item)
            is TranscriptItem.Assistant -> AssistantRow(item)
            is TranscriptItem.Thinking -> Disclosure(stringResource(R.string.thinking), firstLineOf(item.text), item.text)
            is TranscriptItem.Recalled -> Disclosure(stringResource(R.string.recalled_memories), firstLineOf(item.text), item.text)
            is TranscriptItem.Tool -> ToolRow(
                call = item.call,
                cwd = cwd,
                answering = item.call.requestId in commands.answering,
                onDecide = commands::approve,
            )
            is TranscriptItem.Subagent -> {
                val child = item.call.child?.let { state.agents[it] }
                SubagentCard(
                    call = item.call,
                    child = child,
                    waiting = child != null && state.phase(child) == Phase.AwaitingApproval,
                    nested = child?.let { state.childrenOf(it.id).size } ?: 0,
                    onOpen = onOpenAgent,
                )
            }
            is TranscriptItem.Plan -> PlanCard(item)
            is TranscriptItem.Compacted -> Disclosure(stringResource(R.string.compacted), "", item.summary)
            is TranscriptItem.ModelChanged -> EventLine(stringResource(R.string.model_changed, item.model))
            is TranscriptItem.Notice -> NoticeRow(item)
            is TranscriptItem.TurnEnd -> TurnFooter(item.summary)
        }
    }
}

private fun firstLineOf(text: String) = truncate(firstLine(text), 120)

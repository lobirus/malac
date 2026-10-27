package io.malac.app.ui.talk

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.Button
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import io.malac.app.AppGraph
import io.malac.app.R
import io.malac.app.connection.ActiveDaemon
import io.malac.app.ui.agent.LevelMeter
import io.malac.app.ui.components.EmptyState
import io.malac.app.ui.components.ScreenTopBar
import io.malac.app.ui.theme.malac
import io.malac.core.protocol.Role
import io.malac.core.voice.TalkLine
import io.malac.core.voice.TalkState

/**
 * Talk mode: what state the conversation is in, what was said and asked,
 * and buttons to mute and end it. The screen stays on while it is open;
 * back ends the conversation.
 */
@Composable
fun TalkScreen(graph: AppGraph, active: ActiveDaemon, tab: String?, agent: String?, onEnd: () -> Unit) {
    // The token is decrypted once, not on every recomposition.
    val endpoint = remember(active.daemon) { graph.daemons.endpoint(active.daemon) }
    if (endpoint == null) {
        Column(Modifier.fillMaxSize()) {
            ScreenTopBar(title = stringResource(R.string.talk_mode), onBack = onEnd)
            EmptyState(stringResource(R.string.banner_unauthorized))
        }
        return
    }
    val context = LocalContext.current
    val viewModel = viewModel(key = "talk:${active.daemon.id}:$tab:$agent") {
        TalkViewModel(context, graph.http, endpoint, tab, agent, graph.speech)
    }
    val state = viewModel.state
    val playing by viewModel.playing.collectAsStateWithLifecycle()
    val end = {
        viewModel.end()
        onEnd()
    }
    BackHandler(onBack = end)
    KeepScreenOn()

    val phase = if (playing && state.phase != TalkState.Phase.Ended) TalkState.Phase.Speaking else state.phase
    Column(Modifier.fillMaxSize()) {
        ScreenTopBar(
            title = stringResource(R.string.talk_mode),
            subtitle = listOfNotNull(state.model, state.voice).joinToString(" · "),
            onBack = end,
        )
        Column(
            Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 24.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(
                phase.label(),
                style = MaterialTheme.typography.headlineSmall,
                modifier = Modifier.semantics {
                    heading()
                    liveRegion = LiveRegionMode.Polite
                },
            )
            Text(
                hint(state, viewModel.muted),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.malac.text2,
                textAlign = TextAlign.Center,
            )
            LevelMeter(viewModel.level, Modifier.width(160.dp).padding(top = 8.dp))
        }
        HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
        Lines(state.lines, Modifier.weight(1f))
        state.error?.let { error ->
            Text(
                error,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.malac.red,
                modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp).semantics { liveRegion = LiveRegionMode.Polite },
            )
        }
        HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
        Row(
            Modifier.fillMaxWidth().navigationBarsPadding().padding(16.dp),
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            if (state.phase == TalkState.Phase.Ended) {
                Button(onClick = onEnd, modifier = Modifier.weight(1f).heightIn(min = 48.dp)) { Text(stringResource(R.string.close)) }
            } else {
                OutlinedButton(onClick = viewModel::toggleMute, modifier = Modifier.weight(1f).heightIn(min = 48.dp)) {
                    Icon(
                        painterResource(if (viewModel.muted) R.drawable.ic_mic_off else R.drawable.ic_mic),
                        contentDescription = null,
                        modifier = Modifier.size(18.dp),
                    )
                    Spacer(Modifier.width(8.dp))
                    Text(stringResource(if (viewModel.muted) R.string.unmute else R.string.mute))
                }
                Button(onClick = end, modifier = Modifier.weight(1f).heightIn(min = 48.dp)) { Text(stringResource(R.string.end_talk)) }
            }
        }
    }
}

@Composable
private fun Lines(lines: List<TalkLine>, modifier: Modifier) {
    val list = rememberLazyListState()
    val last = lines.lastOrNull()
    LaunchedEffect(lines.size, (last as? TalkLine.Said)?.text?.length) {
        if (lines.isNotEmpty()) list.animateScrollToItem(lines.lastIndex)
    }
    if (lines.isEmpty()) {
        EmptyState(stringResource(R.string.talk_empty), modifier = modifier)
        return
    }
    LazyColumn(
        state = list,
        contentPadding = PaddingValues(16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
        modifier = modifier.fillMaxWidth(),
    ) {
        items(lines, key = { it.key }) { line ->
            when (line) {
                is TalkLine.Said -> Column(Modifier.semantics(mergeDescendants = true) {}) {
                    Text(
                        stringResource(if (line.role == Role.User) R.string.you else R.string.talk_voice),
                        style = MaterialTheme.typography.labelMedium,
                        color = MaterialTheme.malac.text2,
                    )
                    SelectionContainer { Text(line.text, style = MaterialTheme.typography.bodyMedium) }
                }
                is TalkLine.Tool -> Text(line.summary, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.malac.text3)
            }
        }
    }
}

@Composable
private fun TalkState.Phase.label(): String = stringResource(
    when (this) {
        TalkState.Phase.Connecting -> R.string.talk_connecting
        TalkState.Phase.Listening -> R.string.talk_listening
        TalkState.Phase.Thinking -> R.string.talk_thinking
        TalkState.Phase.Speaking -> R.string.talk_speaking
        TalkState.Phase.Ended -> R.string.talk_ended
    },
)

@Composable
private fun hint(state: TalkState, muted: Boolean): String = when {
    state.phase == TalkState.Phase.Ended -> state.ended.orEmpty()
    state.phase == TalkState.Phase.Connecting -> stringResource(R.string.talk_hint_connecting)
    muted -> stringResource(R.string.talk_hint_muted)
    else -> stringResource(R.string.talk_hint)
}

/** Talking takes a while without touching the screen. */
@Composable
private fun KeepScreenOn() {
    val view = LocalView.current
    DisposableEffect(view) {
        view.keepScreenOn = true
        onDispose { view.keepScreenOn = false }
    }
}

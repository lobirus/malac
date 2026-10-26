package io.malac.app.ui.workspaces

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material3.ExtendedFloatingActionButton
import androidx.compose.material3.FloatingActionButtonDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import io.malac.app.R
import io.malac.app.ui.components.EmptyState
import io.malac.app.ui.components.ScreenTopBar
import io.malac.app.ui.label
import io.malac.core.format.displayPath
import io.malac.core.format.formatCost
import io.malac.core.format.formatTokens
import io.malac.core.net.DaemonConnection
import io.malac.core.protocol.AgentStatus
import io.malac.core.state.AgentState
import io.malac.core.state.DaemonState

/** The agents of a tab, each with its live subagents below it. */
@Composable
fun TabScreen(
    state: DaemonState,
    status: DaemonConnection.Status,
    tab: String,
    onOpenAgent: (String) -> Unit,
    onNewAgent: (String) -> Unit,
    onBack: () -> Unit,
) {
    val view = state.tab(tab)
    Box(Modifier.fillMaxSize()) {
        Column(Modifier.fillMaxSize()) {
            ScreenTopBar(
                title = view?.name.orEmpty(),
                subtitle = view?.let { listOf(displayPath(it.cwd, state.daemon?.userHome), it.mode.label()).joinToString(" · ") },
                onBack = onBack,
            )
            val agents = view?.let(state::agentsOf).orEmpty()
            if (agents.isEmpty()) {
                EmptyState(stringResource(R.string.no_agents), stringResource(R.string.no_agents_body))
            } else {
                LazyColumn(contentPadding = PaddingValues(bottom = 88.dp)) {
                    for (agent in agents) agentTree(state, agent, depth = 0, onOpenAgent)
                }
            }
        }
        if (view != null && status == DaemonConnection.Status.Online) {
            ExtendedFloatingActionButton(
                onClick = { onNewAgent(view.id) },
                icon = { Icon(Icons.Filled.Add, contentDescription = null) },
                text = { Text(stringResource(R.string.new_agent)) },
                elevation = FloatingActionButtonDefaults.elevation(0.dp, 0.dp, 0.dp, 0.dp),
                containerColor = MaterialTheme.colorScheme.primary,
                contentColor = MaterialTheme.colorScheme.onPrimary,
                modifier = Modifier.align(Alignment.BottomEnd).padding(16.dp),
            )
        }
    }
}

private fun LazyListScope.agentTree(state: DaemonState, agent: AgentState, depth: Int, onOpen: (String) -> Unit) {
    item(key = agent.id) {
        StatusRow(
            phase = state.phase(agent),
            title = agent.displayName,
            detail = agentDetail(agent),
            onClick = { onOpen(agent.id) },
            indent = (depth * 20).dp,
        )
    }
    if (depth < 8) {
        for (child in state.childrenOf(agent.id)) agentTree(state, child, depth + 1, onOpen)
    }
}

/** What it does now while it runs, else its model and what it cost. */
private fun agentDetail(agent: AgentState): String {
    if (agent.live && agent.status == AgentStatus.Running && agent.activity.isNotEmpty()) return agent.activity
    val tokens = agent.usage.inputTokens + agent.usage.outputTokens
    return listOfNotNull(
        agent.model.ifEmpty { null },
        if (tokens > 0) "${formatTokens(tokens)} tok" else null,
        formatCost(agent.usage.cost).ifEmpty { null },
    ).joinToString(" · ")
}

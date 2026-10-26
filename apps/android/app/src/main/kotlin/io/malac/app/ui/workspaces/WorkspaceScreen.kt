package io.malac.app.ui.workspaces

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import io.malac.app.R
import io.malac.app.connection.ActiveDaemon
import io.malac.app.ui.components.ConnectionBanner
import io.malac.app.ui.components.EmptyState
import io.malac.app.ui.components.ScreenTopBar
import io.malac.app.ui.label
import io.malac.core.format.displayPath
import io.malac.core.net.DaemonConnection
import io.malac.core.protocol.Command
import io.malac.core.state.DaemonState

/** The tabs of one workspace. The connection switches to it without changing the daemon's last used one. */
@Composable
fun WorkspaceScreen(
    active: ActiveDaemon,
    state: DaemonState,
    status: DaemonConnection.Status,
    workspace: String,
    onOpenTab: (String) -> Unit,
    onBack: () -> Unit,
) {
    val view = state.workspace?.takeIf { it.id == workspace }
    LaunchedEffect(workspace, status) {
        if (view == null && status == DaemonConnection.Status.Online) active.connection.send(Command.Resync(workspace))
    }
    Column(Modifier.fillMaxSize()) {
        val summary = state.workspaces.firstOrNull { it.id == workspace }
        ScreenTopBar(title = view?.name ?: summary?.name.orEmpty(), onBack = onBack)
        ConnectionBanner(status, state.shutdownReason, active.connection::reconnectNow)
        when {
            view == null -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { CircularProgressIndicator() }
            view.tabs.isEmpty() -> EmptyState(stringResource(R.string.no_tabs))
            else -> LazyColumn {
                items(view.tabs, key = { it.id }) { tab ->
                    val agents = pluralStringResource(R.plurals.agents, tab.agents.size, tab.agents.size)
                    val detail = listOf(displayPath(tab.cwd, state.daemon?.userHome), agents, tab.mode.label()).joinToString(" · ")
                    StatusRow(state.phase(tab), tab.name, detail, onClick = { onOpenTab(tab.id) })
                }
            }
        }
    }
}

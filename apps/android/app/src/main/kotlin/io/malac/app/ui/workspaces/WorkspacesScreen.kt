package io.malac.app.ui.workspaces

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import io.malac.app.R
import io.malac.app.connection.ActiveDaemon
import io.malac.app.ui.components.ConnectionBanner
import io.malac.app.ui.components.EmptyState
import io.malac.app.ui.components.ScreenTopBar
import io.malac.core.net.DaemonConnection
import io.malac.core.state.DaemonState

@Composable
fun WorkspacesScreen(
    active: ActiveDaemon,
    state: DaemonState,
    status: DaemonConnection.Status,
    onOpenWorkspace: (String) -> Unit,
    onOpenDaemons: () -> Unit,
) {
    Column(Modifier.fillMaxSize()) {
        val daemon = state.daemon
        ScreenTopBar(
            title = active.daemon.name,
            subtitle = daemon?.let { stringResource(R.string.daemon_subtitle, it.version, pluralStringResource(R.plurals.clients, it.clients, it.clients)) },
            actions = {
                IconButton(onClick = onOpenDaemons) {
                    Icon(Icons.Filled.Settings, contentDescription = stringResource(R.string.daemons_title))
                }
            },
        )
        ConnectionBanner(status, state.shutdownReason, active.connection::reconnectNow)
        when {
            !state.loaded -> Unit
            state.workspaces.isEmpty() -> EmptyState(stringResource(R.string.no_workspaces), stringResource(R.string.no_workspaces_body))
            else -> LazyColumn {
                items(state.workspaces.sortedByDescending { it.updated }, key = { it.id }) { workspace ->
                    val tabs = pluralStringResource(R.plurals.tabs, workspace.tabs, workspace.tabs)
                    val detail = if (workspace.running > 0) {
                        stringResource(R.string.workspace_detail_running, tabs, workspace.running)
                    } else {
                        tabs
                    }
                    StatusRow(state.phase(workspace), workspace.name, detail, onClick = { onOpenWorkspace(workspace.id) })
                }
            }
        }
    }
}

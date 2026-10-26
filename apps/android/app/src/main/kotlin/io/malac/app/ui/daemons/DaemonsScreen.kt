package io.malac.app.ui.daemons

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Check
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.malac.app.AppGraph
import io.malac.app.R
import io.malac.app.connection.ActiveDaemon
import io.malac.app.data.SavedDaemon
import io.malac.app.ui.Commands
import io.malac.app.ui.components.ScreenTopBar
import io.malac.app.ui.theme.malac
import io.malac.core.state.DaemonState
import kotlinx.coroutines.launch

/** Saved daemons: switch, rename, forget, add. */
@Composable
fun DaemonsScreen(
    graph: AppGraph,
    active: ActiveDaemon,
    state: DaemonState,
    commands: Commands,
    onAdd: () -> Unit,
    onBack: () -> Unit,
) {
    val daemons by graph.daemons.daemons.collectAsStateWithLifecycle(initialValue = emptyList())
    val scope = rememberCoroutineScope()
    var renaming by remember { mutableStateOf<SavedDaemon?>(null) }
    var forgetting by remember { mutableStateOf<SavedDaemon?>(null) }

    Column(Modifier.fillMaxSize()) {
        ScreenTopBar(
            title = stringResource(R.string.daemons_title),
            onBack = onBack,
            actions = {
                IconButton(onClick = onAdd) { Icon(Icons.Filled.Add, contentDescription = stringResource(R.string.add_daemon)) }
            },
        )
        LazyColumn {
            if (state.loaded) {
                item(key = "details") { DaemonDetails(active.daemon.name, state, commands) }
            }
            items(daemons, key = { it.id }) { daemon ->
                val current = daemon.id == active.daemon.id
                DaemonRow(
                    daemon = daemon,
                    current = current,
                    onSelect = { scope.launch { graph.daemons.select(daemon.id) } },
                    onRename = { renaming = daemon },
                    onForget = { forgetting = daemon },
                )
            }
        }
    }

    renaming?.let { daemon ->
        var name by remember(daemon.id) { mutableStateOf(daemon.name) }
        AlertDialog(
            onDismissRequest = { renaming = null },
            title = { Text(stringResource(R.string.rename)) },
            text = { OutlinedTextField(value = name, onValueChange = { name = it }, singleLine = true) },
            confirmButton = {
                TextButton(onClick = {
                    renaming = null
                    scope.launch { graph.daemons.rename(daemon.id, name.trim().ifEmpty { daemon.name }) }
                }) { Text(stringResource(R.string.save)) }
            },
            dismissButton = { TextButton(onClick = { renaming = null }) { Text(stringResource(R.string.cancel)) } },
        )
    }

    forgetting?.let { daemon ->
        AlertDialog(
            onDismissRequest = { forgetting = null },
            title = { Text(stringResource(R.string.forget_title, daemon.name)) },
            text = { Text(stringResource(R.string.forget_body)) },
            confirmButton = {
                TextButton(onClick = {
                    forgetting = null
                    // App-scoped: forgetting the selected daemon leaves this screen.
                    graph.scope.launch { graph.forget(daemon) }
                }) { Text(stringResource(R.string.forget), color = MaterialTheme.malac.red) }
            },
            dismissButton = { TextButton(onClick = { forgetting = null }) { Text(stringResource(R.string.cancel)) } },
        )
    }
}

@Composable
private fun DaemonRow(daemon: SavedDaemon, current: Boolean, onSelect: () -> Unit, onRename: () -> Unit, onForget: () -> Unit) {
    var menu by remember { mutableStateOf(false) }
    Column {
        Row(
            Modifier
                .fillMaxWidth()
                .clickable(role = Role.RadioButton, onClick = onSelect)
                .semantics(mergeDescendants = true) { selected = current }
                .heightIn(min = 56.dp)
                .padding(start = 16.dp, end = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Box(Modifier.padding(end = 4.dp)) {
                if (current) Icon(Icons.Filled.Check, contentDescription = null, tint = MaterialTheme.malac.green)
            }
            Column(Modifier.weight(1f)) {
                Text(daemon.name, style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(daemon.url, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.malac.text2, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            Box {
                IconButton(onClick = { menu = true }) {
                    Icon(Icons.Filled.MoreVert, contentDescription = stringResource(R.string.daemon_actions, daemon.name))
                }
                DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                    DropdownMenuItem(text = { Text(stringResource(R.string.rename)) }, onClick = { menu = false; onRename() })
                    DropdownMenuItem(text = { Text(stringResource(R.string.forget)) }, onClick = { menu = false; onForget() })
                }
            }
        }
        HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
    }
}

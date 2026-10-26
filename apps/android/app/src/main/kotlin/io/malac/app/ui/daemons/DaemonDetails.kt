package io.malac.app.ui.daemons

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import io.malac.app.R
import io.malac.app.ui.Commands
import io.malac.app.ui.components.Panel
import io.malac.app.ui.theme.malac
import io.malac.core.protocol.Command
import io.malac.core.protocol.KeepAwake
import io.malac.core.state.DaemonState

/** The selected daemon: its version, how other machines reach it, and whether it keeps its machine awake. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun DaemonDetails(name: String, state: DaemonState, commands: Commands) {
    Panel(Modifier.padding(16.dp)) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text(name, style = MaterialTheme.typography.titleMedium, modifier = Modifier.semantics { heading() })
            state.daemon?.let { Detail(stringResource(R.string.daemon_version), it.version) }

            state.remote?.let { remote ->
                Detail(
                    stringResource(R.string.remote_access),
                    stringResource(if (remote.enabled) R.string.remote_on else R.string.remote_off, remote.listen),
                )
                remote.urls.firstOrNull()?.let { Detail(stringResource(R.string.remote_address), it.url) }
                remote.tunnel?.let { tunnel ->
                    val detail = listOfNotNull(tunnel.provider, tunnel.state, tunnel.url, tunnel.error).filter { it.isNotEmpty() }.joinToString(" · ")
                    Detail(stringResource(R.string.remote_tunnel), detail)
                }
                Detail(stringResource(R.string.remote_devices), pluralStringResource(R.plurals.devices, remote.devices, remote.devices))
            }

            state.awake?.let { awake ->
                Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    Text(stringResource(R.string.keep_awake), style = MaterialTheme.typography.labelLarge)
                    val modes = KeepAwake.entries
                    SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth()) {
                        modes.forEachIndexed { index, mode ->
                            SegmentedButton(
                                selected = awake.mode == mode,
                                onClick = { if (awake.mode != mode) commands.send(Command.SetKeepAwake(mode)) },
                                shape = SegmentedButtonDefaults.itemShape(index, modes.size),
                                label = { Text(stringResource(mode.label)) },
                            )
                        }
                    }
                    val status = stringResource(if (awake.awake) R.string.awake_held else R.string.awake_released)
                    Text(
                        listOf(status, awake.reason).filter { it.isNotEmpty() }.joinToString(" · "),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.malac.text2,
                    )
                    awake.error?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.malac.red) }
                }
            }
        }
    }
}

private val KeepAwake.label: Int
    get() = when (this) {
        KeepAwake.Busy -> R.string.keep_awake_busy
        KeepAwake.Always -> R.string.keep_awake_always
        KeepAwake.Off -> R.string.keep_awake_off
    }

@Composable
private fun Detail(label: String, value: String) {
    Column(Modifier.semantics(mergeDescendants = true) {}) {
        Text(label, style = MaterialTheme.typography.labelMedium, color = MaterialTheme.malac.text2)
        Text(value, style = MaterialTheme.typography.bodyMedium)
    }
}

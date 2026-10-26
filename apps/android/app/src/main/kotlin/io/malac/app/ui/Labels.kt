package io.malac.app.ui

import androidx.compose.runtime.Composable
import androidx.compose.ui.res.stringResource
import io.malac.app.R
import io.malac.core.protocol.Mode
import io.malac.core.state.Phase

@Composable
fun Mode.label(): String = stringResource(
    when (this) {
        Mode.Ask -> R.string.mode_ask
        Mode.Edits -> R.string.mode_edits
        Mode.Autopilot -> R.string.mode_autopilot
    },
)

@Composable
fun Phase.label(): String = stringResource(
    when (this) {
        Phase.AwaitingApproval -> R.string.phase_approval
        Phase.Running -> R.string.phase_running
        Phase.Done -> R.string.phase_done
        Phase.Failed -> R.string.phase_failed
        Phase.Stopped -> R.string.phase_stopped
        Phase.Idle -> R.string.phase_idle
    },
)

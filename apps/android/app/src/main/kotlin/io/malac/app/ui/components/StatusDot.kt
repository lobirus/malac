package io.malac.app.ui.components

import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.size
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.unit.dp
import io.malac.app.ui.theme.malac
import io.malac.core.state.Phase

/**
 * The status dot of the web UI: green and pulsing while running, amber when
 * an approval is needed, gray when done, red when failed, hollow when
 * stopped. It is decoration: the row it sits in states the status in words.
 */
@Composable
fun StatusDot(phase: Phase, modifier: Modifier = Modifier) {
    val colors = MaterialTheme.malac
    val color = when (phase) {
        Phase.Running -> colors.green
        Phase.AwaitingApproval -> colors.amber
        Phase.Done -> colors.text3
        Phase.Failed -> colors.red
        Phase.Stopped -> colors.text3
        Phase.Idle -> colors.dotIdle
    }
    val alpha = if (phase == Phase.Running) {
        val pulse by rememberInfiniteTransition(label = "pulse").animateFloat(
            initialValue = 1f,
            targetValue = 0.35f,
            animationSpec = infiniteRepeatable(tween(800), RepeatMode.Reverse),
            label = "pulse",
        )
        pulse
    } else {
        1f
    }
    Canvas(modifier.size(8.dp).clearAndSetSemantics {}) {
        if (phase == Phase.Stopped) {
            drawCircle(color, radius = size.minDimension / 2 - 0.75.dp.toPx(), style = Stroke(1.5.dp.toPx()))
        } else {
            drawCircle(color.copy(alpha = alpha * color.alpha))
        }
    }
}

/** The colour of a status written out, e.g. "Approval needed" in amber. */
@Composable
fun phaseColor(phase: Phase): Color = when (phase) {
    Phase.AwaitingApproval -> MaterialTheme.malac.amber
    Phase.Failed -> MaterialTheme.malac.red
    Phase.Running -> MaterialTheme.malac.green
    else -> MaterialTheme.malac.text2
}

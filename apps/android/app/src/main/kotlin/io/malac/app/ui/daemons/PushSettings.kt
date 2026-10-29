package io.malac.app.ui.daemons

import android.content.ActivityNotFoundException
import android.content.Intent
import android.net.Uri
import androidx.activity.compose.LocalActivity
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.LifecycleResumeEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import io.malac.app.R
import io.malac.app.notify.PushState
import io.malac.app.notify.UnifiedPushRegistrar
import io.malac.app.ui.components.Panel
import io.malac.app.ui.theme.malac

/** ntfy, the UnifiedPush distributor most people have, on Google Play. */
private const val NTFY = "https://play.google.com/store/apps/details?id=io.heckel.ntfy"

/**
 * Instant approvals: on through a UnifiedPush distributor, or how to get
 * there. This phone's own setting, for every saved daemon.
 */
@Composable
fun PushSettings(push: UnifiedPushRegistrar) {
    val state by push.state.collectAsStateWithLifecycle()
    val activity = LocalActivity.current
    // A distributor may have been installed or chosen while the app was away.
    LifecycleResumeEffect(push) {
        push.refresh()
        onPauseOrDispose {}
    }

    Panel(Modifier.padding(horizontal = 16.dp).padding(bottom = 16.dp)) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text(stringResource(R.string.push_title), style = MaterialTheme.typography.titleMedium, modifier = Modifier.semantics { heading() })
                    val status = when (val current = state) {
                        is PushState.On -> stringResource(R.string.push_on, current.distributor)
                        PushState.Off -> stringResource(R.string.push_off)
                        PushState.Unavailable -> stringResource(R.string.push_unavailable)
                    }
                    Text(status, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.malac.text2)
                }
                when (state) {
                    is PushState.On -> TextButton(onClick = push::turnOff) { Text(stringResource(R.string.push_turn_off)) }
                    PushState.Off -> TextButton(onClick = { activity?.let(push::turnOn) }, enabled = activity != null) {
                        Text(stringResource(R.string.push_turn_on))
                    }
                    PushState.Unavailable -> TextButton(onClick = {
                        try {
                            activity?.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(NTFY)))
                        } catch (_: ActivityNotFoundException) {
                            // No browser or store: the text says what to install.
                        }
                    }) { Text(stringResource(R.string.push_get_ntfy)) }
                }
            }
            val detail = when (state) {
                is PushState.On -> R.string.push_on_detail
                PushState.Off -> R.string.push_off_detail
                PushState.Unavailable -> R.string.push_unavailable_detail
            }
            Text(stringResource(detail), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.malac.text2)
        }
    }
}

package io.malac.app.ui.components

import android.Manifest
import android.content.Intent
import android.net.Uri
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.core.app.ActivityCompat
import io.malac.app.R
import io.malac.app.voice.canRecord

/**
 * Asks for the microphone the way Android recommends: right away the first
 * time, after a rationale when Android says one helps, and with the way to
 * Settings once it is denied for good.
 */
@Stable
class MicrophoneAccess internal constructor() {
    internal enum class Dialog { Rationale, Denied, Settings }

    internal var dialog by mutableStateOf<Dialog?>(null)
    internal var pending: (() -> Unit)? = null
    internal var ask: () -> Unit = {}
    internal var granted: () -> Boolean = { false }
    internal var rationale: () -> Boolean = { false }

    /** Runs [onGranted] now if the app may record, else once the user allows it. */
    fun request(onGranted: () -> Unit) {
        if (granted()) {
            onGranted()
            return
        }
        pending = onGranted
        if (rationale()) dialog = Dialog.Rationale else ask()
    }
}

/** A [MicrophoneAccess] for this screen; [MicrophoneAccessDialogs] shows what it asks. */
@Composable
fun rememberMicrophoneAccess(): MicrophoneAccess {
    val context = LocalContext.current
    val access = remember { MicrophoneAccess() }
    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { allowed ->
        val onGranted = access.pending
        access.pending = null
        when {
            allowed -> onGranted?.invoke()
            // Android no longer asks: only Settings can allow it now.
            !access.rationale() -> access.dialog = MicrophoneAccess.Dialog.Settings
            else -> access.dialog = MicrophoneAccess.Dialog.Denied
        }
    }
    access.ask = { launcher.launch(Manifest.permission.RECORD_AUDIO) }
    access.granted = { context.canRecord() }
    access.rationale = {
        context.findActivity()?.let { ActivityCompat.shouldShowRequestPermissionRationale(it, Manifest.permission.RECORD_AUDIO) } == true
    }
    return access
}

@Composable
fun MicrophoneAccessDialogs(access: MicrophoneAccess) {
    val context = LocalContext.current
    val dismiss = {
        access.dialog = null
        access.pending = null
    }
    when (access.dialog) {
        MicrophoneAccess.Dialog.Rationale -> AlertDialog(
            onDismissRequest = dismiss,
            title = { Text(stringResource(R.string.microphone_title)) },
            text = { Text(stringResource(R.string.microphone_rationale)) },
            confirmButton = {
                TextButton(onClick = {
                    access.dialog = null
                    access.ask()
                }) { Text(stringResource(R.string.continue_label)) }
            },
            dismissButton = { TextButton(onClick = dismiss) { Text(stringResource(R.string.not_now)) } },
        )
        MicrophoneAccess.Dialog.Denied -> AlertDialog(
            onDismissRequest = dismiss,
            title = { Text(stringResource(R.string.microphone_title)) },
            text = { Text(stringResource(R.string.microphone_denied)) },
            confirmButton = { TextButton(onClick = dismiss) { Text(stringResource(R.string.ok)) } },
        )
        MicrophoneAccess.Dialog.Settings -> AlertDialog(
            onDismissRequest = dismiss,
            title = { Text(stringResource(R.string.microphone_title)) },
            text = { Text(stringResource(R.string.microphone_settings)) },
            confirmButton = {
                TextButton(onClick = {
                    dismiss()
                    val settings = Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", context.packageName, null))
                    runCatching { context.startActivity(settings) }
                }) { Text(stringResource(R.string.open_settings)) }
            },
            dismissButton = { TextButton(onClick = dismiss) { Text(stringResource(R.string.cancel)) } },
        )
        null -> Unit
    }
}

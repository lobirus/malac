package io.malac.app.ui.daemons

import android.os.Build
import android.provider.Settings
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import androidx.lifecycle.viewmodel.compose.viewModel
import com.google.mlkit.vision.barcode.common.Barcode
import com.google.mlkit.vision.codescanner.GmsBarcodeScannerOptions
import com.google.mlkit.vision.codescanner.GmsBarcodeScanning
import io.malac.app.AppGraph
import io.malac.app.R
import io.malac.app.ui.components.ScreenTopBar
import io.malac.app.ui.theme.malac

/**
 * Pairs a daemon: `malac remote pair` on the computer shows a QR code and a
 * one-time code. Scanning fills the form; an address and a code or a token
 * can be typed too.
 */
@Composable
fun AddDaemonScreen(graph: AppGraph, onDone: () -> Unit, onBack: (() -> Unit)?) {
    val context = LocalContext.current
    val model = viewModel {
        val name = Settings.Global.getString(context.contentResolver, Settings.Global.DEVICE_NAME) ?: Build.MODEL
        AddDaemonViewModel(graph, name)
    }
    val invalidCode = stringResource(R.string.scan_not_malac)
    val fieldColors = OutlinedTextFieldDefaults.colors(focusedBorderColor = MaterialTheme.malac.green)

    Column(Modifier.fillMaxSize()) {
        ScreenTopBar(title = stringResource(R.string.add_daemon), onBack = onBack)
        Column(
            Modifier.fillMaxSize().verticalScroll(rememberScrollState()).imePadding().padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(stringResource(R.string.pair_intro), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.malac.text2)
            OutlinedButton(
                onClick = {
                    val options = GmsBarcodeScannerOptions.Builder().setBarcodeFormats(Barcode.FORMAT_QR_CODE).build()
                    GmsBarcodeScanning.getClient(context, options).startScan()
                        .addOnSuccessListener { barcode ->
                            if (!model.useLink(barcode.rawValue.orEmpty())) graph.connections.notice(invalidCode)
                        }
                },
                modifier = Modifier.fillMaxWidth(),
            ) {
                Icon(painterResource(R.drawable.ic_qr_code), contentDescription = null, modifier = Modifier.padding(end = 8.dp))
                Text(stringResource(R.string.scan_qr))
            }
            HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
            OutlinedTextField(
                value = model.address,
                onValueChange = { if (!(it.contains("#pair=") && model.useLink(it))) model.address = it },
                label = { Text(stringResource(R.string.address)) },
                placeholder = { Text("http://192.168.1.20:7700") },
                singleLine = true,
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, imeAction = ImeAction.Next),
                colors = fieldColors,
                modifier = Modifier.fillMaxWidth(),
            )
            OutlinedTextField(
                value = model.code,
                onValueChange = { model.code = it },
                label = { Text(stringResource(R.string.pairing_code)) },
                placeholder = { Text("ABCDE-FGHJK") },
                singleLine = true,
                keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Characters, autoCorrectEnabled = false, imeAction = ImeAction.Next),
                colors = fieldColors,
                modifier = Modifier.fillMaxWidth(),
            )
            OutlinedTextField(
                value = model.token,
                onValueChange = { model.token = it },
                label = { Text(stringResource(R.string.token_instead)) },
                singleLine = true,
                visualTransformation = PasswordVisualTransformation(),
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, autoCorrectEnabled = false, imeAction = ImeAction.Next),
                colors = fieldColors,
                modifier = Modifier.fillMaxWidth(),
            )
            OutlinedTextField(
                value = model.name,
                onValueChange = { model.name = it },
                label = { Text(stringResource(R.string.daemon_name)) },
                singleLine = true,
                colors = fieldColors,
                modifier = Modifier.fillMaxWidth(),
            )
            OutlinedTextField(
                value = model.deviceName,
                onValueChange = { model.deviceName = it },
                label = { Text(stringResource(R.string.device_name)) },
                supportingText = { Text(stringResource(R.string.device_name_help)) },
                singleLine = true,
                colors = fieldColors,
                modifier = Modifier.fillMaxWidth(),
            )
            model.error?.let {
                Text(
                    it,
                    color = MaterialTheme.malac.red,
                    style = MaterialTheme.typography.bodySmall,
                    modifier = Modifier.semantics { liveRegion = LiveRegionMode.Assertive },
                )
            }
            Button(onClick = { model.pair(onDone) }, enabled = model.canPair, modifier = Modifier.fillMaxWidth()) {
                Text(stringResource(if (model.busy) R.string.pairing else R.string.pair))
            }
        }
    }
}

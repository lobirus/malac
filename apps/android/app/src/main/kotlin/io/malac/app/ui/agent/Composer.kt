package io.malac.app.ui.agent

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material.icons.filled.ArrowDropDown
import androidx.compose.material.icons.filled.Close
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.IconButtonDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
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
import androidx.compose.ui.draw.clip
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.malac.app.R
import io.malac.app.ui.label
import io.malac.app.ui.theme.malac
import io.malac.core.protocol.Mode
import kotlinx.coroutines.launch

/**
 * Prompt, photos, model and mode. [model] and [mode] are what applies to
 * this agent; picking another sends the change right away.
 */
@Composable
fun Composer(
    viewModel: AgentViewModel,
    models: List<String>,
    model: String?,
    mode: Mode?,
    running: Boolean,
    enabled: Boolean,
    onModel: (String) -> Unit,
    onMode: (Mode) -> Unit,
    onSend: () -> Unit,
    onStop: () -> Unit,
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val picker = rememberLauncherForActivityResult(ActivityResultContracts.PickMultipleVisualMedia(maxItems = 6)) { uris ->
        scope.launch { viewModel.addImages(uris.mapNotNull { PickedImage.load(context, it) }) }
    }
    Column(Modifier.fillMaxWidth().navigationBarsPadding().imePadding()) {
        HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
        if (viewModel.images.isNotEmpty()) {
            LazyRow(
                Modifier.padding(horizontal = 12.dp, vertical = 8.dp),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                items(viewModel.images) { image ->
                    Box {
                        Image(
                            bitmap = image.preview,
                            contentDescription = image.name,
                            contentScale = ContentScale.Crop,
                            modifier = Modifier.size(64.dp).clip(MaterialTheme.shapes.small),
                        )
                        // Drawn small; Material still gives it a 48 dp touch target.
                        FilledIconButton(
                            onClick = { viewModel.removeImage(image) },
                            modifier = Modifier.align(Alignment.TopEnd).size(24.dp),
                            colors = IconButtonDefaults.filledIconButtonColors(
                                containerColor = MaterialTheme.colorScheme.primary,
                                contentColor = MaterialTheme.colorScheme.onPrimary,
                            ),
                        ) {
                            Icon(Icons.Filled.Close, contentDescription = stringResource(R.string.remove_image, image.name), modifier = Modifier.size(16.dp))
                        }
                    }
                }
            }
        }
        Row(Modifier.padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            Picker(
                label = stringResource(R.string.model),
                value = model ?: stringResource(R.string.default_model),
                options = models,
                display = { it },
                onPick = onModel,
                enabled = enabled && models.isNotEmpty(),
            )
            if (mode != null) {
                Picker(
                    label = stringResource(R.string.mode),
                    value = mode.label(),
                    options = Mode.entries,
                    display = { it.label() },
                    onPick = onMode,
                    enabled = enabled,
                )
            }
        }
        Row(
            Modifier.padding(start = 4.dp, end = 8.dp, bottom = 8.dp),
            verticalAlignment = Alignment.Bottom,
            horizontalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            IconButton(
                onClick = { picker.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly)) },
                enabled = enabled,
            ) {
                Icon(painterResource(R.drawable.ic_image), contentDescription = stringResource(R.string.attach_photos))
            }
            OutlinedTextField(
                value = viewModel.draftText,
                onValueChange = { viewModel.draftText = it },
                placeholder = { Text(stringResource(if (running) R.string.prompt_running else R.string.prompt_placeholder)) },
                maxLines = 6,
                enabled = enabled,
                shape = MaterialTheme.shapes.large,
                colors = OutlinedTextFieldDefaults.colors(
                    focusedBorderColor = MaterialTheme.malac.green,
                    unfocusedBorderColor = MaterialTheme.colorScheme.outline,
                    focusedContainerColor = MaterialTheme.colorScheme.surfaceContainerLow,
                    unfocusedContainerColor = MaterialTheme.colorScheme.surfaceContainerLow,
                ),
                modifier = Modifier.weight(1f),
            )
            if (running) {
                IconButton(onClick = onStop) {
                    Icon(painterResource(R.drawable.ic_stop), contentDescription = stringResource(R.string.stop), tint = MaterialTheme.malac.red)
                }
            }
            val canSend = enabled && !viewModel.sending && (viewModel.draftText.isNotBlank() || viewModel.images.isNotEmpty())
            FilledIconButton(
                onClick = onSend,
                enabled = canSend,
                colors = IconButtonDefaults.filledIconButtonColors(
                    containerColor = MaterialTheme.colorScheme.primary,
                    contentColor = MaterialTheme.colorScheme.onPrimary,
                ),
            ) {
                Icon(Icons.AutoMirrored.Filled.Send, contentDescription = stringResource(R.string.send))
            }
        }
    }
}

@Composable
private fun <T> Picker(
    label: String,
    value: String,
    options: List<T>,
    display: @Composable (T) -> String,
    onPick: (T) -> Unit,
    enabled: Boolean,
) {
    var open by remember { mutableStateOf(false) }
    Box {
        TextButton(onClick = { open = true }, enabled = enabled) {
            Text(
                "$label: $value",
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.malac.text2,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Icon(Icons.Filled.ArrowDropDown, contentDescription = null, tint = MaterialTheme.malac.text3)
        }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            for (option in options) {
                DropdownMenuItem(
                    text = { Text(display(option)) },
                    onClick = {
                        open = false
                        onPick(option)
                    },
                )
            }
        }
    }
}

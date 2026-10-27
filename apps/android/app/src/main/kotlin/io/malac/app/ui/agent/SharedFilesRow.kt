package io.malac.app.ui.agent

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Share
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.malac.app.R
import io.malac.app.files.Files
import io.malac.app.files.SharedFileStore
import io.malac.app.ui.components.LocalImageViewer
import io.malac.app.ui.components.ViewerImage
import io.malac.app.ui.components.decodeImage
import io.malac.app.ui.theme.malac
import io.malac.core.format.formatBytes
import io.malac.core.net.DaemonException
import io.malac.core.transcript.SharedFile
import io.malac.core.transcript.SharedFiles
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File

/** How tall an image malac sent is shown inline; a fixed box, so loading it moves nothing. */
private val PREVIEW_HEIGHT = 240.dp

/**
 * What a `send_to_user` call sent, shown as the files themselves: its
 * caption, images inline that open full screen, and other files as cards
 * that open in another app.
 */
@Composable
fun SharedFilesRow(shared: SharedFiles, store: SharedFileStore, onReport: (String) -> Unit) {
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        shared.caption?.let { caption ->
            SelectionContainer { Text(caption, style = MaterialTheme.typography.bodyMedium) }
        }
        for (file in shared.files) {
            if (file.isImage) SharedImage(file, store, onReport) else SharedFileCard(file, store, onReport)
        }
    }
}

private sealed interface Preview {
    data object Loading : Preview

    class Ready(val bitmap: ImageBitmap) : Preview

    class Failed(val message: String) : Preview
}

@Composable
private fun SharedImage(file: SharedFile, store: SharedFileStore, onReport: (String) -> Unit) {
    val viewer = LocalImageViewer.current
    val height = with(LocalDensity.current) { PREVIEW_HEIGHT.roundToPx() }
    val notAnImage = stringResource(R.string.image_failed)
    val preview by produceState<Preview>(Preview.Loading, file.id) {
        value = try {
            val bytes = withContext(Dispatchers.IO) { store.bytes(file) }
            // About as tall as it is shown, however large the screenshot.
            val bitmap = withContext(Dispatchers.Default) { decodeImage(bytes, height * 2) }
            if (bitmap != null) Preview.Ready(bitmap.asImageBitmap()) else Preview.Failed(notAnImage)
        } catch (error: CancellationException) {
            throw error
        } catch (error: DaemonException) {
            Preview.Failed(error.message.orEmpty())
        }
    }
    when (val state = preview) {
        is Preview.Failed -> Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
            SharedFileCard(file, store, onReport)
            Text(stringResource(R.string.image_not_loaded, state.message), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.malac.red)
        }
        else -> Box(Modifier.fillMaxWidth().height(PREVIEW_HEIGHT), contentAlignment = Alignment.CenterStart) {
            val bitmap = (state as? Preview.Ready)?.bitmap
            if (bitmap == null) {
                Box(
                    Modifier
                        .fillMaxSize()
                        .clip(MaterialTheme.shapes.small)
                        .background(MaterialTheme.colorScheme.surfaceContainer)
                        .semantics { contentDescription = file.name },
                )
            } else {
                Image(
                    bitmap = bitmap,
                    contentDescription = file.name,
                    contentScale = ContentScale.Fit,
                    modifier = Modifier
                        // As tall as the box, or as wide and less tall when the image is wide; never stretched.
                        .aspectRatio(bitmap.width / bitmap.height.toFloat(), matchHeightConstraintsFirst = true)
                        .clip(MaterialTheme.shapes.small)
                        .clickable(onClickLabel = stringResource(R.string.view_full_screen), role = Role.Image) {
                            viewer?.open(ViewerImage(file.id, file.name, file.type) { store.bytes(file) })
                        },
                )
            }
        }
    }
}

/** A file malac sent: tap to open it in another app, or share it. */
@Composable
private fun SharedFileCard(file: SharedFile, store: SharedFileStore, onReport: (String) -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var busy by remember { mutableStateOf(false) }
    val noApp = stringResource(R.string.no_app_to_open, file.name)
    val noShare = stringResource(R.string.no_app_to_share)
    val failed = stringResource(R.string.download_failed, file.name)

    fun withFile(use: (File) -> Boolean, otherwise: String) {
        if (busy) return
        busy = true
        scope.launch {
            try {
                if (!use(store.download(file))) onReport(otherwise)
            } catch (error: DaemonException) {
                onReport("$failed ${error.message.orEmpty()}".trim())
            } finally {
                busy = false
            }
        }
    }

    val kind = file.name.substringAfterLast('.', "").uppercase().takeIf { it.isNotEmpty() && it.length <= 5 }
    Surface(
        shape = MaterialTheme.shapes.small,
        color = MaterialTheme.colorScheme.surfaceContainerLow,
        border = BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Row(Modifier.padding(end = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            Row(
                Modifier
                    .weight(1f)
                    .clickable(enabled = !busy, onClickLabel = stringResource(R.string.open_file), role = Role.Button) {
                        withFile({ Files.open(context, it, file.type) }, noApp)
                    }
                    .heightIn(min = 56.dp)
                    .padding(horizontal = 12.dp, vertical = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Box(Modifier.size(24.dp), contentAlignment = Alignment.Center) {
                    if (busy) {
                        CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 1.5.dp)
                    } else {
                        Icon(painterResource(R.drawable.ic_file), contentDescription = null, tint = MaterialTheme.malac.text2)
                    }
                }
                Column(Modifier.weight(1f)) {
                    Text(file.name, style = MaterialTheme.typography.bodyMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Text(
                        listOfNotNull(kind, formatBytes(file.size)).joinToString(" · "),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.malac.text2,
                    )
                }
            }
            IconButton(onClick = { withFile({ Files.share(context, it, file.type) }, noShare) }, enabled = !busy) {
                Icon(Icons.Filled.Share, contentDescription = stringResource(R.string.share_file, file.name))
            }
        }
    }
}

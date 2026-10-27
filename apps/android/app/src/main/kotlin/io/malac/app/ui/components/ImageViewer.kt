package io.malac.app.ui.components

import android.app.Activity
import android.content.Context
import android.content.ContextWrapper
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.os.Build
import android.util.Base64
import android.widget.Toast
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.animate
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.calculateCentroid
import androidx.compose.foundation.gestures.calculateCentroidSize
import androidx.compose.foundation.gestures.calculatePan
import androidx.compose.foundation.gestures.calculateZoom
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.Share
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.IconButtonDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.positionChanged
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.paneTitle
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import androidx.core.view.WindowCompat
import io.malac.app.R
import io.malac.app.files.Files
import io.malac.core.protocol.Block
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlin.math.abs
import kotlin.math.max
import kotlin.math.min

/** An image to show full screen. [load] reads its bytes, from memory or from the daemon; [key] names its copy for sharing. */
class ViewerImage(val key: String, val name: String, val mediaType: String, val load: suspend () -> ByteArray)

/** An image of the transcript, kept as base64. */
fun Block.Image.toViewerImage(name: String): ViewerImage {
    val extension = mediaType.substringAfter("image/", "png").substringBefore('+').ifEmpty { "png" }
    val data = data
    return ViewerImage("image-${data.length}-${data.hashCode()}", "$name.$extension", mediaType.ifEmpty { "image/png" }) {
        Base64.decode(data, Base64.DEFAULT)
    }
}

/** Which image is open full screen, if any. */
@Stable
class ImageViewerState {
    var image by mutableStateOf<ViewerImage?>(null)
        private set

    fun open(image: ViewerImage) {
        this.image = image
    }

    fun close() {
        image = null
    }
}

/** The viewer images open in; provided at the root, above every screen. */
val LocalImageViewer = staticCompositionLocalOf<ImageViewerState?> { null }

/** Shows the open image over everything, edge to edge. */
@Composable
fun ImageViewerHost(state: ImageViewerState) {
    val image = state.image ?: return
    key(image.key) {
        ImageViewer(image, onClose = state::close)
    }
}

private const val MAX_SCALE = 6f
private const val DOUBLE_TAP_SCALE = 2.5f

/** The longest edge decoded: sharp at 2.5x on a phone, and small enough to draw. */
private const val MAX_EDGE = 4096

private sealed interface Loaded {
    class Image(val bytes: ByteArray, val bitmap: ImageBitmap) : Loaded

    class Failed(val message: String) : Loaded
}

/**
 * A full-screen image: pinch to zoom around the fingers, drag to pan within
 * the image, double-tap to switch between fitting and 2.5x, swipe down or go
 * back to close. Share and save are in the bar, which a tap hides.
 */
@Composable
private fun ImageViewer(image: ViewerImage, onClose: () -> Unit) {
    val scope = rememberCoroutineScope()
    BackHandler(onBack = onClose)
    LightSystemBarIcons()

    val failedText = stringResource(R.string.image_failed)
    val loaded by produceState<Loaded?>(null, image) {
        value = try {
            val bytes = withContext(Dispatchers.IO) { image.load() }
            val bitmap = withContext(Dispatchers.Default) { decodeImage(bytes, MAX_EDGE) }
            if (bitmap != null) Loaded.Image(bytes, bitmap.asImageBitmap()) else Loaded.Failed(failedText)
        } catch (error: CancellationException) {
            throw error
        } catch (error: Exception) {
            Loaded.Failed(error.message ?: failedText)
        } catch (error: OutOfMemoryError) {
            Loaded.Failed(failedText)
        }
    }

    var scale by remember { mutableFloatStateOf(1f) }
    var offset by remember { mutableStateOf(Offset.Zero) }
    var dismiss by remember { mutableFloatStateOf(0f) }
    var container by remember { mutableStateOf(IntSize.Zero) }
    var chrome by remember { mutableStateOf(true) }

    val bitmap = (loaded as? Loaded.Image)?.bitmap
    val fitted = remember(bitmap, container) { fittedSize(bitmap, container) }

    fun clamp(target: Offset, at: Float): Offset {
        val maxX = max(0f, (fitted.width * at - container.width) / 2)
        val maxY = max(0f, (fitted.height * at - container.height) / 2)
        return Offset(target.x.coerceIn(-maxX, maxX), target.y.coerceIn(-maxY, maxY))
    }

    // Zooms by [zoom] keeping the point under [centroid] (from the centre) still.
    fun transform(centroid: Offset, pan: Offset, zoom: Float) {
        val next = (scale * zoom).coerceIn(1f, MAX_SCALE)
        val factor = next / scale
        offset = clamp((offset - centroid) * factor + centroid + pan, next)
        scale = next
    }

    fun animateTo(targetScale: Float, targetOffset: Offset) {
        val fromScale = scale
        val fromOffset = offset
        scope.launch {
            animate(0f, 1f, animationSpec = tween(220)) { fraction, _ ->
                scale = fromScale + (targetScale - fromScale) * fraction
                offset = fromOffset + (targetOffset - fromOffset) * fraction
            }
        }
    }

    fun toggleZoom(at: Offset) {
        if (scale > 1.05f) {
            animateTo(1f, Offset.Zero)
        } else {
            animateTo(DOUBLE_TAP_SCALE, clamp((offset - at) * (DOUBLE_TAP_SCALE / scale) + at, DOUBLE_TAP_SCALE))
        }
    }

    val center = Offset(container.width / 2f, container.height / 2f)
    // Read here only when they flip, so pinching and dragging redraw without recomposing.
    val zoomed by remember { derivedStateOf { scale > 1.05f } }
    val dragged by remember { derivedStateOf { dismiss != 0f } }
    val zoomLabel = stringResource(if (zoomed) R.string.zoom_out else R.string.zoom_in)

    Box(
        Modifier
            .fillMaxSize()
            // The page shows through as the image is dragged away.
            .drawBehind {
                val fade = if (size.height > 0) min(1f, abs(dismiss) / (size.height / 2f)) else 0f
                drawRect(Color.Black, alpha = 1f - fade * 0.6f)
            }
            .onSizeChanged { container = it }
            .semantics { paneTitle = image.name },
    ) {
        when (val state = loaded) {
            null -> CircularProgressIndicator(color = Color.White, modifier = Modifier.align(Alignment.Center))
            is Loaded.Failed -> Text(
                state.message,
                color = Color.White,
                style = MaterialTheme.typography.bodyMedium,
                textAlign = TextAlign.Center,
                modifier = Modifier.align(Alignment.Center).padding(32.dp),
            )
            is Loaded.Image -> Box(
                Modifier
                    .fillMaxSize()
                    .pointerInput(bitmap, container) {
                        detectTapGestures(
                            onTap = { chrome = !chrome },
                            onDoubleTap = { toggleZoom(it - center) },
                        )
                    }
                    // After the taps in the chain, so it sees moves first and consumes them.
                    .pointerInput(bitmap, container) {
                        awaitEachGesture {
                            awaitFirstDown(requireUnconsumed = false)
                            // A one-finger drag at fitting size closes the viewer.
                            var dismissing = scale <= 1.01f
                            var moving = false
                            var zoomTotal = 1f
                            var panTotal = Offset.Zero
                            do {
                                val event = awaitPointerEvent()
                                if (event.changes.any { it.isConsumed }) break
                                val zoom = event.calculateZoom()
                                val pan = event.calculatePan()
                                if (!moving) {
                                    zoomTotal *= zoom
                                    panTotal += pan
                                    val spread = abs(1 - zoomTotal) * event.calculateCentroidSize(useCurrent = false)
                                    moving = spread > viewConfiguration.touchSlop || panTotal.getDistance() > viewConfiguration.touchSlop
                                }
                                if (moving) {
                                    if (dismissing && event.changes.count { it.pressed } > 1) {
                                        dismissing = false
                                        dismiss = 0f
                                    }
                                    if (dismissing) {
                                        dismiss += pan.y
                                    } else {
                                        transform(event.calculateCentroid(useCurrent = false) - center, pan, zoom)
                                    }
                                    event.changes.forEach { if (it.positionChanged()) it.consume() }
                                }
                            } while (event.changes.any { it.pressed })
                            if (dismissing && moving) {
                                if (abs(dismiss) > container.height / 6f) {
                                    onClose()
                                } else {
                                    val from = dismiss
                                    scope.launch { animate(from, 0f, animationSpec = tween(180)) { value, _ -> dismiss = value } }
                                }
                            }
                        }
                    }
                    .semantics {
                        contentDescription = image.name
                        customActions = listOf(
                            CustomAccessibilityAction(zoomLabel) {
                                toggleZoom(Offset.Zero)
                                true
                            },
                        )
                    },
            ) {
                Image(
                    bitmap = state.bitmap,
                    contentDescription = null,
                    contentScale = ContentScale.Fit,
                    modifier = Modifier
                        .fillMaxSize()
                        .graphicsLayer {
                            scaleX = scale
                            scaleY = scale
                            translationX = offset.x
                            translationY = offset.y + dismiss
                        },
                )
            }
        }

        AnimatedVisibility(chrome && !dragged, enter = fadeIn(), exit = fadeOut(), modifier = Modifier.align(Alignment.TopCenter)) {
            ViewerBar(image, (loaded as? Loaded.Image)?.bytes, onClose)
        }
    }
}

@Composable
private fun ViewerBar(image: ViewerImage, bytes: ByteArray?, onClose: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val saved = stringResource(R.string.image_saved)
    val notSaved = stringResource(R.string.image_not_saved)
    val noApp = stringResource(R.string.no_app_to_share)
    val createDocument = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument(image.mediaType)) { uri ->
        if (uri != null && bytes != null) {
            scope.launch {
                val done = Files.saveTo(context, uri, bytes)
                Toast.makeText(context, if (done) saved else notSaved, Toast.LENGTH_SHORT).show()
            }
        }
    }
    val colors = IconButtonDefaults.iconButtonColors(contentColor = Color.White, disabledContentColor = Color.White.copy(alpha = 0.4f))
    Row(
        Modifier
            .fillMaxWidth()
            .background(Color.Black.copy(alpha = 0.6f))
            // The bar's background reaches under the status bar and the camera cutout; its buttons do not.
            .windowInsetsPadding(WindowInsets.safeDrawing.only(WindowInsetsSides.Top + WindowInsetsSides.Horizontal))
            .height(56.dp)
            .padding(horizontal = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        IconButton(onClick = onClose, colors = colors) {
            Icon(Icons.Filled.Close, contentDescription = stringResource(R.string.close))
        }
        Text(
            image.name,
            color = Color.White,
            style = MaterialTheme.typography.titleSmall,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f).padding(horizontal = 4.dp),
        )
        IconButton(
            onClick = {
                val data = bytes ?: return@IconButton
                scope.launch {
                    val file = Files.cache(context, image.key, image.name, data)
                    if (!Files.share(context, file, image.mediaType)) Toast.makeText(context, noApp, Toast.LENGTH_SHORT).show()
                }
            },
            enabled = bytes != null,
            colors = colors,
        ) {
            Icon(Icons.Filled.Share, contentDescription = stringResource(R.string.share))
        }
        IconButton(
            onClick = {
                val data = bytes ?: return@IconButton
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                    scope.launch {
                        val done = Files.saveImage(context, image.name, image.mediaType, data)
                        Toast.makeText(context, if (done) saved else notSaved, Toast.LENGTH_SHORT).show()
                    }
                } else {
                    createDocument.launch(image.name)
                }
            },
            enabled = bytes != null,
            colors = colors,
        ) {
            Icon(painterResource(R.drawable.ic_download), contentDescription = stringResource(R.string.save_to_pictures))
        }
    }
}

/** Light status and navigation bar icons over the black viewer, restored when it closes. */
@Composable
private fun LightSystemBarIcons() {
    val view = LocalView.current
    DisposableEffect(view) {
        val window = view.context.findActivity()?.window ?: return@DisposableEffect onDispose {}
        val controller = WindowCompat.getInsetsController(window, view)
        val status = controller.isAppearanceLightStatusBars
        val navigation = controller.isAppearanceLightNavigationBars
        controller.isAppearanceLightStatusBars = false
        controller.isAppearanceLightNavigationBars = false
        onDispose {
            controller.isAppearanceLightStatusBars = status
            controller.isAppearanceLightNavigationBars = navigation
        }
    }
}

fun Context.findActivity(): Activity? {
    var context: Context? = this
    while (context is ContextWrapper) {
        if (context is Activity) return context
        context = context.baseContext
    }
    return null
}

/** The image's size when it fits [container], in pixels. */
private fun fittedSize(bitmap: ImageBitmap?, container: IntSize): Size {
    if (bitmap == null || bitmap.width == 0 || bitmap.height == 0 || container.width == 0) return Size.Zero
    val fit = min(container.width / bitmap.width.toFloat(), container.height / bitmap.height.toFloat())
    return Size(bitmap.width * fit, bitmap.height * fit)
}

/** Decodes [bytes] with its longer side at most [maxEdge] pixels, or null when it is not an image. */
fun decodeImage(bytes: ByteArray, maxEdge: Int): Bitmap? {
    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
    val longEdge = max(bounds.outWidth, bounds.outHeight)
    if (longEdge <= 0) return null
    var sample = 1
    while (longEdge / sample > maxEdge) sample *= 2
    return BitmapFactory.decodeByteArray(bytes, 0, bytes.size, BitmapFactory.Options().apply { inSampleSize = sample })
}

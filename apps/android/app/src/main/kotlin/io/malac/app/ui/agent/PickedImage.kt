package io.malac.app.ui.agent

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.net.Uri
import android.provider.OpenableColumns
import android.util.Base64
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import io.malac.app.ui.components.ViewerImage
import io.malac.core.protocol.Block
import io.malac.core.protocol.ImageInput
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.ByteArrayOutputStream
import kotlin.math.max
import kotlin.math.roundToInt

/** A photo or screenshot ready to send: a JPEG at most [MAX_EDGE] pixels on its long side. */
class PickedImage(val name: String, val input: ImageInput, val preview: ImageBitmap) {
    val block: Block.Image get() = Block.Image(input.mediaType, input.data)

    /** The JPEG as it will be sent, for the full-screen viewer. */
    val viewerImage: ViewerImage
        get() {
            val data = input.data
            return ViewerImage("picked-${data.length}-${data.hashCode()}", name, input.mediaType) { Base64.decode(data, Base64.DEFAULT) }
        }

    companion object {
        private const val MAX_EDGE = 2048
        private const val QUALITY = 85

        suspend fun load(context: Context, uri: Uri): PickedImage? = withContext(Dispatchers.IO) {
            val resolver = context.contentResolver
            val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
            resolver.openInputStream(uri)?.use { BitmapFactory.decodeStream(it, null, bounds) }
            val longEdge = max(bounds.outWidth, bounds.outHeight)
            if (longEdge <= 0) return@withContext null
            var sample = 1
            while (longEdge / (sample * 2) >= MAX_EDGE) sample *= 2
            val decoded = resolver.openInputStream(uri)?.use {
                BitmapFactory.decodeStream(it, null, BitmapFactory.Options().apply { inSampleSize = sample })
            } ?: return@withContext null
            val scale = MAX_EDGE.toFloat() / max(decoded.width, decoded.height)
            val bitmap = if (scale < 1f) {
                Bitmap.createScaledBitmap(decoded, (decoded.width * scale).roundToInt(), (decoded.height * scale).roundToInt(), true)
            } else {
                decoded
            }
            val jpeg = ByteArrayOutputStream().use { out ->
                bitmap.compress(Bitmap.CompressFormat.JPEG, QUALITY, out)
                out.toByteArray()
            }
            val name = displayName(context, uri)?.substringBeforeLast('.')?.plus(".jpg") ?: "photo.jpg"
            val data = Base64.encodeToString(jpeg, Base64.NO_WRAP)
            PickedImage(name, ImageInput("image/jpeg", data, name), bitmap.asImageBitmap())
        }

        private fun displayName(context: Context, uri: Uri): String? =
            context.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { cursor ->
                if (cursor.moveToFirst()) cursor.getString(0) else null
            }
    }
}

package io.malac.app.files

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.ClipData
import android.content.ContentValues
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.provider.MediaStore
import android.util.LruCache
import android.webkit.MimeTypeMap
import androidx.annotation.RequiresApi
import androidx.core.content.FileProvider
import io.malac.core.net.DaemonApi
import io.malac.core.transcript.SharedFile
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.File
import java.io.IOException

/**
 * The files of one daemon's `send_to_user` calls: images in memory, for
 * the transcript and the viewer, and other files in the cache, where the
 * FileProvider hands them to the app that opens them.
 */
class SharedFileStore(private val context: Context, private val api: DaemonApi) {
    private val images = object : LruCache<String, ByteArray>(MEMORY_BYTES) {
        override fun sizeOf(key: String, value: ByteArray) = value.size
    }

    suspend fun bytes(file: SharedFile): ByteArray {
        images.get(file.id)?.let { return it }
        return api.shared(file.id).bytes.also { images.put(file.id, it) }
    }

    /** The file in the cache, downloaded first unless it is there already. */
    suspend fun download(file: SharedFile): File {
        val target = Files.cacheFile(context, file.id, file.name)
        if (!target.exists()) api.saveShared(file.id, target)
        return target
    }

    private companion object {
        const val MEMORY_BYTES = 24 * 1024 * 1024
    }
}

/** Handing files to other apps: open, share, save to Pictures. */
object Files {
    private fun authority(context: Context) = "${context.packageName}.files"

    /** Where [name] is kept for [key], in the cache folder the FileProvider serves. */
    fun cacheFile(context: Context, key: String, name: String): File =
        File(File(File(context.cacheDir, "shared"), safe(key)), safe(name))

    /** Writes [bytes] to the cache, once, e.g. to share an image of the transcript. */
    suspend fun cache(context: Context, key: String, name: String, bytes: ByteArray): File = withContext(Dispatchers.IO) {
        cacheFile(context, key, name).also { file ->
            if (!file.exists()) {
                file.parentFile?.mkdirs()
                file.writeBytes(bytes)
            }
        }
    }

    /** Opens [file] with the app the user picks; false when nothing could be started. */
    fun open(context: Context, file: File, type: String): Boolean {
        val uri = uri(context, file)
        val view = Intent(Intent.ACTION_VIEW)
            .setDataAndType(uri, viewType(type, file.name))
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        return start(context, Intent.createChooser(view, null))
    }

    fun share(context: Context, file: File, type: String): Boolean {
        val uri = uri(context, file)
        val send = Intent(Intent.ACTION_SEND)
            .setType(viewType(type, file.name))
            .putExtra(Intent.EXTRA_STREAM, uri)
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        // The chooser passes the read grant on only for a URI in the clip data.
        send.clipData = ClipData.newRawUri(file.name, uri)
        return start(context, Intent.createChooser(send, null))
    }

    /**
     * Saves an image to Pictures/malac through MediaStore, which needs no
     * permission from Android 10. Older versions save with the system's
     * file picker instead; see [saveTo].
     */
    suspend fun saveImage(context: Context, name: String, type: String, bytes: ByteArray): Boolean {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return false
        return withContext(Dispatchers.IO) { insertImage(context, name, type, bytes) }
    }

    @RequiresApi(Build.VERSION_CODES.Q)
    private fun insertImage(context: Context, name: String, type: String, bytes: ByteArray): Boolean {
        val resolver = context.contentResolver
        val values = ContentValues().apply {
            put(MediaStore.MediaColumns.DISPLAY_NAME, name)
            put(MediaStore.MediaColumns.MIME_TYPE, type)
            put(MediaStore.MediaColumns.RELATIVE_PATH, "${Environment.DIRECTORY_PICTURES}/malac")
            put(MediaStore.MediaColumns.IS_PENDING, 1)
        }
        val uri = resolver.insert(MediaStore.Images.Media.EXTERNAL_CONTENT_URI, values) ?: return false
        return try {
            resolver.openOutputStream(uri)?.use { it.write(bytes) } ?: throw IOException("no stream")
            resolver.update(uri, ContentValues().apply { put(MediaStore.MediaColumns.IS_PENDING, 0) }, null, null)
            true
        } catch (error: IOException) {
            resolver.delete(uri, null, null)
            false
        }
    }

    /** Writes [bytes] to a document the user created with the system's file picker. */
    suspend fun saveTo(context: Context, uri: Uri, bytes: ByteArray): Boolean = withContext(Dispatchers.IO) {
        try {
            context.contentResolver.openOutputStream(uri)?.use { it.write(bytes) } != null
        } catch (error: IOException) {
            false
        }
    }

    private fun uri(context: Context, file: File): Uri = FileProvider.getUriForFile(context, authority(context), file)

    private fun start(context: Context, intent: Intent): Boolean = try {
        if (context !is Activity) intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        context.startActivity(intent)
        true
    } catch (error: ActivityNotFoundException) {
        false
    }

    /** A generic type tells other apps nothing; the extension may. */
    private fun viewType(type: String, name: String): String {
        if (type.isNotEmpty() && type != "application/octet-stream") return type
        val extension = name.substringAfterLast('.', "").lowercase()
        return MimeTypeMap.getSingleton().getMimeTypeFromExtension(extension) ?: "*/*"
    }

    private fun safe(name: String): String =
        buildString { for (char in name) append(if (char == '/' || char == '\\' || char == ':' || char.isISOControl()) '_' else char) }
            .trimStart('.')
            .ifEmpty { "file" }
}

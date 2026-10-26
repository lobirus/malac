package io.malac.app

import android.content.Context
import android.os.Build
import java.io.File

/**
 * Keeps the stack trace of a crash for the next launch, which offers it to
 * copy into a bug report. The app sends nothing anywhere by itself.
 */
class CrashLog(private val context: Context) {
    private val file = File(context.filesDir, "last-crash.txt")

    /** Records uncaught exceptions, then lets the system handle them as before. */
    fun install() {
        val previous = Thread.getDefaultUncaughtExceptionHandler()
        Thread.setDefaultUncaughtExceptionHandler { thread, error ->
            runCatching { file.writeText(report(thread, error)) }
            previous?.uncaughtException(thread, error)
        }
    }

    fun read(): String? = file.takeIf { it.exists() }?.let { runCatching { it.readText() }.getOrNull() }

    fun clear() {
        file.delete()
    }

    private fun report(thread: Thread, error: Throwable): String {
        val version = runCatching { context.packageManager.getPackageInfo(context.packageName, 0).versionName }.getOrNull()
        return buildString {
            appendLine("malac ${version.orEmpty()} on Android ${Build.VERSION.RELEASE} (${Build.MANUFACTURER} ${Build.MODEL})")
            appendLine("thread: ${thread.name}")
            append(error.stackTraceToString())
        }
    }
}

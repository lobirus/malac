package io.malac.core.format

import io.malac.core.protocol.Usage
import java.time.Duration
import java.time.Instant
import java.util.Locale

fun truncate(text: String, max: Int): String = if (text.length > max) text.take(max - 1) + "…" else text

fun firstLine(text: String?): String = text.orEmpty().trim().substringBefore('\n')

fun baseName(path: String): String = path.split('/').lastOrNull { it.isNotEmpty() } ?: path

fun plural(count: Int, word: String, many: String = "${word}s"): String = "$count ${if (count == 1) word else many}"

/** [path] relative to [cwd] when it is inside it. */
fun relativePath(path: String, cwd: String?): String =
    if (!cwd.isNullOrEmpty() && path.startsWith("$cwd/")) path.removePrefix("$cwd/") else path

/** Paths under [home] read as `~/…`. */
fun displayPath(path: String, home: String?): String {
    val base = home?.trimEnd('/').orEmpty()
    return if (base.isNotEmpty() && (path == base || path.startsWith("$base/"))) "~" + path.removePrefix(base) else path
}

private fun oneDecimal(value: Double): String = String.format(Locale.ROOT, "%.1f", value).removeSuffix(".0")

/** 950, 7k, 7.2k, 1.4M. */
fun formatTokens(count: Long): String = when {
    count < 1_000 -> count.toString()
    count < 1_000_000 -> oneDecimal(count / 1_000.0) + "k"
    else -> oneDecimal(count / 1_000_000.0) + "M"
}

/** $0.0046, $1.20; empty when the provider reports no cost. */
fun formatCost(cost: Double): String = when {
    cost <= 0.0 -> ""
    cost < 0.01 -> String.format(Locale.ROOT, "$%.4f", cost)
    else -> String.format(Locale.ROOT, "$%.2f", cost)
}

/** 512 B, 84 KB, 1.4 MB, for file sizes. */
fun formatBytes(bytes: Long): String = when {
    bytes < 1_000 -> "${bytes.coerceAtLeast(0)} B"
    bytes < 1_000_000 -> "${Math.round(bytes / 1_000.0)} KB"
    bytes < 1_000_000_000 -> oneDecimal(bytes / 1_000_000.0) + " MB"
    else -> oneDecimal(bytes / 1_000_000_000.0) + " GB"
}

/** 0:07, 1:30, for a recording's length. */
fun formatClock(millis: Long): String {
    val seconds = (millis / 1_000).coerceAtLeast(0)
    return String.format(Locale.ROOT, "%d:%02d", seconds / 60, seconds % 60)
}

/** 640ms, 1.5s, 3m 04s, 2h 05m. */
fun formatDuration(millis: Long): String {
    if (millis < 1_000) return "${millis.coerceAtLeast(0)}ms"
    if (millis < 60_000) return oneDecimal(millis / 1_000.0) + "s"
    val seconds = Math.round(millis / 1_000.0)
    if (seconds < 3_600) return String.format(Locale.ROOT, "%dm %02ds", seconds / 60, seconds % 60)
    return String.format(Locale.ROOT, "%dh %02dm", seconds / 3_600, (seconds % 3_600) / 60)
}

fun formatDuration(from: Instant, to: Instant): String = formatDuration(Duration.between(from, to).toMillis())

/** "7k in · 20 out", plus cached tokens when there are any. */
fun formatUsage(usage: Usage): String = buildList {
    add("${formatTokens(usage.inputTokens)} in")
    add("${formatTokens(usage.outputTokens)} out")
    if (usage.cacheReadTokens > 0) add("${formatTokens(usage.cacheReadTokens)} cached")
}.joinToString(" · ")

/** "just now", "5m ago", "3h ago", "2d ago". */
fun relativeTime(at: Instant, now: Instant = Instant.now()): String {
    val seconds = Duration.between(at, now).seconds
    return when {
        seconds < 45 -> "just now"
        seconds < 3_600 -> "${(seconds / 60.0).let { Math.round(it).coerceAtLeast(1) }}m ago"
        seconds < 86_400 -> "${seconds / 3_600}h ago"
        else -> "${seconds / 86_400}d ago"
    }
}

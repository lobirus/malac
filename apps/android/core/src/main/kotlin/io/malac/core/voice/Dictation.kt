package io.malac.core.voice

/** Text with dictation inserted, and where the cursor goes: right after it. */
data class Inserted(val text: String, val cursor: Int)

/**
 * Puts [spoken] in [text] in place of the selection from [start] to [end],
 * with a space before it unless it starts the text or follows whitespace,
 * and one after it when a word follows.
 */
fun insertDictation(text: String, start: Int, end: Int, spoken: String): Inserted {
    val words = spoken.trim()
    val from = minOf(start, end).coerceIn(0, text.length)
    val to = maxOf(start, end).coerceIn(0, text.length)
    if (words.isEmpty()) return Inserted(text, to)
    val before = text.substring(0, from)
    val after = text.substring(to)
    val lead = if (before.isNotEmpty() && !before.last().isWhitespace()) " " else ""
    val trail = if (after.isNotEmpty() && after.first().isLetterOrDigit()) " " else ""
    val middle = lead + words + trail
    return Inserted(before + middle + after, before.length + middle.length)
}

package io.malac.core.transcript

/** The tool that sends the user files; its result names them in `<shared …/>` tags. */
const val SEND_TO_USER = "send_to_user"

/** A file an agent sent, served by `GET /api/shared/{id}`. */
data class SharedFile(val id: String, val name: String, val type: String, val size: Long) {
    /** Shown inline. SVG is an image too, but only a browser draws it. */
    val isImage: Boolean get() = type.startsWith("image/") && type != "image/svg+xml"
}

/** What a `send_to_user` call sent: an optional caption and the files. */
data class SharedFiles(val caption: String?, val files: List<SharedFile>)

private val SHARED_TAG = Regex("""<shared\s[^>]*?/>""")
private val CAPTION = Regex("""<caption>(.*?)</caption>""", RegexOption.DOT_MATCHES_ALL)

/**
 * The caption and `<shared id="…" name="…" type="…" size="…"/>` tags in a
 * `send_to_user` result, as `crates/malac-core/src/shared.rs` writes them:
 * names cannot hold quotes or angle brackets, so nothing is escaped.
 */
fun parseShared(output: String): SharedFiles {
    val files = SHARED_TAG.findAll(output).mapNotNull { match ->
        val tag = match.value
        val id = attribute(tag, "id") ?: return@mapNotNull null
        val name = attribute(tag, "name") ?: return@mapNotNull null
        SharedFile(
            id = id,
            name = name,
            type = attribute(tag, "type") ?: "application/octet-stream",
            size = attribute(tag, "size")?.toLongOrNull() ?: 0,
        )
    }.toList()
    val caption = CAPTION.find(output)?.groupValues?.get(1)?.trim()?.ifEmpty { null }
    return SharedFiles(caption, files)
}

private fun attribute(tag: String, key: String): String? =
    Regex("""\s$key="([^"]*)"""").find(tag)?.groupValues?.get(1)
